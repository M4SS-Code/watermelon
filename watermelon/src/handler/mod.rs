use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    mem,
    num::NonZero,
    ops::ControlFlow,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use arc_swap::ArcSwap;
use bytes::Bytes;
use thiserror::Error;
use tokio::{
    net::TcpStream,
    select,
    sync::{
        mpsc::{self, error::TrySendError},
        oneshot,
    },
    task::coop,
    time::timeout,
};
use watermelon_mini::{
    ConnectError, ConnectFlags, ConnectionCompression, ConnectionSecurity, easy_connect,
};
use watermelon_net::Connection;
use watermelon_proto::{
    MessageBase, QueueGroup, ServerAddr, ServerInfo, ServerMessage, Subject, SubscriptionId,
    error::ServerError,
    headers::HeaderMap,
    proto::{ClientOp, ServerOp},
};

use self::{delayed::Delayed, pinger::Pinger};
use crate::core::{ClientBuilder, Echo};
use crate::{
    client::{QuickInfo, RawQuickInfo, create_inbox_subject},
    handler::pinger::PingOutcome,
};

mod delayed;
mod pinger;

pub(crate) const MULTIPLEXED_SUBSCRIPTION_ID: SubscriptionId = SubscriptionId::MIN;
const RECV_BUF: usize = 16;

#[derive(Debug)]
pub(crate) struct Handler {
    conn: Connection<
        ConnectionCompression<ConnectionSecurity<TcpStream>>,
        ConnectionSecurity<TcpStream>,
    >,
    info: Arc<ArcSwap<ServerInfo>>,
    quick_info: Arc<RawQuickInfo>,
    delayed_write: Delayed,
    writing: bool,
    flushing: bool,
    shutting_down: bool,

    pinger: Pinger,

    commands: mpsc::Receiver<HandlerCommand>,
    recv_buf: Vec<HandlerCommand>,
    in_flight_commands: VecDeque<InFlightCommand>,

    multiplexed_subscription_prefix: Subject,
    multiplexed_subscriptions: Option<BTreeMap<Subject, oneshot::Sender<ServerMessage>>>,
    subscriptions: BTreeMap<SubscriptionId, Subscription>,

    shutdown_recv: mpsc::Receiver<()>,
}

#[derive(Debug)]
pub(crate) struct RecycledHandler {
    commands: mpsc::Receiver<HandlerCommand>,
    quick_info: Arc<RawQuickInfo>,

    multiplexed_subscription_prefix: Subject,
    subscriptions: BTreeMap<SubscriptionId, Subscription>,

    shutdown_recv: mpsc::Receiver<()>,
}

