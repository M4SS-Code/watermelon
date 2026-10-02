use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZero,
    pin::pin,
    time::Duration,
};

use bytes::Bytes;
use claims::{assert_matches, assert_none, assert_ok, assert_some};
use futures_util::{StreamExt as _, TryStreamExt as _};
use serde_json::Value;
use tokio::time::Instant;
use watermelon::{
    core::{Client, error::ResponseError},
    jetstream::{
        AckPolicy, Consumer, ConsumerConfig, ConsumerDurability, ConsumerSpecificConfig,
        ConsumerStorage, DeliverPolicy, DiscardPolicy, JetstreamClient, JetstreamMessage,
        JetstreamPublish, ReplayPolicy, Storage, StreamConfig,
        error::{
            ConsumerStreamError, JetstreamError, JetstreamMessageAckError, JetstreamPublishError,
        },
    },
    proto::Subject,
};
use watermelon_testkit::{NatsServer, TimeoutExt as _, eventually};

use crate::util::{TIMEOUT, connect, server};

async fn jetstream_server() -> Option<NatsServer> {
    NatsServer::builder().jetstream().start().await
}

async fn jetstream(server: &NatsServer) -> JetstreamClient {
    JetstreamClient::new(connect(server).await)
}

fn stream_config(name: &str, subjects: &[&str]) -> StreamConfig {
    StreamConfig {
        name: name.to_owned(),
        subjects: subjects
            .iter()
            .map(|subject| subject.parse().unwrap())
            .collect(),
        max_consumers: None,
        max_messages: None,
        max_bytes: None,
        max_age: Duration::ZERO,
        max_messages_per_subject: None,
        max_message_size: None,
        discard_policy: DiscardPolicy::Old,
        storage: Storage::Memory,
        replicas: NonZero::new(1).unwrap(),
        duplicate_window: Duration::from_secs(120),
        compression: None,
        allow_direct: false,
        mirror_direct: false,
        sealed: false,
        allow_delete: true,
        allow_purge: true,
        allow_rollup_hdrs: false,
    }
}

fn consumer_config(name: &str) -> ConsumerConfig {
    ConsumerConfig {
        durability: ConsumerDurability::Durable,
        name: name.to_owned(),
        description: String::new(),
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit {
            wait: Duration::from_secs(30),
            max_pending: None,
        },
        max_deliver: None,
        backoff: Vec::new(),
        filter_subjects: Vec::new(),
        replay_policy: ReplayPolicy::Instant,
        rate_limit: None,
        headers_only: false,
        specs: ConsumerSpecificConfig::Pull {
            max_waiting: None,
            max_request_batch: None,
            max_request_expires: Duration::ZERO,
            max_request_max_bytes: None,
        },
        inactive_threshold: Duration::ZERO,
        replicas: None,
        storage: ConsumerStorage::Memory,
        metadata: BTreeMap::new(),
    }
}

/// Create stream `S` on `s.>` with consumer `C`
async fn stream_with_consumer(js: &JetstreamClient) -> Consumer {
    js.create_stream(&stream_config("S", &["s.>"]))
        .within(TIMEOUT)
        .await
        .unwrap();
    js.create_consumer("S", &consumer_config("C"))
        .within(TIMEOUT)
        .await
        .unwrap()
}

async fn js_publish(js: &JetstreamClient, subject: &'static str, payload: &'static [u8]) -> u64 {
    js.publish(Subject::from_static(subject))
        .payload(Bytes::from_static(payload))
        .within(TIMEOUT)
        .await
        .unwrap()
        .sequence
}

/// Pull up to `max_msgs` messages without waiting for new ones
async fn pull(js: &JetstreamClient, consumer: &Consumer, max_msgs: usize) -> Vec<JetstreamMessage> {
    js.consumer_batch(consumer, Duration::ZERO, max_msgs)
        .within(TIMEOUT)
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .within(TIMEOUT)
        .await
        .unwrap()
}

