use std::{collections::BTreeSet, num::NonZero, time::Duration};

use bytes::Bytes;
use claims::{assert_matches, assert_none, assert_ok};
use futures_util::StreamExt as _;
use tokio::{
    task::{JoinHandle, JoinSet},
    time::Instant,
};
use watermelon::{
    core::{
        Client, Echo, Subscription, error::ResponseError, publish::Publish, request::ResponseFut,
    },
    proto::{
        QueueGroup, Subject,
        headers::{HeaderMap, HeaderName, HeaderValue},
    },
};
use watermelon_testkit::{TimeoutExt as _, eventually};

use crate::util::{
    TIMEOUT, assert_no_message, connect, connect_with, next_message, publish, server, subscribe,
    subscriptions, sync,
};

#[tokio::test]
async fn connect_exchanges_metadata() {
    let Some(server) = server().await else {
        return;
    };
    let client = connect_with(&server, Client::builder().client_name("core-metadata")).await;

    let info = client.server_info().expect("server info after connect");
    assert!(info.supports_headers);
    assert!(client.quick_info().is_connected());
    assert!(!client.quick_info().is_lameduck());

    let connections = server.connections().await;
    assert_eq!(connections.len(), 1);
    assert_eq!(connections[0]["name"], "core-metadata");
    assert_eq!(connections[0]["lang"], "rust-watermelon");
    // The version is the one of watermelon-mini, which builds the CONNECT
    assert!(
        connections[0]["version"]
            .as_str()
            .is_some_and(|v| !v.is_empty())
    );
}

#[tokio::test]
async fn publish_subscribe_roundtrip() {
    let Some(server) = server().await else {
        return;
    };
    let subscriber = connect(&server).await;
    let publisher = connect(&server).await;

    let mut subscription = subscribe(&subscriber, "greetings").await;
    sync(&subscriber).await;

    publish(&publisher, "greetings", b"hello").await;
    let message = next_message(&mut subscription).await;
    assert_eq!(message.base.subject.as_str(), "greetings");
    assert_eq!(message.base.reply_subject, None);
    assert!(message.base.headers.is_empty());
    assert_eq!(message.base.payload, "hello");
    assert_eq!(message.status_code, None);
}

#[tokio::test]
async fn headers_and_reply_subject_roundtrip() {
    let Some(server) = server().await else {
        return;
    };
    let subscriber = connect(&server).await;
    let publisher = connect(&server).await;

    let mut subscription = subscribe(&subscriber, "with.headers").await;
    sync(&subscriber).await;

    let single = HeaderName::from_static("Single");
    let multi = HeaderName::from_static("Multi");
    let mut headers = HeaderMap::new();
    headers.insert(single.clone(), HeaderValue::from_static("one"));
    headers.append(multi.clone(), HeaderValue::from_static("a"));
    headers.append(multi.clone(), HeaderValue::from_static("b"));

    publisher
        .publish(Subject::from_static("with.headers"))
        .reply_subject(Some(Subject::from_static("reply.here")))
        .headers(headers)
        .payload(Bytes::from_static(b"body"))
        .within(TIMEOUT)
        .await
        .unwrap();

    let message = next_message(&mut subscription).await;
    assert_eq!(message.base.reply_subject.as_deref(), Some("reply.here"));
    assert_eq!(
        message.base.headers.get(&single),
        Some(&HeaderValue::from_static("one"))
    );
    assert_eq!(
        message
            .base
            .headers
            .get_all(&multi)
            .map(HeaderValue::as_bytes)
            .collect::<Vec<_>>(),
        [b"a", b"b"]
    );
    assert_eq!(message.base.payload, "body");
}

#[tokio::test]
async fn empty_and_max_size_payloads() {
    let Some(server) = server().await else {
        return;
    };
    let subscriber = connect(&server).await;
    let publisher = connect(&server).await;
    let max_payload = usize::try_from(publisher.server_info().unwrap().max_payload.get()).unwrap();

    let mut subscription = subscribe(&subscriber, "sizes").await;
    sync(&subscriber).await;

    let big = Bytes::from((0..=u8::MAX).cycle().take(max_payload).collect::<Vec<_>>());
    for payload in [Bytes::new(), big] {
        publisher
            .publish(Subject::from_static("sizes"))
            .payload(payload.clone())
            .within(TIMEOUT)
            .await
            .unwrap();
        assert_eq!(next_message(&mut subscription).await.base.payload, payload);
    }
}