#[derive(Debug, Error)]
pub enum ConnectHandlerError {
    #[error("connect error")]
    Connect(#[source] ConnectError),
    #[error("timed out while attempting to connect")]
    TimedOut,
}

#[derive(Debug)]
struct Subscription {
    subject: Subject,
    queue_group: Option<QueueGroup>,
    messages: mpsc::Sender<Result<ServerMessage, ServerError>>,
    remaining: Option<NonZero<u64>>,
    failed_subscribe: bool,
}

#[derive(Debug)]
pub(crate) enum HandlerCommand {
    Publish {
        message: MessageBase,
    },
    RequestMultiplexed {
        subject: Subject,
        reply_subject: Subject,
        headers: HeaderMap,
        payload: Bytes,
        reply: oneshot::Sender<ServerMessage>,
    },
    UnsubscribeMultiplexed {
        reply_subject: Subject,
    },
    Subscribe {
        id: SubscriptionId,
        subject: Subject,
        queue_group: Option<QueueGroup>,
        messages: mpsc::Sender<Result<ServerMessage, ServerError>>,
    },
    Unsubscribe {
        id: SubscriptionId,
        max_messages: Option<NonZero<u64>>,
    },
}

#[derive(Debug)]
pub(crate) enum InFlightCommand {
    Unimportant,
    Subscribe { id: SubscriptionId },
}

#[derive(Debug)]
pub(crate) enum HandlerOutput {
    ServerError,
    UnexpectedState,
    Disconnected,
    Closed,
}

impl Handler {
    pub(crate) async fn connect(
        addr: &ServerAddr,
        builder: &ClientBuilder,
        mut recycle: RecycledHandler,
    ) -> Result<Option<Self>, (ConnectHandlerError, RecycledHandler)> {
        let mut flags = ConnectFlags::default();
        flags.echo = matches!(builder.echo, Echo::Allow);
        #[cfg(feature = "non-standard-zstd")]
        {
            flags.zstd_compression_level = builder.non_standard_zstd_compression_level;
        }

        let (mut conn, info) = select! {
            biased;
            () = recycle.wait_shutdown() => {
                return Ok(None);
            },
            connect_result = timeout(builder.connect_timeout, easy_connect(addr, builder.auth_method.as_ref(), flags)) => {
                match connect_result {
                    Ok(Ok(items)) => items,
                    Ok(Err(err)) => return Err((ConnectHandlerError::Connect(err), recycle)),
                    Err(_elapsed) => return Err((ConnectHandlerError::TimedOut, recycle)),
                }
            }
        };

        #[cfg(feature = "non-standard-zstd")]
        let is_zstd_compressed = if let Connection::Streaming(streaming) = &conn {
            streaming.socket().is_zstd_compressed()
        } else {
            false
        };
        recycle.quick_info.store(|quick_info| QuickInfo {
            is_connected: true,
            #[cfg(feature = "non-standard-zstd")]
            is_zstd_compressed,
            is_lameduck: false,
            ..quick_info
        });

        let mut in_flight_commands = VecDeque::new();
        for (&id, subscription) in &recycle.subscriptions {
            in_flight_commands.push_back(InFlightCommand::Subscribe { id });
            conn.enqueue_write_op(&ClientOp::Subscribe {
                id,
                subject: subscription.subject.clone(),
                queue_group: subscription.queue_group.clone(),
            });

            if let Some(remaining) = subscription.remaining {
                in_flight_commands.push_back(InFlightCommand::Unimportant);
                conn.enqueue_write_op(&ClientOp::Unsubscribe {
                    id,
                    max_messages: Some(remaining),
                });
            }
        }

        let delayed_write = Delayed::new(builder.write_delay);

        Ok(Some(Self {
            conn,
            info: Arc::new(ArcSwap::new(Arc::from(info))),
            quick_info: recycle.quick_info,
            delayed_write,
            writing: false,
            flushing: false,
            shutting_down: false,
            pinger: Pinger::new(),
            commands: recycle.commands,
            recv_buf: Vec::with_capacity(RECV_BUF),
            in_flight_commands,
            subscriptions: recycle.subscriptions,
            multiplexed_subscription_prefix: recycle.multiplexed_subscription_prefix,
            multiplexed_subscriptions: None,
            shutdown_recv: recycle.shutdown_recv,
        }))
    }

    pub(crate) async fn recycle(mut self) -> RecycledHandler {
        self.quick_info.store_is_connected(false);
        let _ = self.conn.shutdown().await;

        RecycledHandler {
            commands: self.commands,
            quick_info: self.quick_info,
            subscriptions: self.subscriptions,
            multiplexed_subscription_prefix: self.multiplexed_subscription_prefix,
            shutdown_recv: self.shutdown_recv,
        }
    }

    pub(crate) fn info(&self) -> &Arc<ArcSwap<ServerInfo>> {
        &self.info
    }

    pub(crate) fn multiplexed_subscription_prefix(&self) -> &Subject {
        &self.multiplexed_subscription_prefix
    }

