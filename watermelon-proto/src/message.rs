use bytes::Bytes;
use bytestring::ByteString;

use crate::{StatusCode, Subject, headers::HeaderMap, subscription_id::SubscriptionId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageBase {
    pub subject: Subject,
    pub reply_subject: Option<Subject>,
    pub headers: HeaderMap,
    pub payload: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerMessage {
    pub status_code: Option<StatusCode>,
    /// The human readable description following [`Self::status_code`]
    ///
    /// Some status codes, like `409`, are used by the NATS Server for
    /// multiple unrelated conditions which can only be told apart
    /// via this description.
    pub status_description: Option<ByteString>,
    pub subscription_id: SubscriptionId,
    pub base: MessageBase,
}