#[tokio::test]
async fn messages_from_one_publisher_keep_their_order() {
    // Stays below the subscription buffer, so no message can be dropped
    const MESSAGES: usize = 200;

    let Some(server) = server().await else {
        return;
    };
    let subscriber = connect(&server).await;
    let publisher = connect(&server).await;

    let mut subscription = subscribe(&subscriber, "ordered").await;
    sync(&subscriber).await;

    for i in 0..MESSAGES {
        publisher
            .publish(Subject::from_static("ordered"))
            .payload(i.to_string().into())
            .within(TIMEOUT)
            .await
            .unwrap();
    }
    for i in 0..MESSAGES {
        assert_eq!(
            next_message(&mut subscription).await.base.payload,
            i.to_string()
        );
    }
}

#[tokio::test]
async fn wildcard_subscriptions_match_by_token() {
    let Some(server) = server().await else {
        return;
    };
    let subscriber = connect(&server).await;
    let publisher = connect(&server).await;

    let mut single = subscribe(&subscriber, "orders.*").await;
    let mut full = subscribe(&subscriber, "orders.>").await;
    sync(&subscriber).await;

    // Same publisher, so delivery order matches publish order: the first
    // message each subscription receives proves the earlier ones didn't match.
    publish(&publisher, "orders", b"no match").await;
    publish(&publisher, "other.new", b"no match").await;
    publish(&publisher, "orders.eu.new", b"full only").await;
    publish(&publisher, "orders.new", b"both").await;

    assert_eq!(next_message(&mut single).await.base.payload, "both");
    assert_eq!(next_message(&mut full).await.base.payload, "full only");
    assert_eq!(next_message(&mut full).await.base.payload, "both");

    sync(&publisher).await;
    sync(&subscriber).await;
    assert_no_message(&mut single);
    assert_no_message(&mut full);
}

#[tokio::test]
async fn queue_group_delivers_each_message_once() {
    const MESSAGES: usize = 100;

    let Some(server) = server().await else {
        return;
    };
    let subscriber_a = connect(&server).await;
    let subscriber_b = connect(&server).await;
    let publisher = connect(&server).await;

    let group = QueueGroup::from_static("workers");
    let mut subscriptions = Vec::new();
    for subscriber in [&subscriber_a, &subscriber_b] {
        subscriptions.push(
            subscriber
                .subscribe(Subject::from_static("jobs"), Some(group.clone()))
                .within(TIMEOUT)
                .await
                .unwrap(),
        );
        sync(subscriber).await;
    }

    for i in 0..MESSAGES {
        publisher
            .publish(Subject::from_static("jobs"))
            .payload(i.to_string().into())
            .within(TIMEOUT)
            .await
            .unwrap();
    }
    sync(&publisher).await;
    sync(&subscriber_a).await;
    sync(&subscriber_b).await;

    // Every message has been dispatched by now, so drain without waiting
    let mut received = Vec::new();
    for subscription in &mut subscriptions {
        while let Some(message) = futures_util::FutureExt::now_or_never(subscription.next()) {
            received.push(message.unwrap().unwrap().base.payload);
        }
    }
    let unique = received.iter().collect::<BTreeSet<_>>();
    assert_eq!(received.len(), MESSAGES, "duplicated or lost messages");
    assert_eq!(unique.len(), MESSAGES);
}

#[tokio::test]
async fn echo_is_prevented_by_default() {
    let Some(server) = server().await else {
        return;
    };
    let client = connect(&server).await;

    let mut subscription = subscribe(&client, "echo").await;
    publish(&client, "echo", b"own").await;
    sync(&client).await;
    assert_no_message(&mut subscription);
}

#[tokio::test]
async fn echo_allow_delivers_own_messages() {
    let Some(server) = server().await else {
        return;
    };
    let client = connect_with(&server, Client::builder().echo(Echo::Allow)).await;

    let mut subscription = subscribe(&client, "echo").await;
    publish(&client, "echo", b"own").await;
    assert_eq!(next_message(&mut subscription).await.base.payload, "own");
}

