use std::{
    fmt::{Debug, Display},
    time::Duration,
};

use bytes::Bytes;
use watermelon_proto::{ServerMessage, Subject};

use crate::client::{Client, ClientClosedError};

/// An error encountered while acknowledging a `JetStream` message.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum JetstreamMessageAckError {
    /// The client has been closed.
    #[error("client closed")]
    ClientClosed(#[source] ClientClosedError),
    /// The message has no reply subject to acknowledge on.
    #[error("message has no reply subject")]
    NoReplySubject,
    /// The message was already acknowledged (ack/nak/term).
    /// Progress (`+WPI`) does not trigger this.
    #[error("message already acknowledged")]
    AlreadyAcknowledged,
}

/// A `JetStream` message that supports acknowledgment operations.
///
/// Wraps a [`ServerMessage`] and provides methods for ACK, NAK, TERM,
/// and progress indicator. Obtained from [`ConsumerBatch`](super::ConsumerBatch)
/// and [`ConsumerStream`](super::ConsumerStream).
///
/// The [`message`](JetstreamMessage::message) field is public, matching the
/// [`ServerMessage`] pattern — access `subject`, `reply_subject`, `headers`,
/// and `payload` directly on `msg.message.base`.
pub struct JetstreamMessage {
    /// The underlying server message. Access `.message.base.subject`, `.message.base.headers`,
    /// `.message.base.payload`, `.message.base.reply_subject`, `.message.base.status_code`,
    /// `.message.base.subscription_id`.
    pub message: ServerMessage,
    /// Client handle, dropped after a terminal acknowledgment.
    client: Option<Client>,
    /// Whether the message has been acknowledged (ack, nak, or term).
    /// Progress (`+WPI`) does NOT set this flag, as it can be sent multiple times.
    acknowledged: bool,
}

impl JetstreamMessage {
    /// Construct a new [`JetstreamMessage`] from a raw server message.
    #[must_use]
    pub(crate) fn new(message: ServerMessage, client: Client) -> Self {
        Self {
            message,
            client: Some(client),
            acknowledged: false,
        }
    }

    /// Acknowledge the message has been processed.
    ///
    /// After calling this, the server will not redeliver the message.
    ///
    /// # Errors
    ///
    /// Returns an error if the message has no reply subject, was already
    /// acknowledged, or the client is closed.
    pub async fn ack(&mut self) -> Result<(), JetstreamMessageAckError> {
        let reply_subject = self.take_reply_subject()?;
        self.ensure_not_acked()?;

        let client = self.take_client()?;

        client
            .publish(reply_subject)
            .payload(Bytes::from_static(b"+ACK"))
            .await
            .map_err(JetstreamMessageAckError::ClientClosed)?;
        self.mark_acknowledged();
        Ok(())
    }

    /// Negative acknowledge — request redelivery.
    ///
    /// The message will be redelivered after the consumer's `ack_wait` period.
    ///
    /// # Errors
    ///
    /// Returns an error if the message has no reply subject, was already
    /// acknowledged, or the client is closed.
    pub async fn nak(&mut self) -> Result<(), JetstreamMessageAckError> {
        let reply_subject = self.take_reply_subject()?;
        self.ensure_not_acked()?;

        let client = self.take_client()?;

        client
            .publish(reply_subject)
            .payload(Bytes::from_static(b"-NAK"))
            .await
            .map_err(JetstreamMessageAckError::ClientClosed)?;
        self.mark_acknowledged();
        Ok(())
    }

    /// Negative acknowledge with a delay before redelivery.
    ///
    /// The message will be redelivered after the specified delay.
    ///
    /// # Errors
    ///
    /// Returns an error if the message has no reply subject, was already
    /// acknowledged, or the client is closed.
    pub async fn nak_with_delay(
        &mut self,
        delay: Duration,
    ) -> Result<(), JetstreamMessageAckError> {
        let reply_subject = self.take_reply_subject()?;
        self.ensure_not_acked()?;

        let payload = format!("-NAK {{\"delay\": {}}}", delay.as_nanos()).into_bytes();

        let client = self.take_client()?;

        client
            .publish(reply_subject)
            .payload(Bytes::from(payload))
            .await
            .map_err(JetstreamMessageAckError::ClientClosed)?;
        self.mark_acknowledged();
        Ok(())
    }

