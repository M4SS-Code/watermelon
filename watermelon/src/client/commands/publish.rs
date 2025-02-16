use std::{
    fmt::{self, Debug},
    future::IntoFuture,
};

use bytes::Bytes;
use watermelon_proto::{
    headers::{HeaderMap, HeaderName, HeaderValue},
    MessageBase, Subject,
};

use crate::{
    client::{Client, ClientClosedError, TryCommandError},
    handler::HandlerCommand,
    util::BoxFuture,
};

use super::Request;

/// A publishable message
#[derive(Debug, Clone)]
pub struct Publish {
    pub(super) subject: Subject,
    pub(super) reply_subject: Option<Subject>,
    pub(super) headers: HeaderMap,
    pub(super) payload: Bytes,
}

/// A constructor for a publishable message
///
/// Obtained from [`Publish::builder`].
#[derive(Debug)]
pub struct PublishBuilder {
    publish: Publish,
}

/// A constructor for a publishable message to be sent using the given client
///
/// Obtained from [`Client::publish`].
pub struct ClientPublish<'a> {
    client: &'a Client,
    publish: Publish,
}

/// A publisheable message ready to be published to the given client
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct DoClientPublish<'a> {
    client: &'a Client,
    publish: Publish,
}

/// A constructor for a publishable message to be sent using the given owned client
///
/// Obtained from [`Client::publish_owned`].
pub struct OwnedClientPublish {
    client: Client,
    publish: Publish,
}

/// A publisheable message ready to be published to the given owned client
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct DoOwnedClientPublish {
    client: Client,
    publish: Publish,
}

macro_rules! publish {
    () => {
        #[must_use]
        pub fn reply_subject(mut self, reply_subject: Option<Subject>) -> Self {
            self.publish_mut().reply_subject = reply_subject;
            self
        }

        #[must_use]
        pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
            self.publish_mut().headers.insert(name, value);
            self
        }

        #[must_use]
        pub fn headers(mut self, headers: HeaderMap) -> Self {
            self.publish_mut().headers = headers;
            self
        }
    };
}

impl Publish {
    /// Build a new [`Publish`]
    #[must_use]
    pub fn builder(subject: Subject) -> PublishBuilder {
        PublishBuilder::subject(subject)
    }

    /// Publish this message to `client`
    pub fn client(self, client: &Client) -> DoClientPublish<'_> {
        DoClientPublish {
            client,
            publish: self,
        }
    }

    /// Publish this message to `client`, taking ownership of it
    pub fn client_owned(self, client: Client) -> DoOwnedClientPublish {
        DoOwnedClientPublish {
            client,
            publish: self,
        }
    }

    pub fn into_request(self) -> Request {
        Request {
            publish: self,
            response_timeout: None,
        }
    }

    fn into_message_base(self) -> MessageBase {
        let Self {
            subject,
            reply_subject,
            headers,
            payload,
        } = self;
        MessageBase {
            subject,
            reply_subject,
            headers,
            payload,
        }
    }
}

impl PublishBuilder {
    #[must_use]
    pub fn subject(subject: Subject) -> Self {
        Self {
            publish: Publish {
                subject,
                reply_subject: None,
                headers: HeaderMap::new(),
                payload: Bytes::new(),
            },
        }
    }

    publish!();

    #[must_use]
    pub fn payload(mut self, payload: Bytes) -> Publish {
        self.publish.payload = payload;
        self.publish
    }

    fn publish_mut(&mut self) -> &mut Publish {
        &mut self.publish
    }
}

impl<'a> ClientPublish<'a> {
    pub(crate) fn build(client: &'a Client, subject: Subject) -> Self {
        Self {
            client,
            publish: PublishBuilder::subject(subject).publish,
        }
    }

    publish!();

    pub fn payload(mut self, payload: Bytes) -> DoClientPublish<'a> {
        self.publish.payload = payload;
        self.publish.client(self.client)
    }

    /// Convert this into [`OwnedClientPublish`]
    #[must_use]
    pub fn to_owned(self) -> OwnedClientPublish {
        OwnedClientPublish {
            client: self.client.clone(),
            publish: self.publish,
        }
    }

    fn publish_mut(&mut self) -> &mut Publish {
        &mut self.publish
    }
}

impl OwnedClientPublish {
    pub(crate) fn build(client: Client, subject: Subject) -> Self {
        Self {
            client,
            publish: PublishBuilder::subject(subject).publish,
        }
    }

    publish!();

    pub fn payload(mut self, payload: Bytes) -> DoOwnedClientPublish {
        self.publish.payload = payload;
        self.publish.client_owned(self.client)
    }

    fn publish_mut(&mut self) -> &mut Publish {
        &mut self.publish
    }
}

impl DoClientPublish<'_> {
    /// Publish this message if there's enough immediately available space in the internal buffers
    ///
    /// This method will publish the given message only if there's enough
    /// immediately available space to enqueue it in the client's
    /// networking stack.
    ///
    /// # Errors
    ///
    /// It returns an error if the client's buffer is full or if the client has been closed.
    pub fn try_publish(self) -> Result<(), TryCommandError> {
        try_publish(self.client, self.publish)
    }
}

impl<'a> IntoFuture for DoClientPublish<'a> {
    type Output = Result<(), ClientClosedError>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { publish(self.client, self.publish).await })
    }
}

impl DoOwnedClientPublish {
    /// Publish this message if there's enough immediately available space in the internal buffers
    ///
    /// This method will publish the given message only if there's enough
    /// immediately available space to enqueue it in the client's
    /// networking stack.
    ///
    /// # Errors
    ///
    /// It returns an error if the client's buffer is full or if the client has been closed.
    pub fn try_publish(self) -> Result<(), TryCommandError> {
        try_publish(&self.client, self.publish)
    }
}

impl IntoFuture for DoOwnedClientPublish {
    type Output = Result<(), ClientClosedError>;
    type IntoFuture = BoxFuture<'static, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { publish(&self.client, self.publish).await })
    }
}

fn try_publish(client: &Client, publish: Publish) -> Result<(), TryCommandError> {
    client.try_enqueue_command(HandlerCommand::Publish {
        message: publish.into_message_base(),
    })
}

async fn publish(client: &Client, publish: Publish) -> Result<(), ClientClosedError> {
    client
        .enqueue_command(HandlerCommand::Publish {
            message: publish.into_message_base(),
        })
        .await
}

impl Debug for ClientPublish<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientPublish")
            .field("publish", &self.publish)
            .finish_non_exhaustive()
    }
}

impl Debug for DoClientPublish<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DoClientPublish")
            .field("publish", &self.publish)
            .finish_non_exhaustive()
    }
}

impl Debug for OwnedClientPublish {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedClientPublish")
            .field("publish", &self.publish)
            .finish_non_exhaustive()
    }
}

impl Debug for DoOwnedClientPublish {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DoOwnedClientPublish")
            .field("publish", &self.publish)
            .finish_non_exhaustive()
    }
}