#[tokio::test]
async fn close_after_limits_deliveries() {
    let Some(server) = server().await else {
        return;
    };
    let subscriber = connect(&server).await;
    let publisher = connect(&server).await;

    let mut subscription = subscribe(&subscriber, "limited").await;
    subscription
        .close_after(NonZero::new(3).unwrap())
        .within(TIMEOUT)
        .await
        .unwrap();
    sync(&subscriber).await;

    for _ in 0..5 {
        publish(&publisher, "limited", b"msg").await;
    }
    for _ in 0..3 {
        next_message(&mut subscription).await;
    }
    assert_none!(subscription.next().within(TIMEOUT).await);

    sync(&subscriber).await;
    let connections = server.connections().await;
    assert!(
        connections
            .iter()
            .all(|connection| !subscriptions(connection).contains(&"limited")),
        "server still has the subscription: {connections:?}"
    );
}

#[tokio::test]
async fn close_and_drop_unsubscribe() {
    let Some(server) = server().await else {
        return;
    };
    let client = connect(&server).await;

    let mut closed = subscribe(&client, "closed").await;
    let dropped = subscribe(&client, "dropped").await;
    sync(&client).await;
    let connections = server.connections().await;
    assert_eq!(connections.len(), 1);
    let subjects = subscriptions(&connections[0]);
    assert!(subjects.contains(&"closed") && subjects.contains(&"dropped"));

    closed.close().within(TIMEOUT).await.unwrap();
    drop(dropped);
    sync(&client).await;

    let connections = server.connections().await;
    let subjects = subscriptions(&connections[0]);
    assert!(
        !subjects.contains(&"closed") && !subjects.contains(&"dropped"),
        "server still has the subscriptions: {subjects:?}"
    );
    assert_none!(closed.next().within(TIMEOUT).await);
}

/// Answer every request on `subject` by echoing its payload and headers
async fn spawn_echo_responder(client: &Client, subject: &str) -> JoinHandle<()> {
    let mut requests = subscribe(client, subject).await;
    sync(client).await;

    let client = client.clone();
    tokio::spawn(async move {
        while let Some(Ok(request)) = requests.next().await {
            let reply_subject = request.base.reply_subject.expect("request without reply");
            client
                .publish(reply_subject)
                .headers(request.base.headers)
                .payload(request.base.payload)
                .await
                .unwrap();
        }
    })
}

#[tokio::test]
async fn request_receives_response() {
    let Some(server) = server().await else {
        return;
    };
    let responder = connect(&server).await;
    let requester = connect(&server).await;
    let _responder = spawn_echo_responder(&responder, "service.echo").await;

    let header = HeaderName::from_static("Trace");
    let response = requester
        .request(Subject::from_static("service.echo"))
        .header(header.clone(), HeaderValue::from_static("abc"))
        .payload(Bytes::from_static(b"ping"))
        .within(TIMEOUT)
        .await
        .unwrap()
        .within(TIMEOUT)
        .await
        .unwrap();
    assert_eq!(response.base.payload, "ping");
    assert_eq!(
        response.base.headers.get(&header),
        Some(&HeaderValue::from_static("abc"))
    );
}

#[tokio::test]
async fn request_with_explicit_reply_subject() {
    let Some(server) = server().await else {
        return;
    };
    let responder = connect(&server).await;
    let requester = connect(&server).await;
    let _responder = spawn_echo_responder(&responder, "service.echo").await;

    let response = requester
        .request(Subject::from_static("service.echo"))
        .reply_subject(Some(Subject::from_static("my.reply")))
        .payload(Bytes::from_static(b"ping"))
        .within(TIMEOUT)
        .await
        .unwrap()
        .within(TIMEOUT)
        .await
        .unwrap();
    assert_eq!(response.base.subject.as_str(), "my.reply");
    assert_eq!(response.base.payload, "ping");

    // The dedicated reply subscription only lives for a single response
    sync(&requester).await;
    let connections = server.connections().await;
    assert!(
        connections
            .iter()
            .all(|connection| !subscriptions(connection).contains(&"my.reply")),
        "reply subscription leaked: {connections:?}"
    );
}