/// The raw consumer info, which exposes the delivery state [`Consumer`] doesn't
async fn consumer_info(client: &Client, stream: &str, consumer: &str) -> Value {
    let response = client
        .request(
            format!("$JS.API.CONSUMER.INFO.{stream}.{consumer}")
                .parse()
                .unwrap(),
        )
        .payload(Bytes::new())
        .within(TIMEOUT)
        .await
        .unwrap()
        .within(TIMEOUT)
        .await
        .unwrap();
    serde_json::from_slice(&response.base.payload).unwrap()
}

/// Wait for the consumer to see `expected` undelivered messages
///
/// The stream notifies its consumers of new messages asynchronously, so a
/// `no_wait` pull right after a publish could otherwise come back empty.
async fn wait_for_num_pending(client: &Client, expected: u64) {
    eventually(TIMEOUT, async || {
        consumer_info(client, "S", "C").await["num_pending"] == expected
    })
    .await;
}

/// Wait for the server to apply the acknowledgements sent so far
///
/// `JetStream` processes acks asynchronously, so there is no barrier to sync on.
async fn wait_for_ack_pending(client: &Client, expected: u64) {
    eventually(TIMEOUT, async || {
        consumer_info(client, "S", "C").await["num_ack_pending"] == expected
    })
    .await;
}

fn assert_api_error(result: Result<impl std::fmt::Debug, JetstreamError>, code: u16) {
    match result {
        // `JetstreamApiError` has no accessors: match on its `Display` output
        Err(JetstreamError::Api(err)) => assert!(
            err.to_string().contains(&format!(" code={code} ")),
            "expected API error code {code}, got: {err}"
        ),
        other => panic!("expected API error code {code}, got: {other:?}"),
    }
}

#[tokio::test]
async fn create_and_get_stream() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;

    let created = js
        .create_stream(&stream_config("S", &["s.>"]))
        .within(TIMEOUT)
        .await
        .unwrap();
    assert_eq!(created.config.name, "S");
    assert_eq!(created.config.subjects, [Subject::from_static("s.>")]);

    let fetched = assert_some!(js.stream("S").within(TIMEOUT).await.unwrap());
    assert_eq!(fetched.config.name, "S");
    assert_eq!(fetched.created_at, created.created_at);

    assert_none!(js.stream("MISSING").within(TIMEOUT).await.unwrap());
}

#[tokio::test]
async fn create_stream_is_idempotent_but_rejects_conflicts() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;

    let config = stream_config("S", &["s.>"]);
    js.create_stream(&config).within(TIMEOUT).await.unwrap();
    js.create_stream(&config).within(TIMEOUT).await.unwrap();

    let conflicting = js
        .create_stream(&stream_config("S", &["other.>"]))
        .within(TIMEOUT)
        .await;
    assert_api_error(conflicting, 10058);
}

#[tokio::test]
async fn create_stream_rejects_invalid_names() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;

    let result = js
        .create_stream(&stream_config("bad name", &["s.>"]))
        .within(TIMEOUT)
        .await;
    assert_matches!(result, Err(JetstreamError::Subject(_)));
}

#[tokio::test]
async fn list_streams_across_pages() {
    // The server returns at most 256 streams per page
    const STREAMS: usize = 300;

    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;

    assert_eq!(js.streams().count().within(TIMEOUT).await, 0);

    for i in 0..STREAMS {
        let subject = format!("s{i}.>");
        js.create_stream(&stream_config(&format!("S{i}"), &[&subject]))
            .within(TIMEOUT)
            .await
            .unwrap();
    }

    let names = js
        .streams()
        .map_ok(|stream| stream.config.name)
        .try_collect::<Vec<_>>()
        .within(TIMEOUT)
        .await
        .unwrap();
    let unique = names.iter().collect::<BTreeSet<_>>();
    assert_eq!(unique.len(), STREAMS, "missing streams");
    assert_eq!(names.len(), STREAMS, "duplicated streams");
}

