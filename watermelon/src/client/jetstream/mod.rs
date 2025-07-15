use std::fmt;
use std::{fmt::Display, time::Duration};

use bytes::Bytes;
use resources::Response;
use serde::{Deserialize, Serialize};
use serde_json::json;
use watermelon_proto::StatusCode;
use watermelon_proto::{Subject, error::SubjectValidateError};

pub use self::commands::{ConsumerBatch, ConsumerStream, ConsumerStreamError, Consumers, Streams};
pub use self::resources::{
    AckPolicy, Compression, Consumer, ConsumerConfig, ConsumerDurability, ConsumerSpecificConfig,
    ConsumerStorage, DeliverPolicy, DiscardPolicy, ReplayPolicy, RetentionPolicy, Storage, Stream,
    StreamConfig, StreamState,
};
use crate::client::ClientRequest;
use crate::core::Client;

use super::{ClientClosedError, ResponseError};

const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

mod commands;
mod resources;

/// A NATS Jetstream client
///
/// `JetstreamClient` is a `Clone`able handle to a NATS [`Client`],
/// with Jetstream specific configurations.
#[derive(Debug, Clone)]
pub struct JetstreamClient {
    client: Client,
    prefix: Subject,
    request_timeout: Duration,
}

/// A Jetstream API error
#[derive(Debug, Deserialize, thiserror::Error)]
#[error("jetstream error status={status} code={code} description={description}")]
pub struct JetstreamApiError {
    #[serde(rename = "code")]
    status: StatusCode,
    #[serde(rename = "err_code")]
    code: JetstreamErrorCode,
    description: String,
}

/// The type of error encountered while processing a Jetstream request
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JetstreamErrorCode(u16);

