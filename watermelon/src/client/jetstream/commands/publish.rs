use std::{
    fmt::{self, Debug},
    future::IntoFuture,
};

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use watermelon_proto::{
    StatusCode, Subject,
    headers::{HeaderMap, HeaderName, HeaderValue},
};

use crate::{
    client::{ClientClosedError, JetstreamClient, JetstreamError},
    util::BoxFuture,
};

/// Error returned when a `JetStream` publish does not succeed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum JetstreamPublishError {
    /// `JetStream` is not enabled for this account.
    #[error("jetstream not enabled for this account")]
    JetStreamNotEnabled,
    /// No stream matches the subject.
    #[error("no stream matches the subject")]
    NoStreamMatches,
    /// The stream is full.
    #[error("stream is full")]
    StreamFull,
    /// Messages are being discarded from the stream.
    #[error("messages are being discarded")]
    MessagesDiscarded,
    /// Other `JetStream` publish error.
    #[error("jetstream publish error: {0}")]
    Other(String),
}

/// Acknowledgment of a successful `JetStream` publish.
#[derive(Debug, Deserialize)]
pub struct PubAck {
    /// The stream the message was published to.
    pub stream: String,
    /// The sequence number of the message in the stream.
    pub sequence: u64,
    /// The domain (if applicable).
    #[serde(default)]
    pub domain: Option<String>,
    /// The publish was a duplicate; this is the original sequence.
    #[serde(default)]
    pub duplicate: Option<bool>,
}

/// A publishable `JetStream` message.
#[derive(Debug)]
pub struct JetstreamPublish {
    subject: Subject,
    payload: Bytes,
    stream: Option<String>,
    expected_stream: Option<String>,
    expected_last_stream_sequence: Option<u64>,
    expected_last_subject_sequence: Option<u64>,
    expected_last_message_id: Option<String>,
    message_id: Option<String>,
    ttl: Option<u32>,
}

/// A constructor for a publishable `JetStream` message.
///
/// Obtained from [`JetstreamPublish::builder`].
#[derive(Debug)]
pub struct JetstreamPublishBuilder {
    publish: JetstreamPublish,
}

/// A constructor for a `JetStream` publishable message to be sent using the given client.
///
/// Obtained from [`JetstreamClient::publish`].
pub struct ClientJetstreamPublish<'a> {
    client: &'a JetstreamClient,
    publish: JetstreamPublish,
}

/// A `JetStream` publishable message ready to be published to the given client.
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct DoClientJetstreamPublish<'a> {
    client: &'a JetstreamClient,
    publish: JetstreamPublish,
}

/// A constructor for a `JetStream` publishable message to be sent using the given owned client.
///
/// Obtained from [`JetstreamClient::publish_owned`].
pub struct OwnedClientJetstreamPublish {
    client: JetstreamClient,
    publish: JetstreamPublish,
}

/// A `JetStream` publishable message ready to be published to the given owned client.
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct DoOwnedClientJetstreamPublish {
    client: JetstreamClient,
    publish: JetstreamPublish,
}

macro_rules! jetstream_publish_builder {
    ($payload_t:ty) => {
        /// Set the stream to publish to.
        #[must_use]
        pub fn stream(mut self, stream: &str) -> Self {
            self.publish_mut().stream = Some(stream.to_owned());
            self
        }

        /// Expect the message to be in this stream, fail otherwise.
        #[must_use]
        pub fn expected_stream(mut self, stream: &str) -> Self {
            self.publish_mut().expected_stream = Some(stream.to_owned());
            self
        }

        /// Expect the last stream sequence to match this value.
        #[must_use]
        pub fn expected_last_stream_sequence(mut self, sequence: u64) -> Self {
            self.publish_mut().expected_last_stream_sequence = Some(sequence);
            self
        }

        /// Expect the last subject sequence to match this value.
        #[must_use]
        pub fn expected_last_subject_sequence(mut self, sequence: u64) -> Self {
            self.publish_mut().expected_last_subject_sequence = Some(sequence);
            self
        }

        /// Expect the last message ID to match this value.
        #[must_use]
        pub fn expected_last_message_id(mut self, id: &str) -> Self {
            self.publish_mut().expected_last_message_id = Some(id.to_owned());
            self
        }

        /// Set a message ID for deduplication.
        #[must_use]
        pub fn message_id(mut self, id: &str) -> Self {
            self.publish_mut().message_id = Some(id.to_owned());
            self
        }

        /// Set a TTL in seconds after which to discard the message.
        #[must_use]
        pub fn ttl(mut self, seconds: u32) -> Self {
            self.publish_mut().ttl = Some(seconds);
            self
        }

        /// Serialize `payload` to JSON and use it as the payload.
        ///
        /// # Errors
        ///
        /// Returns an error if the serializer fails.
        pub fn payload_json<T: Serialize>(
            self,
            payload: &T,
        ) -> Result<$payload_t, serde_json::Error> {
            let payload = serde_json::to_vec(payload)?;
            Ok(self.payload(Bytes::from(payload)))
        }
    };
}