#[tokio::test]
async fn concurrent_requests_get_their_own_response() {
    const REQUESTS: usize = 200;

    let Some(server) = server().await else {
        return;
    };
    let responder = connect(&server).await;
    let requester = connect(&server).await;
    let _responder = spawn_echo_responder(&responder, "service.echo").await;

    let mut requests = JoinSet::new();
    for i in 0..REQUESTS {
        let requester = requester.clone();
        requests.spawn(async move {
            let response = requester
                .request_owned(Subject::from_static("service.echo"))
                .payload(i.to_string().into())
                .await
                .unwrap()
                .await
                .unwrap();
            assert_eq!(response.base.payload, i.to_string());
        });
    }
    while let Some(result) = requests.join_next().within(TIMEOUT).await {
        assert_ok!(result);
    }
}

#[tokio::test]
async fn request_without_responders_fails_fast() {
    let Some(server) = server().await else {
        return;
    };
    let client = connect(&server).await;

    let response = client
        .request(Subject::from_static("nobody.home"))
        .response_timeout(Duration::from_secs(60))
        .payload(Bytes::new())
        .within(TIMEOUT)
        .await
        .unwrap()
        .within(TIMEOUT)
        .await;
    assert_matches!(response, Err(ResponseError::NoResponders));
}

/// A subject with a subscriber that never replies
///
/// Keep the returned subscription alive for the subject to stay silent.
async fn silent_responder(client: &Client) -> Subscription {
    let subscription = subscribe(client, "silent").await;
    sync(client).await;
    subscription
}

/// Await `response`, asserting it times out no earlier than `timeout`
async fn assert_times_out(response: ResponseFut, timeout: Duration) {
    let started_at = Instant::now();
    assert_matches!(response.within(TIMEOUT).await, Err(ResponseError::TimedOut));
    assert!(
        started_at.elapsed() >= timeout,
        "timed out after {:?}, before the {timeout:?} timeout",
        started_at.elapsed()
    );
}

#[tokio::test]
async fn request_times_out_without_a_response() {
    let Some(server) = server().await else {
        return;
    };
    let silent = connect(&server).await;
    let requester = connect(&server).await;
    let _requests = silent_responder(&silent).await;

    let timeout = Duration::from_millis(100);
    let response = requester
        .request(Subject::from_static("silent"))
        .response_timeout(timeout)
        .payload(Bytes::new())
        .within(TIMEOUT)
        .await
        .unwrap();
    assert_times_out(response, timeout).await;
}

#[tokio::test]
async fn request_uses_the_client_default_timeout() {
    let Some(server) = server().await else {
        return;
    };
    let silent = connect(&server).await;
    let timeout = Duration::from_millis(100);
    let requester =
        connect_with(&server, Client::builder().default_response_timeout(timeout)).await;
    let _requests = silent_responder(&silent).await;

    let response = requester
        .request(Subject::from_static("silent"))
        .payload(Bytes::new())
        .within(TIMEOUT)
        .await
        .unwrap();
    assert_times_out(response, timeout).await;
}

#[tokio::test]
async fn request_timeout_overrides_the_client_default() {
    let Some(server) = server().await else {
        return;
    };
    let silent = connect(&server).await;
    let requester = connect_with(
        &server,
        Client::builder().default_response_timeout(Duration::from_secs(3600)),
    )
    .await;
    let _requests = silent_responder(&silent).await;

    let timeout = Duration::from_millis(100);
    let response = requester
        .request(Subject::from_static("silent"))
        .response_timeout(timeout)
        .payload(Bytes::new())
        .within(TIMEOUT)
        .await
        .unwrap();
    assert_times_out(response, timeout).await;
}

#[tokio::test]
async fn try_request_times_out() {
    let Some(server) = server().await else {
        return;
    };
    let silent = connect(&server).await;
    let requester = connect(&server).await;
    let _requests = silent_responder(&silent).await;

    let timeout = Duration::from_millis(100);
    let response = requester
        .request(Subject::from_static("silent"))
        .response_timeout(timeout)
        .payload(Bytes::new())
        .try_request()
        .unwrap();
    assert_times_out(response, timeout).await;
}