/// An error encountered while making a Jetstream request
#[derive(Debug, thiserror::Error)]
pub enum JetstreamError {
    #[error("invalid subject")]
    Subject(#[source] SubjectValidateError),
    #[error("client closed")]
    ClientClosed(#[source] ClientClosedError),
    #[error("client request failure")]
    ResponseError(#[source] ResponseError),
    #[error("JSON deserialization")]
    Json(#[source] serde_json::Error),
    #[error("bad response code")]
    Api(#[source] JetstreamApiError),
}

impl JetstreamClient {
    /// Create a Jetstream client using the default configuration
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self::new_with_prefix(client, Subject::from_static("$JS.API"))
    }

    /// Create a Jetstream client using the provided `domain`
    ///
    /// # Errors
    ///
    /// It returns an error if the subject derived by the `domain` is not valid.
    pub fn new_with_domain(
        client: Client,
        domain: impl Display,
    ) -> Result<Self, SubjectValidateError> {
        let prefix = format!("$JS.{domain}.API").try_into()?;
        Ok(Self::new_with_prefix(client, prefix))
    }

    /// Create a Jetstream client using the provided API `prefix`
    #[must_use]
    pub fn new_with_prefix(client: Client, prefix: Subject) -> Self {
        Self {
            client,
            prefix,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }

    /// Create a new stream
    ///
    /// # Errors
    ///
    /// It returns an error if the stream name produces an invalid subject or if an error occurs
    /// while creating the stream.
    pub async fn create_stream(&self, config: &StreamConfig) -> Result<Stream, JetstreamError> {
        let subject = format!("{}.STREAM.CREATE.{}", self.prefix, config.name)
            .try_into()
            .map_err(JetstreamError::Subject)?;

        let payload = serde_json::to_vec(config).map_err(JetstreamError::Json)?;
        let resp = self
            .make_request(subject)
            .payload(payload.into())
            .await
            .map_err(JetstreamError::ClientClosed)?;
        let resp = resp.await.map_err(JetstreamError::ResponseError)?;

        let json = serde_json::from_slice::<Response<Stream>>(&resp.base.payload)
            .map_err(JetstreamError::Json)?;
        match json {
            Response::Response(stream) => Ok(stream),
            Response::Error { error } => Err(JetstreamError::Api(error)),
        }
    }

    /// List streams present within this client's Jetstream context
    pub fn streams(&self) -> Streams {
        Streams::new(self.clone())
    }

    /// Obtain a stream present within this client's Jetstream context
    ///
    /// # Errors
    ///
    /// It returns an error if the given `name` produces an invalid subject or if an error occurs
    /// while creating the stream.
    pub async fn stream(&self, name: impl Display) -> Result<Option<Stream>, JetstreamError> {
        let subject = format!("{}.STREAM.INFO.{}", self.prefix, name)
            .try_into()
            .map_err(JetstreamError::Subject)?;
        let resp = self
            .make_request(subject)
            .payload(Bytes::new())
            .await
            .map_err(JetstreamError::ClientClosed)?;
        let resp = resp.await.map_err(JetstreamError::ResponseError)?;

        let json = serde_json::from_slice::<Response<Stream>>(&resp.base.payload)
            .map_err(JetstreamError::Json)?;
        match json {
            Response::Response(stream) => Ok(Some(stream)),
            Response::Error { error } if error.code == JetstreamErrorCode::STREAM_NOT_FOUND => {
                Ok(None)
            }
            Response::Error { error } => Err(JetstreamError::Api(error)),
        }
    }

    /// Create a new consumer
    ///
    /// # Errors
    ///
    /// It returns an error if the given `stream_name` or consumer name produces an invalid subject
    /// or if an error occurs while creating the consumer.
    pub async fn create_consumer(
        &self,
        stream_name: &str,
        config: &ConsumerConfig,
    ) -> Result<Consumer, JetstreamError> {
        let mut subject = format!(
            "{}.CONSUMER.CREATE.{}.{}",
            self.prefix, stream_name, config.name
        );
        if let [filter_subject] = &*config.filter_subjects {
            subject.push('.');
            subject.push_str(filter_subject);
        }

        let subject = subject.try_into().map_err(JetstreamError::Subject)?;

        let payload = serde_json::to_vec(&json!({
            "stream_name": stream_name,
            "config": config,
            "action": "create",
            "pedantic": true,
        }))
        .map_err(JetstreamError::Json)?;
        let resp = self
            .make_request(subject)
            .payload(payload.into())
            .await
            .map_err(JetstreamError::ClientClosed)?;
        let resp = resp.await.map_err(JetstreamError::ResponseError)?;

        let json = serde_json::from_slice::<Response<Consumer>>(&resp.base.payload)
            .map_err(JetstreamError::Json)?;
        match json {
            Response::Response(consumer) => Ok(consumer),
            Response::Error { error } => Err(JetstreamError::Api(error)),
        }
    }

    /// List consumers present within this client's Jetstream context
    pub fn consumers(&self, stream_name: impl Display) -> Consumers {
        Consumers::new(self.clone(), stream_name)
    }

    /// Obtain a consumer present within this client's Jetstream context
    ///
    /// # Errors
    ///
    /// It returns an error if the given `stream_name` and `consumer_name` produce an invalid
    /// subject or if an error occurs while creating the consumer.
    pub async fn consumer(
        &self,
        stream_name: impl Display,
        consumer_name: impl Display,
    ) -> Result<Option<Consumer>, JetstreamError> {
        let subject = format!(
            "{}.CONSUMER.INFO.{}.{}",
            self.prefix, stream_name, consumer_name
        )
        .try_into()
        .map_err(JetstreamError::Subject)?;
        let resp = self
            .make_request(subject)
            .payload(Bytes::new())
            .await
            .map_err(JetstreamError::ClientClosed)?;
        let resp = resp.await.map_err(JetstreamError::ResponseError)?;

        let json = serde_json::from_slice::<Response<Consumer>>(&resp.base.payload)
            .map_err(JetstreamError::Json)?;
        match json {
            Response::Response(stream) => Ok(Some(stream)),
            Response::Error { error } if error.code == JetstreamErrorCode::CONSUMER_NOT_FOUND => {
                Ok(None)
            }
            Response::Error { error } => Err(JetstreamError::Api(error)),
        }
    }

    /// Run a batch request over the provided `consumer`
    ///
    /// # Errors
    ///
    /// An error is returned if the subject is not valid or if the client has been closed.
    pub async fn consumer_batch(
        &self,
        consumer: &Consumer,
        expires: Duration,
        max_msgs: usize,
    ) -> Result<ConsumerBatch, JetstreamError> {
        ConsumerBatch::new(consumer, self.clone(), expires, max_msgs).await
    }

    /// Run a stream request over the provided `consumer`
    pub fn consumer_stream(
        &self,
        consumer: Consumer,
        expires: Duration,
        max_msgs: usize,
    ) -> ConsumerStream {
        ConsumerStream::new(consumer, self.clone(), expires, max_msgs)
    }

    pub(crate) fn subject_for_request(&self, endpoint: &Subject) -> Subject {
        Subject::from_dangerous_value(format!("{}.{}", self.prefix, endpoint).into())
    }

    fn make_request(&self, subject: Subject) -> ClientRequest<'_> {
        self.client
            .request(subject)
            .response_timeout(self.request_timeout)
    }

    /// Get a reference to the inner NATS Core client
    #[must_use]
    pub fn client(&self) -> &Client {
        &self.client
    }

    #[must_use]
    pub fn prefix(&self) -> &Subject {
        &self.prefix
    }
}

impl JetstreamErrorCode {
    pub const NOT_ENABLED: Self = Self(10076);
    pub const NOT_ENABLED_FOR_ACCOUNT: Self = Self(10039);
    pub const BAD_REQUEST: Self = Self(10003);

    pub const STREAM_NOT_FOUND: Self = Self(10059);
    pub const STREAM_NAME_IN_USE: Self = Self(10058);
    pub const STREAM_MESSAGE_NOT_FOUND: Self = Self(10037);
    pub const STREAM_WRONG_LAST_SEQUENCE: Self = Self(10071);

    pub const COULD_NOT_CREATE_CONSUMER: Self = Self(10012);
    pub const CONSUMER_NOT_FOUND: Self = Self(10014);
    pub const CONSUMER_NAME_IN_USE: Self = Self(10148);

    pub const CONSUMER_DUPLICATE_FILTER_SUBJECTS: Self = Self(10136);
    pub const CONSUMER_OVERLAPPING_FILTER_SUBJECTS: Self = Self(10138);
    pub const CONSUMER_FILTER_SUBJECTS_IS_EMPTY: Self = Self(10139);
}

impl Display for JetstreamErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl From<u16> for JetstreamErrorCode {
    fn from(value: u16) -> Self {
        Self(value)
    }
}

impl From<JetstreamErrorCode> for u16 {
    fn from(value: JetstreamErrorCode) -> Self {
        value.0
    }
}