    fn handle_server_op(&mut self, server_op: ServerOp) -> ControlFlow<HandlerOutput, ()> {
        match server_op {
            ServerOp::Message { message }
                if message.subscription_id == MULTIPLEXED_SUBSCRIPTION_ID =>
            {
                let Some(multiplexed_subscriptions) = &mut self.multiplexed_subscriptions else {
                    return ControlFlow::Continue(());
                };

                if let Some(sender) = multiplexed_subscriptions.remove(&message.base.subject) {
                    let _ = sender.send(message);
                } else {
                    // 🤷
                }
            }
            ServerOp::Message { message } => {
                let subscription_id = message.subscription_id;

                if let Some(subscription) = self.subscriptions.get_mut(&subscription_id) {
                    match subscription.messages.try_send(Ok(message)) {
                        Ok(()) => {}
                        #[expect(
                            clippy::match_same_arms,
                            reason = "the case still needs to be implemented"
                        )]
                        Err(TrySendError::Full(_)) => {
                            // TODO
                        }
                        Err(TrySendError::Closed(_)) => {
                            self.in_flight_commands
                                .push_back(InFlightCommand::Unimportant);
                            self.conn.enqueue_write_op(&ClientOp::Unsubscribe {
                                id: subscription_id,
                                max_messages: None,
                            });
                            return ControlFlow::Continue(());
                        }
                    }

                    if let Some(remaining) = &mut subscription.remaining {
                        match NonZero::new(remaining.get() - 1) {
                            Some(new_remaining) => *remaining = new_remaining,
                            None => {
                                self.subscriptions.remove(&subscription_id);
                            }
                        }
                    }
                } else {
                    // 🤷
                }
            }
            ServerOp::Success => {
                let Some(in_flight_command) = self.in_flight_commands.pop_front() else {
                    return ControlFlow::Break(HandlerOutput::UnexpectedState);
                };

                match in_flight_command {
                    InFlightCommand::Unimportant | InFlightCommand::Subscribe { .. } => {
                        // Nothing to do
                    }
                }
            }
            ServerOp::Error { error } if error.is_fatal() == Some(false) => {
                let Some(in_flight_command) = self.in_flight_commands.pop_front() else {
                    return ControlFlow::Break(HandlerOutput::UnexpectedState);
                };

                match in_flight_command {
                    InFlightCommand::Unimportant => {
                        // Nothing to do
                    }
                    InFlightCommand::Subscribe { id } => {
                        if let Some(mut subscription) = self.subscriptions.remove(&id) {
                            match subscription.messages.try_send(Err(error)) {
                                Ok(()) | Err(TrySendError::Closed(_)) => {
                                    // Nothing to do
                                }
                                Err(TrySendError::Full(_)) => {
                                    // The error is going to be lost

                                    // We have to put the subscription back in order for the unsubscribe to be handled correctly
                                    subscription.failed_subscribe = true;
                                    self.subscriptions.insert(id, subscription);
                                    self.quick_info.store_is_failed_unsubscribe(true);
                                }
                            }
                        }
                    }
                }
            }
            ServerOp::Error { error: _ } => return ControlFlow::Break(HandlerOutput::ServerError),
            ServerOp::Ping => {
                self.conn.enqueue_write_op(&ClientOp::Pong);
            }
            ServerOp::Pong => {
                self.pinger.handle_pong();
            }
            ServerOp::Info { info } => {
                self.quick_info.store_is_lameduck(info.lame_duck_mode);
                self.info.store(Arc::from(info));
            }
        }

