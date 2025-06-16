#[cfg(test)]
use std::net::{IpAddr, Ipv4Addr};
use std::{fmt::Write, num::NonZero, process::abort, sync::Arc, time::Duration};

use arc_swap::ArcSwap;
use bytes::Bytes;
use tokio::{
    select,
    sync::{
        mpsc::{self, Permit, error::TrySendError},
        oneshot,
    },
    task::JoinHandle,
    time::sleep,
};
use watermelon_mini::ConnectError;
#[cfg(test)]
use watermelon_proto::NonStandardServerInfo;
use watermelon_proto::{
    QueueGroup, ServerAddr, ServerInfo, Subject, SubscriptionId, headers::HeaderMap,
};

pub use self::builder::{ClientBuilder, Echo};
pub use self::commands::{
    ClientPublish, ClientRequest, DoClientPublish, DoClientRequest, DoOwnedClientPublish,
    DoOwnedClientRequest, OwnedClientPublish, OwnedClientRequest, Publish, PublishBuilder, Request,
    RequestBuilder, ResponseError, ResponseFut,
};
pub use self::jetstream::{
    AckPolicy, Compression, Consumer, ConsumerBatch, ConsumerConfig, ConsumerDurability,
    ConsumerSpecificConfig, ConsumerStorage, ConsumerStream, ConsumerStreamError, Consumers,
    DeliverPolicy, DiscardPolicy, JetstreamClient, JetstreamError, JetstreamError2,
    JetstreamErrorCode, ReplayPolicy, RetentionPolicy, Storage, Stream, StreamConfig, StreamState,
    Streams,
};
pub use self::quick_info::QuickInfo;
pub(crate) use self::quick_info::RawQuickInfo;
#[cfg(test)]
use self::tests::TestHandler;
use crate::{
    core::{MultiplexedSubscription, Subscription},
    handler::{
        Handler, HandlerCommand, HandlerOutput, MULTIPLEXED_SUBSCRIPTION_ID, RecycledHandler,
    },
    util::atomic::{AtomicU64, Ordering},
};

mod builder;
mod commands;
mod jetstream;
mod quick_info;
#[cfg(test)]
pub(crate) mod tests;

#[cfg(feature = "from-env")]
pub(super) mod from_env;

const CLIENT_OP_CHANNEL_SIZE: usize = 512;
const SUBSCRIPTION_CHANNEL_SIZE: usize = 256;
const MIN_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(60);

/// A NATS client
///
/// `Client` is a `Clone`able handle to a NATS connection.
/// If the connection is lost, the client will automatically reconnect and
/// resume any currently open subscriptions.
///
/// Dropping all handles of `Client` will immediately kill the underlying
/// TCP connection to the server and lose all in flight publishes.
/// Use [`Client::close`] to gracefully shutdown the client.
#[derive(Debug, Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

#[derive(Debug)]
struct ClientInner {
    sender: mpsc::Sender<HandlerCommand>,
    info: Arc<ArcSwap<ServerInfo>>,
    quick_info: Arc<RawQuickInfo>,
    multiplexed_subscription_prefix: Subject,
    next_subscription_id: AtomicU64,
    inbox_prefix: Subject,
    default_response_timeout: Duration,
    handler: JoinHandle<()>,
    shutdown_sender: mpsc::Sender<()>,
}

/// An error encountered while trying to publish a command to a closed [`Client`]
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
#[error("client closed")]
pub struct ClientClosedError;