#[tokio::test]
async fn consumer_lifecycle() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;
    let created = stream_with_consumer(&js).await;
    assert_eq!(created.stream_name, "S");
    assert_eq!(created.config.name, "C");

    let fetched = assert_some!(js.consumer("S", "C").within(TIMEOUT).await.unwrap());
    assert_eq!(fetched.created_at, created.created_at);
    let listed = js
        .consumers("S")
        .map_ok(|consumer| consumer.config.name)
        .try_collect::<Vec<_>>()
        .within(TIMEOUT)
        .await
        .unwrap();
    assert_eq!(listed, ["C"]);

    js.delete_consumer("S", "C").within(TIMEOUT).await.unwrap();
    assert_none!(js.consumer("S", "C").within(TIMEOUT).await.unwrap());
    assert_api_error(js.delete_consumer("S", "C").within(TIMEOUT).await, 10014);
}

#[tokio::test]
async fn consumers_of_a_missing_stream() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;

    assert_api_error(js.consumer("MISSING", "C").within(TIMEOUT).await, 10059);
    assert_api_error(
        js.create_consumer("MISSING", &consumer_config("C"))
            .within(TIMEOUT)
            .await,
        10059,
    );
}

#[tokio::test]
async fn consumer_filter_subjects() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;
    js.create_stream(&stream_config("S", &["s.>"]))
        .within(TIMEOUT)
        .await
        .unwrap();

    for (name, filters) in [
        ("NONE", &[][..]),
        ("ONE", &["s.a"][..]),
        ("MANY", &["s.a", "s.b"][..]),
    ] {
        let mut config = consumer_config(name);
        config.filter_subjects = filters.iter().map(|f| f.parse().unwrap()).collect();
        let consumer = js
            .create_consumer("S", &config)
            .within(TIMEOUT)
            .await
            .unwrap();
        assert_eq!(consumer.config.filter_subjects, config.filter_subjects);
    }

    let mut overlapping = consumer_config("OVERLAPPING");
    overlapping.filter_subjects = vec![Subject::from_static("s.>"), Subject::from_static("s.a")];
    assert_api_error(
        js.create_consumer("S", &overlapping).within(TIMEOUT).await,
        10138,
    );
}

#[tokio::test]
async fn list_consumers_across_pages() {
    // The server returns at most 256 consumers per page
    const CONSUMERS: usize = 300;

    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;
    js.create_stream(&stream_config("S", &["s.>"]))
        .within(TIMEOUT)
        .await
        .unwrap();

    for i in 0..CONSUMERS {
        js.create_consumer("S", &consumer_config(&format!("C{i}")))
            .within(TIMEOUT)
            .await
            .unwrap();
    }

    let names = js
        .consumers("S")
        .map_ok(|consumer| consumer.config.name)
        .try_collect::<Vec<_>>()
        .within(TIMEOUT)
        .await
        .unwrap();
    let unique = names.iter().collect::<BTreeSet<_>>();
    assert_eq!(unique.len(), CONSUMERS, "missing consumers");
    assert_eq!(names.len(), CONSUMERS, "duplicated consumers");
}

#[tokio::test]
async fn publish_returns_acks() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;
    js.create_stream(&stream_config("S", &["s.>"]))
        .within(TIMEOUT)
        .await
        .unwrap();

    let ack = js
        .publish(Subject::from_static("s.a"))
        .payload(Bytes::from_static(b"1"))
        .within(TIMEOUT)
        .await
        .unwrap();
    assert_eq!(ack.stream, "S");
    assert_eq!(ack.sequence, 1);
    assert_eq!(ack.duplicate, None);

    let ack = js
        .clone()
        .publish_owned(Subject::from_static("s.a"))
        .payload(Bytes::from_static(b"2"))
        .within(TIMEOUT)
        .await
        .unwrap();
    assert_eq!(ack.sequence, 2);

    let ack = JetstreamPublish::builder(Subject::from_static("s.a"))
        .payload_json(&serde_json::json!({ "n": 3 }))
        .unwrap()
        .client(&js)
        .within(TIMEOUT)
        .await
        .unwrap();
    assert_eq!(ack.sequence, 3);
}

