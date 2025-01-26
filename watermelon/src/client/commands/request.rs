use std::{
    fmt::{self, Debug},
    future::{Future, IntoFuture},
    num::NonZeroU64,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use bytes::Bytes;
use futures_core::Stream;
use pin_project_lite::pin_project;
use tokio::time::{sleep, Sleep};
use watermelon_proto::{
    error::ServerError,
    headers::{HeaderMap, HeaderName, HeaderValue},
    ServerMessage, StatusCode, Subject,
};

use crate::{
    client::{Client, ClientClosedError, TryCommandError},
    core::MultiplexedSubscription,
    future::BoxFuture,
    subscription::Subscription,
};

use super::Publish;

/// A publishable request
#[derive(Debug, Clone)]
pub struct Request {
    pub(super) publish: Publish,
    pub(super) response_timeout: Option<Duration>,
}

/// A constructor for a publishable request
///
/// Obtained from [`Request::builder`].
#[derive(Debug)]
pub struct RequestBuilder {
    request: Request,
}

/// A constructor for a publishable request to be sent using the given client
///
/// Obtained from [`Client::request`].
pub struct ClientRequest<'a> {
    client: &'a Client,
    request: Request,
}

/// A publisheable request ready to be published to the given client
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct DoClientRequest<'a> {
    client: &'a Client,
    request: Request,
}

/// A constructor for a publishable request to be sent using the given owned client
///
/// Obtained from [`Client::request_owned`].
pub struct OwnedClientRequest {
    client: Client,
    request: Request,
}

/// A publisheable request ready to be published to the given owned client
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct DoOwnedClientRequest {
    client: Client,
    request: Request,
}

pin_project! {
    /// A [`Future`] for receiving a response
    #[derive(Debug)]
    #[must_use = "consider using a `Publish` instead of `Request` if uninterested in the response"]
    pub struct ResponseFut {
        subscription: ResponseSubscription,
        #[pin]
        timeout: Sleep,
    }
}

#[derive(Debug)]
enum ResponseSubscription {
    Multiplexed(MultiplexedSubscription),
    Subscription(Subscription),
}

/// An error encountered while waiting for a response
#[derive(Debug, thiserror::Error)]
pub enum ResponseError {
    /// The [`Subscription`] encountered a server error
    #[error("server error")]
    ServerError(#[source] ServerError),
    /// The NATS server told us that no subscriptions are present for the requested subject
    #[error("no responders")]
    NoResponders,
    /// A response hasn't been received within the timeout
    #[error("received no response within the timeout window")]
    TimedOut,
    /// The [`Subscription`] was closed without yielding any message
    ///
    /// On a multiplexed subscription this may mean that the client
    /// reconnected to the server
    #[error("subscription closed")]
    SubscriptionClosed,
}

macro_rules! request {
    () => {
        #[must_use]
        pub fn reply_subject(mut self, reply_subject: Option<Subject>) -> Self {
            self.request_mut().publish.reply_subject = reply_subject;
            self
        }

        #[must_use]
        pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
            self.request_mut().publish.headers.insert(name, value);
            self
        }

        #[must_use]
        pub fn headers(mut self, headers: HeaderMap) -> Self {
            self.request_mut().publish.headers = headers;
            self
        }

        #[must_use]
        pub fn response_timeout(mut self, timeout: Duration) -> Self {
            self.request_mut().response_timeout = Some(timeout);
            self
        }
    };
}

impl Request {
    /// Build a new [`Request`]
    #[must_use]
    pub fn builder(subject: Subject) -> RequestBuilder {
        RequestBuilder::subject(subject)
    }

    /// Publish this request to `client`
    pub fn client(self, client: &Client) -> DoClientRequest<'_> {
        DoClientRequest {
            client,
            request: self,
        }
    }

    /// Publish this request to `client`, taking ownership of it
    pub fn client_owned(self, client: Client) -> DoOwnedClientRequest {
        DoOwnedClientRequest {
            client,
            request: self,
        }
    }
}

impl RequestBuilder {
    #[must_use]
    pub fn subject(subject: Subject) -> Self {
        Self {
            request: Request {
                publish: Publish {
                    subject,
                    reply_subject: None,
                    headers: HeaderMap::new(),
                    payload: Bytes::new(),
                },
                response_timeout: None,
            },
        }
    }

    request!();

    #[must_use]
    pub fn payload(mut self, payload: Bytes) -> Request {
        self.request.publish.payload = payload;
        self.request
    }

    fn request_mut(&mut self) -> &mut Request {
        &mut self.request
    }
}

impl<'a> ClientRequest<'a> {
    pub(crate) fn build(client: &'a Client, subject: Subject) -> Self {
        Self {
            client,
            request: RequestBuilder::subject(subject).request,
        }
    }

    request!();

    pub fn payload(mut self, payload: Bytes) -> DoClientRequest<'a> {
        self.request.publish.payload = payload;
        self.request.client(self.client)
    }

    /// Convert this into [`OwnedClientRequest`]
    #[must_use]
    pub fn to_owned(self) -> OwnedClientRequest {
        OwnedClientRequest {
            client: self.client.clone(),
            request: self.request,
        }
    }

    fn request_mut(&mut self) -> &mut Request {
        &mut self.request
    }
}