impl JetstreamPublish {
    /// Build a new [`JetstreamPublish`].
    #[must_use]
    pub fn builder(subject: Subject) -> JetstreamPublishBuilder {
        JetstreamPublishBuilder::subject(subject)
    }

    /// Publish this message to [`JetstreamClient`].
    pub fn client(self, client: &JetstreamClient) -> DoClientJetstreamPublish<'_> {
        DoClientJetstreamPublish {
            client,
            publish: self,
        }
    }

    /// Publish this message to [`JetstreamClient`], taking ownership of it.
    pub fn client_owned(self, client: JetstreamClient) -> DoOwnedClientJetstreamPublish {
        DoOwnedClientJetstreamPublish {
            client,
            publish: self,
        }
    }
}

impl JetstreamPublishBuilder {
    #[must_use]
    pub fn subject(subject: Subject) -> Self {
        Self {
            publish: JetstreamPublish {
                subject,
                payload: Bytes::new(),
                stream: None,
                expected_stream: None,
                expected_last_stream_sequence: None,
                expected_last_subject_sequence: None,
                expected_last_message_id: None,
                message_id: None,
                ttl: None,
            },
        }
    }

    jetstream_publish_builder!(JetstreamPublish);

    #[must_use]
    pub fn payload(mut self, payload: Bytes) -> JetstreamPublish {
        self.publish.payload = payload;
        self.publish
    }

    fn publish_mut(&mut self) -> &mut JetstreamPublish {
        &mut self.publish
    }
}

impl<'a> ClientJetstreamPublish<'a> {
    pub(crate) fn build(client: &'a JetstreamClient, subject: Subject) -> Self {
        Self {
            client,
            publish: JetstreamPublishBuilder::subject(subject).publish,
        }
    }

    jetstream_publish_builder!(DoClientJetstreamPublish<'a>);

    pub fn payload(mut self, payload: Bytes) -> DoClientJetstreamPublish<'a> {
        self.publish.payload = payload;
        self.publish.client(self.client)
    }

    /// Convert this into [`OwnedClientJetstreamPublish`].
    #[must_use]
    pub fn to_owned(self) -> OwnedClientJetstreamPublish {
        OwnedClientJetstreamPublish {
            client: self.client.clone(),
            publish: self.publish,
        }
    }

    fn publish_mut(&mut self) -> &mut JetstreamPublish {
        &mut self.publish
    }
}

impl OwnedClientJetstreamPublish {
    pub(crate) fn build(client: JetstreamClient, subject: Subject) -> Self {
        Self {
            client,
            publish: JetstreamPublishBuilder::subject(subject).publish,
        }
    }

    jetstream_publish_builder!(DoOwnedClientJetstreamPublish);

    pub fn payload(mut self, payload: Bytes) -> DoOwnedClientJetstreamPublish {
        self.publish.payload = payload;
        self.publish.client_owned(self.client)
    }

    fn publish_mut(&mut self) -> &mut JetstreamPublish {
        &mut self.publish
    }
}

impl DoClientJetstreamPublish<'_> {
    /// Publish this message and await the [`PubAck`].
    ///
    /// # Errors
    ///
    /// Returns an error if the client is closed or the server returns an error.
    pub async fn publish(self) -> Result<PubAck, JetstreamError> {
        do_publish(self.client, self.publish).await
    }
}

impl<'a> IntoFuture for DoClientJetstreamPublish<'a> {
    type Output = Result<PubAck, JetstreamError>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { do_publish(self.client, self.publish).await })
    }
}

impl DoOwnedClientJetstreamPublish {
    /// Publish this message and await the [`PubAck`].
    ///
    /// # Errors
    ///
    /// Returns an error if the client is closed or the server returns an error.
    pub async fn publish(self) -> Result<PubAck, JetstreamError> {
        do_publish(&self.client, self.publish).await
    }
}

impl IntoFuture for DoOwnedClientJetstreamPublish {
    type Output = Result<PubAck, JetstreamError>;
    type IntoFuture = BoxFuture<'static, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { do_publish(&self.client, self.publish).await })
    }
}