#[tokio::test]
async fn publish_deduplicates_by_message_id() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;
    js.create_stream(&stream_config("S", &["s.>"]))
        .within(TIMEOUT)
        .await
        .unwrap();

    for _ in 0..2 {
        js.publish(Subject::from_static("s.a"))
            .message_id("once")
            .payload(Bytes::new())
            .within(TIMEOUT)
            .await
            .unwrap();
    }
    let duplicate = js
        .publish(Subject::from_static("s.a"))
        .message_id("once")
        .payload(Bytes::new())
        .within(TIMEOUT)
        .await
        .unwrap();
    assert_eq!(duplicate.sequence, 1);
    assert_eq!(duplicate.duplicate, Some(true));
}

#[tokio::test]
async fn publish_expectations_that_hold() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;
    js.create_stream(&stream_config("S", &["s.>"]))
        .within(TIMEOUT)
        .await
        .unwrap();

    js.publish(Subject::from_static("s.a"))
        .message_id("first")
        .payload(Bytes::new())
        .within(TIMEOUT)
        .await
        .unwrap();
    let ack = js
        .publish(Subject::from_static("s.a"))
        .expected_stream("S")
        .expected_last_stream_sequence(1)
        .expected_last_subject_sequence(1)
        .expected_last_message_id("first")
        .payload(Bytes::new())
        .within(TIMEOUT)
        .await
        .unwrap();
    assert_eq!(ack.sequence, 2);
}

#[tokio::test]
async fn publish_expectations_that_fail_are_errors() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;
    js.create_stream(&stream_config("S", &["s.>"]))
        .within(TIMEOUT)
        .await
        .unwrap();
    js_publish(&js, "s.a", b"").await;

    let publish = || js.publish(Subject::from_static("s.a"));
    assert_api_error(
        publish()
            .expected_stream("OTHER")
            .payload(Bytes::new())
            .within(TIMEOUT)
            .await,
        10060,
    );
    assert_api_error(
        publish()
            .expected_last_stream_sequence(5)
            .payload(Bytes::new())
            .within(TIMEOUT)
            .await,
        10071,
    );
    assert_api_error(
        publish()
            .expected_last_subject_sequence(5)
            .payload(Bytes::new())
            .within(TIMEOUT)
            .await,
        10071,
    );
    assert_api_error(
        publish()
            .expected_last_message_id("other")
            .payload(Bytes::new())
            .within(TIMEOUT)
            .await,
        10070,
    );
    // The stream doesn't allow per-message TTLs
    assert_api_error(
        publish()
            .ttl(60)
            .payload(Bytes::new())
            .within(TIMEOUT)
            .await,
        10166,
    );
}

#[tokio::test]
async fn publish_without_a_matching_stream_fails() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;

    let result = js
        .publish(Subject::from_static("no.stream"))
        .payload(Bytes::new())
        .within(TIMEOUT)
        .await;
    assert_matches!(
        result,
        Err(JetstreamError::PublishStatus(
            JetstreamPublishError::NoStreamMatches
        ))
    );
}

#[tokio::test]
async fn publish_rejects_invalid_header_values() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;

    let result = js
        .publish(Subject::from_static("s.a"))
        .message_id("line\r\nbreak")
        .payload(Bytes::new())
        .within(TIMEOUT)
        .await;
    assert_matches!(result, Err(JetstreamError::HeaderValue(_)));
}

#[tokio::test]
async fn consumer_batch_delivers_up_to_max_messages() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;
    let consumer = stream_with_consumer(&js).await;

    assert!(pull(&js, &consumer, 3).await.is_empty());

    for payload in [b"1", b"2", b"3", b"4", b"5"] {
        js_publish(&js, "s.a", payload).await;
    }
    let payloads = |messages: Vec<JetstreamMessage>| {
        messages
            .into_iter()
            .map(|message| message.message.base.payload)
            .collect::<Vec<_>>()
    };
    wait_for_num_pending(js.client(), 5).await;
    assert_eq!(payloads(pull(&js, &consumer, 3).await), ["1", "2", "3"]);
    assert_eq!(payloads(pull(&js, &consumer, 3).await), ["4", "5"]);
    assert!(pull(&js, &consumer, 3).await.is_empty());
}