#[tokio::test]
async fn request_with_explicit_reply_subject_times_out() {
    let Some(server) = server().await else {
        return;
    };
    let silent = connect(&server).await;
    let requester = connect(&server).await;
    let _requests = silent_responder(&silent).await;

    let timeout = Duration::from_millis(100);
    let response = requester
        .request(Subject::from_static("silent"))
        .reply_subject(Some(Subject::from_static("my.reply")))
        .response_timeout(timeout)
        .payload(Bytes::new())
        .within(TIMEOUT)
        .await
        .unwrap();
    assert_times_out(response, timeout).await;

    // Timing out drops the dedicated reply subscription
    sync(&requester).await;
    let connections = server.connections().await;
    assert!(
        connections
            .iter()
            .all(|connection| !subscriptions(connection).contains(&"my.reply")),
        "reply subscription leaked: {connections:?}"
    );
}

#[tokio::test]
async fn request_with_explicit_reply_subject_without_responders() {
    let Some(server) = server().await else {
        return;
    };
    let client = connect(&server).await;

    let response = client
        .request(Subject::from_static("nobody.home"))
        .reply_subject(Some(Subject::from_static("my.reply")))
        .response_timeout(Duration::from_secs(60))
        .payload(Bytes::new())
        .within(TIMEOUT)
        .await
        .unwrap()
        .within(TIMEOUT)
        .await;
    assert_matches!(response, Err(ResponseError::NoResponders));
}

#[tokio::test]
async fn custom_inbox_prefix_is_used_for_replies() {
    let Some(server) = server().await else {
        return;
    };
    let responder = connect(&server).await;
    let requester = connect_with(
        &server,
        Client::builder().inbox_prefix(Subject::from_static("custom.inbox")),
    )
    .await;

    let mut requests = subscribe(&responder, "service").await;
    sync(&responder).await;

    let response = requester
        .request(Subject::from_static("service"))
        .payload(Bytes::new())
        .within(TIMEOUT)
        .await
        .unwrap();
    let request = next_message(&mut requests).await;
    let reply_subject = request.base.reply_subject.unwrap();
    assert!(
        reply_subject.starts_with("custom.inbox."),
        "unexpected reply subject {reply_subject}"
    );

    publish(&responder, &reply_subject, b"pong").await;
    let response = response.within(TIMEOUT).await.unwrap();
    assert_eq!(response.base.payload, "pong");
}

#[tokio::test]
async fn publish_variants_deliver() {
    let Some(server) = server().await else {
        return;
    };
    let subscriber = connect(&server).await;
    let publisher = connect(&server).await;

    let mut subscription = subscribe(&subscriber, "variants").await;
    sync(&subscriber).await;

    let subject = Subject::from_static("variants");
    publisher
        .publish(subject.clone())
        .payload(Bytes::from_static(b"1"))
        .try_publish()
        .unwrap();
    publisher
        .clone()
        .publish_owned(subject.clone())
        .payload(Bytes::from_static(b"2"))
        .within(TIMEOUT)
        .await
        .unwrap();
    Publish::builder(subject.clone())
        .payload(Bytes::from_static(b"3"))
        .client(&publisher)
        .within(TIMEOUT)
        .await
        .unwrap();
    publisher
        .publish(subject)
        .payload_json(&serde_json::json!({ "n": 4 }))
        .unwrap()
        .within(TIMEOUT)
        .await
        .unwrap();

    for expected in ["1", "2", "3", r#"{"n":4}"#] {
        assert_eq!(next_message(&mut subscription).await.base.payload, expected);
    }
}

#[tokio::test]
async fn close_disconnects_and_rejects_commands() {
    let Some(server) = server().await else {
        return;
    };
    let client = connect(&server).await;
    let mut subscription = subscribe(&client, "before.close").await;
    sync(&client).await;

    client.close().within(TIMEOUT).await;

    assert_none!(subscription.next().within(TIMEOUT).await);
    assert!(
        client
            .publish(Subject::from_static("after.close"))
            .payload(Bytes::new())
            .within(TIMEOUT)
            .await
            .is_err()
    );
    assert!(
        client
            .subscribe(Subject::from_static("after.close"), None)
            .within(TIMEOUT)
            .await
            .is_err()
    );
    eventually(TIMEOUT, async || server.connections().await.is_empty()).await;
}