        ControlFlow::Continue(())
    }

    #[cold]
    fn failed_unsubscribe(&mut self) {
        #[derive(Debug, Copy, Clone)]
        enum CloseReason {
            FailedSubscribe,
            Dropped,
        }

        self.quick_info.store_is_failed_unsubscribe(false);

        if let Some(multiplexed_subscriptions) = &mut self.multiplexed_subscriptions {
            multiplexed_subscriptions.retain(|_subject, sender| !sender.is_closed());
        }

        let closed_subscription_ids = self
            .subscriptions
            .iter()
            .filter_map(|(&id, subscription)| {
                if subscription.failed_subscribe {
                    Some((id, CloseReason::FailedSubscribe))
                } else if subscription.messages.is_closed() {
                    Some((id, CloseReason::Dropped))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        for (closed_subscription_id, reason) in closed_subscription_ids {
            if matches!(reason, CloseReason::Dropped) {
                self.in_flight_commands
                    .push_back(InFlightCommand::Unimportant);
                self.conn.enqueue_write_op(&ClientOp::Unsubscribe {
                    id: closed_subscription_id,
                    max_messages: None,
                });
            }

            self.subscriptions.remove(&closed_subscription_id);
        }
    }

    #[cold]
    fn begin_shutdown(&mut self) {
        if self.shutting_down {
            return;
        }

        self.shutting_down = true;
        self.commands.close();
    }
}

impl Future for Handler {
    type Output = HandlerOutput;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let ping_outcome = this.pinger.poll(cx, || {
            this.conn.enqueue_write_op(&ClientOp::Ping);
        });
        match ping_outcome {
            PingOutcome::Ok => {}
            PingOutcome::TooManyInFlightPings => {
                return Poll::Ready(HandlerOutput::Disconnected);
            }
        }

        if !this.shutting_down {
            if this.quick_info.get().is_failed_unsubscribe {
                this.failed_unsubscribe();
            }

            if this.shutdown_recv.poll_recv(cx).is_ready() {
                this.begin_shutdown();
            }
        }

        let mut handled_server_op = false;
        loop {
            match this.conn.poll_read_next(cx) {
                Poll::Pending => break,
                Poll::Ready(Ok(server_op)) => {
                    let _ = this.handle_server_op(server_op);
                    handled_server_op = true;
                }
                Poll::Ready(Err(_err)) => return Poll::Ready(HandlerOutput::Disconnected),
            }
        }
        if handled_server_op {
            this.pinger.reset();
        }

        let mut receive_outcome = ReceiveOutcome::NoMoreCommands;
        let mut iterate_again = true;
        while mem::take(&mut iterate_again) {
            receive_outcome = this.receive_command(cx);
            if matches!(receive_outcome, ReceiveOutcome::NoMoreSpace) {
                // We reached the soft limit on the write buffer so
                // immediately start the write process.
                this.writing = true;
            }

            match &mut this.conn {
                Connection::Streaming(streaming) => {
                    if streaming.may_write() {
                        if !this.writing
                            && (this.flushing || this.delayed_write.poll_can_proceed(cx).is_ready())
                        {
                            // We weren't writing, but we were either flushing (so going
                            // back to writing is kind of free) or we're allowed to start
                            // writing again anyways.
                            this.writing = true;
                        }

                        if this.writing && coop::has_budget_remaining() {
                            match streaming.poll_write_next(cx) {
                                Poll::Pending if coop::has_budget_remaining() => {
                                    // There's no point in flushing. The underlying
                                    // `AsyncWrite` implementation already flushes
                                    // when full.
                                    this.flushing = false;
                                }
                                Poll::Pending => {
                                    // The tokio coop scheduler injected this `Poll::Pending`,
                                    // so it'd be wrong to interpret this as the writer
                                    // being blocked and stopping flushes from happening.
                                }
                                Poll::Ready(Ok(_n)) => {
                                    // Iterate again to register the waker and to try encoding
                                    // and writing more stuff.
                                    iterate_again = true;
                                }
                                Poll::Ready(Err(_err)) => {
                                    return Poll::Ready(HandlerOutput::Disconnected);
                                }
                            }
                        }
                    } else if this.writing {
                        // We must have been in a writing state, but now there's
                        // nothing left to write. Start flushing.
                        this.writing = false;
                        this.flushing = true;
                    }
                }
                #[cfg(feature = "websocket")]
                Connection::Websocket(websocket) => {
                    if websocket.should_flush()
                        && !this.flushing
                        && this.delayed_write.poll_can_proceed(cx).is_ready()
                    {
                        this.flushing = true;
                    }
                }
                #[cfg(not(feature = "websocket"))]
                Connection::Websocket(_) => {
                    debug_assert!(false, "should never be constructed");
                }
            }
        }

        if this.flushing {
            match this.conn.poll_flush(cx) {
                Poll::Pending => {}
                Poll::Ready(Ok(())) => this.flushing = false,
                Poll::Ready(Err(_err)) => return Poll::Ready(HandlerOutput::Disconnected),
            }
        }

        if this.shutting_down
            && matches!(receive_outcome, ReceiveOutcome::NoMoreCommands)
            && !this.writing
            && !this.flushing
        {
            Poll::Ready(HandlerOutput::Closed)
        } else {
            Poll::Pending
        }
    }
}

#[derive(Debug, Copy, Clone)]
enum ReceiveOutcome {
    NoMoreCommands,
    NoMoreSpace,
}

impl Handler {
    // TODO: refactor this, a view into Handler is needed in order to split `recv_buf` from the
    // rest.
    #[expect(
        clippy::too_many_lines,
        reason = "not good, but a non trivial refactor is needed"
    )]
    fn receive_command(&mut self, cx: &mut Context<'_>) -> ReceiveOutcome {
        while self.conn.may_enqueue_more_ops() {
            debug_assert!(self.recv_buf.is_empty());

            match self
                .commands
                .poll_recv_many(cx, &mut self.recv_buf, RECV_BUF)
            {
                Poll::Pending => return ReceiveOutcome::NoMoreCommands,
                Poll::Ready(1..) => {
                    for cmd in self.recv_buf.drain(..) {
                        match cmd {
                            HandlerCommand::Publish { message } => {
                                self.in_flight_commands
                                    .push_back(InFlightCommand::Unimportant);
                                self.conn.enqueue_write_op(&ClientOp::Publish { message });
                            }
                            HandlerCommand::RequestMultiplexed {
                                subject,
                                reply_subject,
                                headers,
                                payload,
                                reply,
                            } => {
                                debug_assert!(
                                    reply_subject
                                        .starts_with(&*self.multiplexed_subscription_prefix)
                                );

                                let multiplexed_subscriptions =
                                    if let Some(multiplexed_subscriptions) =
                                        &mut self.multiplexed_subscriptions
                                    {
                                        multiplexed_subscriptions
                                    } else {
                                        init_multiplexed_subscriptions(
                                            &mut self.in_flight_commands,
                                            &mut self.conn,
                                            &self.multiplexed_subscription_prefix,
                                            &mut self.multiplexed_subscriptions,
                                        )
                                    };

                                self.in_flight_commands
                                    .push_back(InFlightCommand::Unimportant);
                                multiplexed_subscriptions.insert(reply_subject.clone(), reply);

                                let message = MessageBase {
                                    subject,
                                    reply_subject: Some(reply_subject),
                                    headers,
                                    payload,
                                };
                                self.conn.enqueue_write_op(&ClientOp::Publish { message });
                            }
                            HandlerCommand::UnsubscribeMultiplexed { reply_subject } => {
                                debug_assert!(
                                    reply_subject
                                        .starts_with(&*self.multiplexed_subscription_prefix)
                                );

                                if let Some(multiplexed_subscriptions) =
                                    &mut self.multiplexed_subscriptions
                                {
                                    let _ = multiplexed_subscriptions.remove(&reply_subject);
                                }
                            }
                            HandlerCommand::Subscribe {
                                id,
                                subject,
                                queue_group,
                                messages,
                            } => {
                                self.subscriptions.insert(
                                    id,
                                    Subscription {
                                        subject: subject.clone(),
                                        queue_group: queue_group.clone(),
                                        messages,
                                        remaining: None,
                                        failed_subscribe: false,
                                    },
                                );
                                self.in_flight_commands
                                    .push_back(InFlightCommand::Subscribe { id });
                                self.conn.enqueue_write_op(&ClientOp::Subscribe {
                                    id,
                                    subject,
                                    queue_group,
                                });
                            }
                            HandlerCommand::Unsubscribe {
                                id,
                                max_messages: Some(max_messages),
                            } => {
                                if let Some(subscription) = self.subscriptions.get_mut(&id) {
                                    subscription.remaining = Some(max_messages);
                                    self.in_flight_commands
                                        .push_back(InFlightCommand::Unimportant);
                                    self.conn.enqueue_write_op(&ClientOp::Unsubscribe {
                                        id,
                                        max_messages: Some(max_messages),
                                    });
                                }
                            }
                            HandlerCommand::Unsubscribe {
                                id,
                                max_messages: None,
                            } => {
                                if self.subscriptions.remove(&id).is_some() {
                                    self.in_flight_commands
                                        .push_back(InFlightCommand::Unimportant);
                                    self.conn.enqueue_write_op(&ClientOp::Unsubscribe {
                                        id,
                                        max_messages: None,
                                    });
                                }
                            }
                        }
                    }
                }
                Poll::Ready(0) => self.shutting_down = true,
            }
        }

        ReceiveOutcome::NoMoreSpace
    }
}