#[tokio::test]
async fn consumer_batch_with_expiry() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;
    let consumer = stream_with_consumer(&js).await;
    js_publish(&js, "s.a", b"1").await;
    wait_for_num_pending(js.client(), 1).await;

    let messages = js
        .consumer_batch(&consumer, Duration::from_millis(500), 10)
        .within(TIMEOUT)
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .within(TIMEOUT)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
}

#[tokio::test]
async fn consumer_stream_spans_batches() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;
    let consumer = stream_with_consumer(&js).await;

    for i in 0..10 {
        js.publish(Subject::from_static("s.a"))
            .payload(i.to_string().into())
            .within(TIMEOUT)
            .await
            .unwrap();
    }

    let mut stream = pin!(js.consumer_stream(consumer, Duration::ZERO, 3));
    for i in 0..10 {
        let mut message = stream.next().within(TIMEOUT).await.unwrap().unwrap();
        assert_eq!(message.message.base.payload, i.to_string());
        message.ack().within(TIMEOUT).await.unwrap();
    }

    // Messages published while the stream is running are picked up too
    js_publish(&js, "s.a", b"late").await;
    let message = stream.next().within(TIMEOUT).await.unwrap().unwrap();
    assert_eq!(message.message.base.payload, "late");
}

#[tokio::test]
async fn consumer_stream_breaks_on_errors() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;
    let consumer = stream_with_consumer(&js).await;
    js.delete_consumer("S", "C").within(TIMEOUT).await.unwrap();

    let mut stream = pin!(js.consumer_stream(consumer, Duration::ZERO, 3));
    let error = stream.next().within(TIMEOUT).await.unwrap().unwrap_err();
    assert_matches!(error, ConsumerStreamError::BatchError(_));
    assert_none!(stream.next().within(TIMEOUT).await);
}

#[tokio::test]
async fn ack_settles_messages() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;
    let consumer = stream_with_consumer(&js).await;
    js_publish(&js, "s.a", b"1").await;
    wait_for_num_pending(js.client(), 1).await;

    let [mut message] = pull(&js, &consumer, 1).await.try_into().unwrap();
    wait_for_ack_pending(js.client(), 1).await;
    assert!(!message.is_acked());

    message.ack().within(TIMEOUT).await.unwrap();
    assert!(message.is_acked());
    wait_for_ack_pending(js.client(), 0).await;

    assert_matches!(
        message.ack().within(TIMEOUT).await,
        Err(JetstreamMessageAckError::NoReplySubject)
    );
}

#[tokio::test]
async fn acking_twice_is_rejected() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;
    let consumer = stream_with_consumer(&js).await;
    js_publish(&js, "s.a", b"1").await;
    wait_for_num_pending(js.client(), 1).await;

    let [mut message] = pull(&js, &consumer, 1).await.try_into().unwrap();
    message.ack().within(TIMEOUT).await.unwrap();
    // The reply subject is consumed by the first ack
    assert!(message.term().within(TIMEOUT).await.is_err());
}

#[tokio::test]
async fn nak_redelivers() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;
    let consumer = stream_with_consumer(&js).await;
    js_publish(&js, "s.a", b"1").await;

    let mut stream = pin!(js.consumer_stream(consumer, Duration::ZERO, 1));
    let mut message = stream.next().within(TIMEOUT).await.unwrap().unwrap();
    message.nak().within(TIMEOUT).await.unwrap();
    assert!(message.is_acked());

    let redelivered = stream.next().within(TIMEOUT).await.unwrap().unwrap();
    assert_eq!(redelivered.message.base.payload, "1");
    let info = consumer_info(js.client(), "S", "C").await;
    assert_eq!(info["num_redelivered"], 1);
}