    /// Send a progress indicator — extend the redelivery deadline.
    ///
    /// Resets the redelivery timer without acknowledging or rejecting the message.
    /// Can be called multiple times, unlike [`ack`](Self::ack), [`nak`](Self::nak),
    /// and [`term`](Self::term).
    ///
    /// # Errors
    ///
    /// Returns an error if the message has no reply subject or the client is closed.
    pub async fn progress(&self) -> Result<(), JetstreamMessageAckError> {
        let reply_subject = self
            .message
            .base
            .reply_subject
            .as_ref()
            .ok_or(JetstreamMessageAckError::NoReplySubject)?
            .clone();

        let client = self
            .client
            .as_ref()
            .ok_or(JetstreamMessageAckError::NoReplySubject)?;

        client
            .publish(reply_subject)
            .payload(Bytes::from_static(b"+WPI"))
            .await
            .map_err(JetstreamMessageAckError::ClientClosed)?;
        Ok(())
    }

    /// Terminate the message — stop all redelivery attempts.
    ///
    /// The message will never be redelivered, regardless of the `max_deliver` setting.
    ///
    /// # Errors
    ///
    /// Returns an error if the message has no reply subject, was already
    /// acknowledged, or the client is closed.
    pub async fn term(&mut self) -> Result<(), JetstreamMessageAckError> {
        let reply_subject = self.take_reply_subject()?;
        self.ensure_not_acked()?;

        let client = self.take_client()?;

        client
            .publish(reply_subject)
            .payload(Bytes::from_static(b"+TERM"))
            .await
            .map_err(JetstreamMessageAckError::ClientClosed)?;
        self.mark_acknowledged();
        Ok(())
    }

    /// Terminate the message with a reason.
    ///
    /// The reason will be included in the `JetStream` advisory event sent by the server.
    ///
    /// # Errors
    ///
    /// Returns an error if the message has no reply subject, was already
    /// acknowledged, or the client is closed.
    pub async fn term_with_reason(
        &mut self,
        reason: impl Display,
    ) -> Result<(), JetstreamMessageAckError> {
        let reply_subject = self.take_reply_subject()?;
        self.ensure_not_acked()?;

        let payload = format!("+TERM {reason}").into_bytes();

        let client = self.take_client()?;

        client
            .publish(reply_subject)
            .payload(Bytes::from(payload))
            .await
            .map_err(JetstreamMessageAckError::ClientClosed)?;
        self.mark_acknowledged();
        Ok(())
    }

    /// Check if the message has been acknowledged (ack/nak/term).
    /// Progress (`+WPI`) does not set this flag.
    #[must_use]
    pub fn is_acked(&self) -> bool {
        self.acknowledged
    }

    fn take_reply_subject(&mut self) -> Result<Subject, JetstreamMessageAckError> {
        self.message
            .base
            .reply_subject
            .take()
            .ok_or(JetstreamMessageAckError::NoReplySubject)
    }

    fn take_client(&mut self) -> Result<Client, JetstreamMessageAckError> {
        self.client
            .take()
            .ok_or(JetstreamMessageAckError::NoReplySubject)
    }

    fn ensure_not_acked(&self) -> Result<(), JetstreamMessageAckError> {
        if self.acknowledged {
            return Err(JetstreamMessageAckError::AlreadyAcknowledged);
        }
        Ok(())
    }

    fn mark_acknowledged(&mut self) {
        self.acknowledged = true;
    }
}

impl Debug for JetstreamMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JetstreamMessage")
            .field("message", &self.message)
            .field("acknowledged", &self.acknowledged)
            .finish_non_exhaustive()
    }
}
