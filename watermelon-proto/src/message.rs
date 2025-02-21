use bytes::Bytes;

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
    pub subscription_id: SubscriptionId,
    pub base: MessageBase,
}