#[tokio::test]
async fn nak_with_delay_postpones_redelivery() {
    const DELAY: Duration = Duration::from_millis(300);

    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;
    let consumer = stream_with_consumer(&js).await;
    js_publish(&js, "s.a", b"1").await;

    let mut stream = pin!(js.consumer_stream(consumer, Duration::ZERO, 1));
    let mut message = stream.next().within(TIMEOUT).await.unwrap().unwrap();
    let nacked_at = Instant::now();
    message.nak_with_delay(DELAY).within(TIMEOUT).await.unwrap();

    let redelivered = stream.next().within(TIMEOUT).await.unwrap().unwrap();
    assert_eq!(redelivered.message.base.payload, "1");
    assert!(nacked_at.elapsed() >= DELAY, "redelivered before the delay");
}

#[tokio::test]
async fn term_stops_redelivery() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;
    let consumer = stream_with_consumer(&js).await;
    js_publish(&js, "s.a", b"1").await;
    js_publish(&js, "s.a", b"2").await;
    wait_for_num_pending(js.client(), 2).await;

    let [mut first, mut second] = pull(&js, &consumer, 2).await.try_into().unwrap();
    first.term().within(TIMEOUT).await.unwrap();
    second
        .term_with_reason("not interested")
        .within(TIMEOUT)
        .await
        .unwrap();
    wait_for_ack_pending(js.client(), 0).await;

    assert!(pull(&js, &consumer, 2).await.is_empty());
    let info = consumer_info(js.client(), "S", "C").await;
    assert_eq!(info["num_redelivered"], 0);
}

#[tokio::test]
async fn progress_keeps_messages_pending() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;
    let consumer = stream_with_consumer(&js).await;
    js_publish(&js, "s.a", b"1").await;
    wait_for_num_pending(js.client(), 1).await;

    let [mut message] = pull(&js, &consumer, 1).await.try_into().unwrap();
    message.progress().within(TIMEOUT).await.unwrap();
    assert!(!message.is_acked());
    wait_for_ack_pending(js.client(), 1).await;

    message.ack().within(TIMEOUT).await.unwrap();
    wait_for_ack_pending(js.client(), 0).await;
}

#[tokio::test]
async fn domain_prefixes_the_api() {
    let Some(server) = NatsServer::builder().jetstream_domain("hub").start().await else {
        return;
    };
    let client = connect(&server).await;

    assert!(JetstreamClient::new_with_domain(client.clone(), "bad domain").is_err());

    let js = JetstreamClient::new_with_domain(client, "hub").unwrap();
    assert_eq!(js.prefix().as_str(), "$JS.hub.API");
    js.create_stream(&stream_config("S", &["s.>"]))
        .within(TIMEOUT)
        .await
        .unwrap();
    let ack = js
        .publish(Subject::from_static("s.a"))
        .payload(Bytes::new())
        .within(TIMEOUT)
        .await
        .unwrap();
    assert_eq!(ack.domain.as_deref(), Some("hub"));
}

#[tokio::test]
async fn jetstream_disabled() {
    let Some(server) = server().await else {
        return;
    };
    let js = jetstream(&server).await;

    assert_matches!(
        js.create_stream(&stream_config("S", &["s.>"]))
            .within(TIMEOUT)
            .await,
        Err(JetstreamError::ResponseError(ResponseError::NoResponders))
    );
    assert_matches!(
        js.stream("S").within(TIMEOUT).await,
        Err(JetstreamError::ResponseError(ResponseError::NoResponders))
    );
}

#[tokio::test]
async fn unresponsive_server_times_out() {
    let Some(server) = jetstream_server().await else {
        return;
    };
    let js = jetstream(&server).await;

    server.pause();
    let result = js.stream("S").within(TIMEOUT).await;
    server.resume();
    assert_matches!(
        result,
        Err(JetstreamError::ResponseError(ResponseError::TimedOut))
    );
    assert_ok!(js.stream("S").within(TIMEOUT).await);
}