impl RecycledHandler {
    pub(crate) fn new(
        commands: mpsc::Receiver<HandlerCommand>,
        quick_info: Arc<RawQuickInfo>,
        builder: &ClientBuilder,
        shutdown_recv: mpsc::Receiver<()>,
    ) -> Self {
        Self {
            commands,
            quick_info,
            subscriptions: BTreeMap::new(),
            multiplexed_subscription_prefix: create_inbox_subject(&builder.inbox_prefix),
            shutdown_recv,
        }
    }

    pub(crate) async fn wait_shutdown(&mut self) {
        let _ = self.shutdown_recv.recv().await;
    }
}

#[cold]
fn init_multiplexed_subscriptions<'a>(
    in_flight_commands: &mut VecDeque<InFlightCommand>,
    conn: &mut Connection<
        ConnectionCompression<ConnectionSecurity<TcpStream>>,
        ConnectionSecurity<TcpStream>,
    >,
    multiplexed_subscription_prefix: &Subject,
    multiplexed_subscriptions: &'a mut Option<BTreeMap<Subject, oneshot::Sender<ServerMessage>>>,
) -> &'a mut BTreeMap<Subject, oneshot::Sender<ServerMessage>> {
    in_flight_commands.push_back(InFlightCommand::Subscribe {
        id: MULTIPLEXED_SUBSCRIPTION_ID,
    });
    conn.enqueue_write_op(&ClientOp::Subscribe {
        id: MULTIPLEXED_SUBSCRIPTION_ID,
        subject: Subject::from_dangerous_value(
            format!("{multiplexed_subscription_prefix}.*").into(),
        ),
        queue_group: None,
    });

    multiplexed_subscriptions.insert(BTreeMap::new())
}