/// Internal helper used by both the builder types and `JetstreamClient::publish`.
pub(crate) async fn do_publish(
    client: &JetstreamClient,
    jetstream_publish: JetstreamPublish,
) -> Result<PubAck, JetstreamError> {
    let JetstreamPublish {
        subject,
        payload,
        stream,
        expected_stream,
        expected_last_stream_sequence,
        expected_last_subject_sequence,
        expected_last_message_id,
        message_id,
        ttl,
    } = jetstream_publish;

    let headers = build_headers(
        stream.as_deref(),
        expected_stream.as_deref(),
        expected_last_stream_sequence,
        expected_last_subject_sequence,
        expected_last_message_id.as_deref(),
        message_id.as_deref(),
        ttl,
    );

    // Use the core client's request API — it handles reply subjects,
    // multiplexed subscriptions, and timeouts for us.
    let response_fut = client
        .client()
        .request(subject)
        .headers(headers)
        .payload(payload)
        .await
        .map_err(JetstreamError::ClientClosed)?;

    let response = response_fut
        .await
        .map_err(|_| JetstreamError::ClientClosed(ClientClosedError))?;

    // Check for status codes
    if let Some(status) = response.status_code {
        if status == StatusCode::NO_RESPONDERS {
            return Err(JetstreamError::PublishStatus(
                crate::client::jetstream::JetstreamPublishError::JetStreamNotEnabled,
            ));
        }
        let status_u16 = u16::from(status);
        return Err(match status_u16 {
            503 => JetstreamError::PublishStatus(
                crate::client::jetstream::JetstreamPublishError::JetStreamNotEnabled,
            ),
            409 => JetstreamError::PublishStatus(
                crate::client::jetstream::JetstreamPublishError::NoStreamMatches,
            ),
            _ => {
                let detail = String::from_utf8_lossy(&response.base.payload).to_string();
                JetstreamError::PublishStatus(
                    crate::client::jetstream::JetstreamPublishError::Other(detail),
                )
            }
        });
    }

    let pub_ack =
        serde_json::from_slice::<PubAck>(&response.base.payload).map_err(JetstreamError::Json)?;
    Ok(pub_ack)
}

pub(crate) fn build_headers(
    stream: Option<&str>,
    expected_stream: Option<&str>,
    expected_last_stream_sequence: Option<u64>,
    expected_last_subject_sequence: Option<u64>,
    expected_last_message_id: Option<&str>,
    message_id: Option<&str>,
    ttl: Option<u32>,
) -> HeaderMap {
    let mut headers = HeaderMap::new();

    if let Some(s) = stream {
        headers.insert(
            HeaderName::from_static("Nats-Stream"),
            HeaderValue::from_dangerous_value(s.into()),
        );
    }
    if let Some(s) = expected_stream {
        headers.insert(
            HeaderName::from_static("Nats-Expected-Stream"),
            HeaderValue::from_dangerous_value(s.into()),
        );
    }
    if let Some(seq) = expected_last_stream_sequence {
        headers.insert(
            HeaderName::from_static("Nats-Expected-Last-Sequence"),
            HeaderValue::from_dangerous_value(seq.to_string().into()),
        );
    }
    if let Some(seq) = expected_last_subject_sequence {
        headers.insert(
            HeaderName::from_static("Nats-Expected-Last-Subject-Sequence"),
            HeaderValue::from_dangerous_value(seq.to_string().into()),
        );
    }
    if let Some(id) = expected_last_message_id {
        headers.insert(
            HeaderName::from_static("Nats-Expected-Last-Message-Id"),
            HeaderValue::from_dangerous_value(id.into()),
        );
    }
    if let Some(id) = message_id {
        headers.insert(
            HeaderName::from_static("Nats-Message-Id"),
            HeaderValue::from_dangerous_value(id.into()),
        );
    }
    if let Some(t) = ttl {
        headers.insert(
            HeaderName::from_static("Nats-TTL"),
            HeaderValue::from_dangerous_value(t.to_string().into()),
        );
    }

    headers
}

impl Debug for ClientJetstreamPublish<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientJetstreamPublish")
            .field("publish", &self.publish)
            .finish_non_exhaustive()
    }
}

impl Debug for DoClientJetstreamPublish<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DoClientJetstreamPublish")
            .field("publish", &self.publish)
            .finish_non_exhaustive()
    }
}

impl Debug for OwnedClientJetstreamPublish {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedClientJetstreamPublish")
            .field("publish", &self.publish)
            .finish_non_exhaustive()
    }
}

impl Debug for DoOwnedClientJetstreamPublish {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DoOwnedClientJetstreamPublish")
            .field("publish", &self.publish)
            .finish_non_exhaustive()
    }
}