#[derive(Debug, thiserror::Error)]
#[error("try command error")]
pub enum TryCommandError {
    /// The client's internal buffer is currently full
    #[error("buffer full")]
    BufferFull,
    /// The client has been closed via [`Client::close`]
    #[error("client closed")]
    Closed(#[source] ClientClosedError),
}

impl Client {
    /// Construct a new client
    #[must_use]
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    pub(super) async fn connect(
        addr: ServerAddr,
        builder: ClientBuilder,
    ) -> Result<Self, ConnectError> {
        let (sender, receiver) = mpsc::channel(CLIENT_OP_CHANNEL_SIZE);

        let (shutdown_sender, shutdown_receiver) = mpsc::channel(1);

        let quick_info = Arc::new(RawQuickInfo::new());
        let handle = RecycledHandler::new(
            receiver,
            Arc::clone(&quick_info),
            &builder,
            shutdown_receiver,
        );
        let handle = Handler::connect(&addr, &builder, handle)
            .await
            .map_err(|(err, _recycle)| err)?
            .expect("shutdown while connecting");
        let info = Arc::clone(handle.info());
        let multiplexed_subscription_prefix = handle.multiplexed_subscription_prefix().clone();
        let inbox_prefix = builder.inbox_prefix.clone();
        let default_response_timeout = builder.default_response_timeout;

        let handler = tokio::spawn(async move {
            let mut handle = handle;

            #[expect(clippy::while_let_loop)]
            loop {
                match (&mut handle).await {
                    HandlerOutput::ServerError
                    | HandlerOutput::Disconnected
                    | HandlerOutput::UnexpectedState => {
                        let mut recycle = handle.recycle().await;

                        let mut delay = MIN_RECONNECT_DELAY;

                        loop {
                            select! {
                                biased;
                                () = recycle.wait_shutdown() => {
                                    return;
                                },
                                () = sleep(delay) => {},
                            }

                            match Handler::connect(&addr, &builder, recycle).await {
                                Ok(Some(new_handle)) => {
                                    handle = new_handle;
                                    break;
                                }
                                Ok(None) => {
                                    // shutdown
                                    return;
                                }
                                Err((_err, prev_recycle)) => {
                                    recycle = prev_recycle;
                                    delay *= 2;
                                    delay = delay.min(MAX_RECONNECT_DELAY);
                                }
                            }
                        }
                    }
                    HandlerOutput::Closed => break,
                }
            }
        });

        Ok(Self {
            inner: Arc::new(ClientInner {
                info,
                sender,
                quick_info,
                multiplexed_subscription_prefix,
                next_subscription_id: AtomicU64::new(u64::from(MULTIPLEXED_SUBSCRIPTION_ID) + 1),
                inbox_prefix,
                default_response_timeout,
                handler,
                shutdown_sender,
            }),
        })
    }

    #[cfg(test)]
    pub(crate) fn test(client_to_handler_chan_size: usize) -> (Self, TestHandler) {
        let builder = Self::builder();
        let (sender, receiver) = mpsc::channel(client_to_handler_chan_size);
        let (shutdown_sender, shutdown_receiver) = mpsc::channel(1);
        let info = Arc::new(ArcSwap::new(Arc::from(ServerInfo {
            id: "1234".to_owned(),
            name: "watermelon-test".to_owned(),
            version: "2.10.17".to_owned(),
            go_version: "1.22.5".to_owned(),
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: NonZero::new(4222).unwrap(),
            supports_headers: true,
            max_payload: NonZero::new(1024 * 1024).unwrap(),
            protocol_version: 2,
            client_id: Some(1),
            auth_required: false,
            tls_required: false,
            tls_verify: false,
            tls_available: false,
            connect_urls: Vec::new(),
            websocket_connect_urls: Vec::new(),
            lame_duck_mode: false,
            git_commit: None,
            supports_jetstream: true,
            ip: None,
            client_ip: None,
            nonce: None,
            cluster_name: None,
            domain: None,

            non_standard: NonStandardServerInfo::default(),
        })));
        let quick_info = Arc::new(RawQuickInfo::new());
        let multiplexed_subscription_prefix = create_inbox_subject(&builder.inbox_prefix);

        let this = Self {
            inner: Arc::new(ClientInner {
                sender,
                info: Arc::clone(&info),
                quick_info: Arc::clone(&quick_info),
                multiplexed_subscription_prefix,
                next_subscription_id: AtomicU64::new(1),
                inbox_prefix: builder.inbox_prefix,
                default_response_timeout: builder.default_response_timeout,
                handler: tokio::spawn(async move {}),
                shutdown_sender,
            }),
        };
        let handler = TestHandler {
            receiver,
            _info: info,
            quick_info,
        };
        (this, handler)
    }

    /// Publish a new message to the NATS server
    ///
    /// Consider calling [`Publish::client`] instead if you already have
    /// a [`Publish`] instance.
    #[must_use]
    pub fn publish(&self, subject: Subject) -> ClientPublish<'_> {
        ClientPublish::build(self, subject)
    }