impl OwnedClientRequest {
    pub(crate) fn build(client: Client, subject: Subject) -> Self {
        Self {
            client,
            request: RequestBuilder::subject(subject).request,
        }
    }

    request!();

    pub fn payload(mut self, payload: Bytes) -> DoOwnedClientRequest {
        self.request.publish.payload = payload;
        self.request.client_owned(self.client)
    }

    fn request_mut(&mut self) -> &mut Request {
        &mut self.request
    }
}

impl DoClientRequest<'_> {
    /// Publish this request if there's enough immediately available space in the internal buffers
    ///
    /// This method will publish the given request only if there's enough
    /// immediately available space to enqueue it in the client's
    /// networking stack.
    ///
    /// # Errors
    ///
    /// It returns an error if the client's buffer is full or if the client has been closed.
    pub fn try_request(self) -> Result<ResponseFut, TryCommandError> {
        try_request(self.client, self.request)
    }
}

impl<'a> IntoFuture for DoClientRequest<'a> {
    type Output = Result<ResponseFut, ClientClosedError>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { request(self.client, self.request).await })
    }
}

impl DoOwnedClientRequest {
    /// Request this message if there's enough immediately available space in the internal buffers
    ///
    /// This method will publish the given request only if there's enough
    /// immediately available space to enqueue it in the client's
    /// networking stack.
    ///
    /// # Errors
    ///
    /// It returns an error if the client's buffer is full or if the client has been closed.
    pub fn try_request(self) -> Result<ResponseFut, TryCommandError> {
        try_request(&self.client, self.request)
    }
}

impl IntoFuture for DoOwnedClientRequest {
    type Output = Result<ResponseFut, ClientClosedError>;
    type IntoFuture = BoxFuture<'static, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { request(&self.client, self.request).await })
    }
}

impl Future for ResponseFut {
    type Output = Result<ServerMessage, ResponseError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        match this.subscription {
            ResponseSubscription::Multiplexed(receiver) => match Pin::new(receiver).poll(cx) {
                Poll::Pending => match this.timeout.poll(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(()) => Poll::Ready(Err(ResponseError::TimedOut)),
                },
                Poll::Ready(Ok(message))
                    if message.status_code == Some(StatusCode::NO_RESPONDERS) =>
                {
                    Poll::Ready(Err(ResponseError::NoResponders))
                }
                Poll::Ready(Ok(message)) => Poll::Ready(Ok(message)),
                Poll::Ready(Err(_err)) => Poll::Ready(Err(ResponseError::SubscriptionClosed)),
            },
            ResponseSubscription::Subscription(subscription) => {
                match Pin::new(subscription).poll_next(cx) {
                    Poll::Pending => match this.timeout.poll(cx) {
                        Poll::Pending => Poll::Pending,
                        Poll::Ready(()) => Poll::Ready(Err(ResponseError::TimedOut)),
                    },
                    Poll::Ready(Some(Ok(message)))
                        if message.status_code == Some(StatusCode::NO_RESPONDERS) =>
                    {
                        Poll::Ready(Err(ResponseError::NoResponders))
                    }
                    Poll::Ready(Some(Ok(message))) => Poll::Ready(Ok(message)),
                    Poll::Ready(Some(Err(server_error))) => {
                        Poll::Ready(Err(ResponseError::ServerError(server_error)))
                    }
                    Poll::Ready(None) => Poll::Ready(Err(ResponseError::SubscriptionClosed)),
                }
            }
        }
    }
}

fn try_request(client: &Client, request: Request) -> Result<ResponseFut, TryCommandError> {
    let subscription = if let Some(reply_subject) = &request.publish.reply_subject {
        let subscription = client.try_subscribe(reply_subject.clone(), None)?;
        client.lazy_unsubscribe(subscription.id, Some(NonZeroU64::new(1).unwrap()));

        request.publish.client(client).try_publish()?;
        ResponseSubscription::Subscription(subscription)
    } else {
        let receiver = client.try_multiplexed_request(
            request.publish.subject,
            request.publish.headers,
            request.publish.payload,
        )?;
        ResponseSubscription::Multiplexed(receiver)
    };

    let timeout = sleep(
        request
            .response_timeout
            .unwrap_or(client.default_response_timeout()),
    );
    Ok(ResponseFut {
        subscription,
        timeout,
    })
}

async fn request(client: &Client, request: Request) -> Result<ResponseFut, ClientClosedError> {
    let subscription = if let Some(reply_subject) = &request.publish.reply_subject {
        let subscription = client.subscribe(reply_subject.clone(), None).await?;
        client.lazy_unsubscribe(subscription.id, Some(NonZeroU64::new(1).unwrap()));

        request.publish.client(client).await?;
        ResponseSubscription::Subscription(subscription)
    } else {
        let receiver = client
            .multiplexed_request(
                request.publish.subject,
                request.publish.headers,
                request.publish.payload,
            )
            .await?;
        ResponseSubscription::Multiplexed(receiver)
    };

    let timeout = sleep(
        request
            .response_timeout
            .unwrap_or(client.default_response_timeout()),
    );
    Ok(ResponseFut {
        subscription,
        timeout,
    })
}

impl Debug for ClientRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientRequest")
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}

impl Debug for DoClientRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DoClientRequest")
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}

impl Debug for OwnedClientRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedClientRequest")
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}

impl Debug for DoOwnedClientRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DoOwnedClientRequest")
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}