    /// Publish a new message to the NATS server
    ///
    /// Consider calling [`Request::client`] instead if you already have
    /// a [`Request`] instance.
    #[must_use]
    pub fn request(&self, subject: Subject) -> ClientRequest<'_> {
        ClientRequest::build(self, subject)
    }

    /// Publish a new message to the NATS server, taking ownership of this client
    ///
    /// When possible consider using [`Client::publish`] instead.
    ///
    /// Consider calling [`Publish::client_owned`] instead if you already have
    /// a [`Publish`] instance.
    #[must_use]
    pub fn publish_owned(self, subject: Subject) -> OwnedClientPublish {
        OwnedClientPublish::build(self, subject)
    }

    /// Publish a new message to the NATS server, taking ownership of this client
    ///
    /// When possible consider using [`Client::request`] instead.
    ///
    /// Consider calling [`Request::client_owned`] instead if you already have
    /// a [`Request`] instance.
    #[must_use]
    pub fn request_owned(self, subject: Subject) -> OwnedClientRequest {
        OwnedClientRequest::build(self, subject)
    }

    /// Subscribe to the given filter subject
    ///
    /// Create a new subscription with the NATS server and ask for all
    /// messages matching the given `filter_subject` to be delivered
    /// to the client.
    ///
    /// If `queue_group` is provided and multiple clients subscribe with
    /// the same [`QueueGroup`] value, the NATS server will try to deliver
    /// these messages to only one of the clients.
    ///
    /// If the client was built with [`Echo::Allow`], then messages
    /// published by this same client may be received by this subscription.
    ///
    /// # Errors
    ///
    /// This returns an error if the connection with the client is closed.
    pub async fn subscribe(
        &self,
        filter_subject: Subject,
        queue_group: Option<QueueGroup>,
    ) -> Result<Subscription, ClientClosedError> {
        let permit = self
            .inner
            .sender
            .reserve()
            .await
            .map_err(|_| ClientClosedError)?;

        Ok(self.do_subscribe(permit, filter_subject, queue_group))
    }

    pub(crate) fn try_subscribe(
        &self,
        filter_subject: Subject,
        queue_group: Option<QueueGroup>,
    ) -> Result<Subscription, TryCommandError> {
        let permit = self
            .inner
            .sender
            .try_reserve()
            .map_err(|_| TryCommandError::BufferFull)?;

        Ok(self.do_subscribe(permit, filter_subject, queue_group))
    }

    fn do_subscribe(
        &self,
        permit: Permit<'_, HandlerCommand>,
        filter_subject: Subject,
        queue_group: Option<QueueGroup>,
    ) -> Subscription {
        let id = self
            .inner
            .next_subscription_id
            .fetch_add(1, Ordering::AcqRel)
            .into();
        if id == SubscriptionId::MAX {
            abort();
        }
        let (sender, receiver) = mpsc::channel(SUBSCRIPTION_CHANNEL_SIZE);

        permit.send(HandlerCommand::Subscribe {
            id,
            subject: filter_subject,
            queue_group,
            messages: sender,
        });
        Subscription::new(id, self.clone(), receiver)
    }

    pub(super) async fn multiplexed_request(
        &self,
        subject: Subject,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<MultiplexedSubscription, ClientClosedError> {
        let permit = self
            .inner
            .sender
            .reserve()
            .await
            .map_err(|_| ClientClosedError)?;

        Ok(self.do_multiplexed_request(permit, subject, headers, payload))
    }

    pub(super) fn try_multiplexed_request(
        &self,
        subject: Subject,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<MultiplexedSubscription, TryCommandError> {
        let permit = self
            .inner
            .sender
            .try_reserve()
            .map_err(|_| TryCommandError::BufferFull)?;

        Ok(self.do_multiplexed_request(permit, subject, headers, payload))
    }

    fn do_multiplexed_request(
        &self,
        permit: Permit<'_, HandlerCommand>,
        subject: Subject,
        headers: HeaderMap,
        payload: Bytes,
    ) -> MultiplexedSubscription {
        let (sender, receiver) = oneshot::channel();

        let reply_subject = create_inbox_subject(&self.inner.multiplexed_subscription_prefix);

        permit.send(HandlerCommand::RequestMultiplexed {
            subject,
            reply_subject: reply_subject.clone(),
            headers,
            payload,
            reply: sender,
        });
        MultiplexedSubscription::new(reply_subject, receiver, self.clone())
    }

    /// Get the last [`ServerInfo`] sent by the server
    ///
    /// Consider calling [`Client::quick_info`] if you only need
    /// information about Lame Duck Mode.
    #[must_use]
    pub fn server_info(&self) -> Arc<ServerInfo> {
        self.inner.info.load_full()
    }

    /// Get information about the client
    #[must_use]
    pub fn quick_info(&self) -> QuickInfo {
        self.inner.quick_info.get()
    }

    pub(crate) fn create_inbox_subject(&self) -> Subject {
        create_inbox_subject(&self.inner.inbox_prefix)
    }

    pub(crate) fn default_response_timeout(&self) -> Duration {
        self.inner.default_response_timeout
    }

    pub(crate) fn lazy_unsubscribe_multiplexed(&self, reply_subject: Subject) {
        if self
            .try_enqueue_command(HandlerCommand::UnsubscribeMultiplexed { reply_subject })
            .is_ok()
        {
            return;
        }

        self.inner.quick_info.store_is_failed_unsubscribe(true);
    }

    pub(crate) async fn unsubscribe(
        &self,
        id: SubscriptionId,
        max_messages: Option<NonZero<u64>>,
    ) -> Result<(), ClientClosedError> {
        self.enqueue_command(HandlerCommand::Unsubscribe { id, max_messages })
            .await
    }

    pub(crate) fn lazy_unsubscribe(&self, id: SubscriptionId, max_messages: Option<NonZero<u64>>) {
        if self
            .try_enqueue_command(HandlerCommand::Unsubscribe { id, max_messages })
            .is_ok()
        {
            return;
        }

        self.inner.quick_info.store_is_failed_unsubscribe(true);
    }

    pub(super) async fn enqueue_command(
        &self,
        cmd: HandlerCommand,
    ) -> Result<(), ClientClosedError> {
        self.inner
            .sender
            .send(cmd)
            .await
            .map_err(|_| ClientClosedError)
    }

    pub(super) fn try_enqueue_command(&self, cmd: HandlerCommand) -> Result<(), TryCommandError> {
        self.inner
            .sender
            .try_send(cmd)
            .map_err(TryCommandError::from_try_send_error)
    }

    /// Close this client, waiting for any remaining buffered messages to be processed first
    ///
    /// Attempts to send commands to the NATS server after this method has been called will
    /// result into a [`ClientClosedError`] error.
    pub async fn close(&self) {
        // If this fails to send, either another shutdown is already in flight or the client
        // has already been shutdown.
        let _ = self.inner.shutdown_sender.try_send(());

        self.inner.shutdown_sender.closed().await;
    }
}

impl Drop for ClientInner {
    fn drop(&mut self) {
        self.handler.abort();
    }
}

impl TryCommandError {
    #[expect(
        clippy::needless_pass_by_value,
        reason = "this is an auxiliary conversion function"
    )]
    pub(crate) fn from_try_send_error<T>(err: TrySendError<T>) -> Self {
        match err {
            TrySendError::Full(_) => Self::BufferFull,
            TrySendError::Closed(_) => Self::Closed(ClientClosedError),
        }
    }
}

pub(crate) fn create_inbox_subject(prefix: &Subject) -> Subject {
    let mut suffix = [0u8; 16];
    #[cfg(feature = "rand")]
    rand::fill(&mut suffix);
    #[cfg(all(not(feature = "rand"), feature = "getrandom"))]
    getrandom::fill(&mut suffix).expect("unable to generate random suffix");
    #[cfg(all(not(feature = "rand"), not(feature = "getrandom")))]
    compile_error!("Please enable the `rand` or the `getrandom` feature");

    let mut subject = String::with_capacity(prefix.len() + ".".len() + (suffix.len() * 2));
    write!(&mut subject, "{}.{:x}", prefix, u128::from_ne_bytes(suffix)).unwrap();

    Subject::from_dangerous_value(subject.into())
}
