use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    num::NonZeroU64,
    ops::ControlFlow,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use arc_swap::ArcSwap;
use bytes::Bytes;
use tokio::{
    net::TcpStream,
    sync::{
        mpsc::{self, error::TrySendError},
        oneshot,
    },
    time::{self, Instant, Sleep},
};
use watermelon_mini::{
    easy_connect, ConnectError, ConnectFlags, ConnectionCompression, ConnectionSecurity,
};
use watermelon_net::Connection;
use watermelon_proto::{
    error::ServerError,
    headers::HeaderMap,
    proto::{ClientOp, ServerOp},
    MessageBase, QueueGroup, ServerAddr, ServerInfo, ServerMessage, Subject, SubscriptionId,
};

use crate::client::{create_inbox_subject, QuickInfo, RawQuickInfo};
use crate::core::{ClientBuilder, Echo};

pub(crate) const MULTIPLEXED_SUBSCRIPTION_ID: SubscriptionId = SubscriptionId::MIN;
const PING_INTERVAL: Duration = Duration::from_secs(10);
const RECV_BUF: usize = 16;

#[derive(Debug)]
pub(crate) struct Handler {
    conn: Connection<
        ConnectionCompression<ConnectionSecurity<TcpStream>>,
        ConnectionSecurity<TcpStream>,
    >,
    info: Arc<ArcSwap<ServerInfo>>,
    quick_info: Arc<RawQuickInfo>,
    delayed_flusher: Option<DelayedFlusher>,
    flushing: bool,
    shutting_down: bool,

    ping_interval: Pin<Box<Sleep>>,
    pending_pings: u8,

    commands: mpsc::Receiver<HandlerCommand>,
    recv_buf: Vec<HandlerCommand>,
    in_flight_commands: VecDeque<InFlightCommand>,

    multiplexed_subscription_prefix: Subject,
    multiplexed_subscriptions: Option<BTreeMap<Subject, oneshot::Sender<ServerMessage>>>,
    subscriptions: BTreeMap<SubscriptionId, Subscription>,

    awaiting_close: Vec<oneshot::Sender<()>>,
}

#[derive(Debug)]
struct DelayedFlusher {
    // INVARIANT: `interval != Duration::ZERO`
    interval: Duration,
    delay: Pin<Box<Option<Sleep>>>,
}

#[derive(Debug)]
pub(crate) struct RecycledHandler {
    commands: mpsc::Receiver<HandlerCommand>,
    quick_info: Arc<RawQuickInfo>,

    multiplexed_subscription_prefix: Subject,
    subscriptions: BTreeMap<SubscriptionId, Subscription>,

    awaiting_close: Vec<oneshot::Sender<()>>,
}

#[derive(Debug)]
struct Subscription {
    subject: Subject,
    queue_group: Option<QueueGroup>,
    messages: mpsc::Sender<Result<ServerMessage, ServerError>>,
    remaining: Option<NonZeroU64>,
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
        max_messages: Option<NonZeroU64>,
    },
    Close(oneshot::Sender<()>),
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
        recycle: RecycledHandler,
    ) -> Result<Self, (ConnectError, RecycledHandler)> {
        let mut flags = ConnectFlags::default();
        flags.echo = matches!(builder.echo, Echo::Allow);
        #[cfg(feature = "non-standard-zstd")]
        {
            flags.zstd = builder.non_standard_zstd;
        }

        let (mut conn, info) = match easy_connect(addr, builder.auth_method.as_ref(), flags).await {
            Ok(items) => items,
            Err(err) => return Err((err, recycle)),
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

        let delayed_flusher = if builder.flush_interval.is_zero() {
            None
        } else {
            Some(DelayedFlusher {
                interval: builder.flush_interval,
                delay: Box::pin(None),
            })
        };

        Ok(Self {
            conn,
            info: Arc::new(ArcSwap::new(Arc::from(info))),
            quick_info: recycle.quick_info,
            delayed_flusher,
            flushing: false,
            shutting_down: false,
            ping_interval: Box::pin(time::sleep(PING_INTERVAL)),
            pending_pings: 0,
            commands: recycle.commands,
            recv_buf: Vec::with_capacity(RECV_BUF),
            in_flight_commands,
            subscriptions: recycle.subscriptions,
            multiplexed_subscription_prefix: recycle.multiplexed_subscription_prefix,
            multiplexed_subscriptions: None,
            awaiting_close: recycle.awaiting_close,
        })
    }

    pub(crate) async fn recycle(mut self) -> RecycledHandler {
        self.quick_info.store_is_connected(false);
        let _ = self.conn.shutdown().await;

        RecycledHandler {
            commands: self.commands,
            quick_info: self.quick_info,
            subscriptions: self.subscriptions,
            multiplexed_subscription_prefix: self.multiplexed_subscription_prefix,
            awaiting_close: self.awaiting_close,
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
                        match NonZeroU64::new(remaining.get() - 1) {
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
                self.pending_pings = self.pending_pings.saturating_sub(1);
            }
            ServerOp::Info { info } => {
                self.quick_info.store_is_lameduck(info.lame_duck_mode);
                self.info.store(Arc::from(info));
            }
        }

        ControlFlow::Continue(())
    }

    #[cold]
    fn ping(&mut self, cx: &mut Context<'_>) -> Result<(), HandlerOutput> {
        if self.pending_pings < 2 {
            loop {
                self.reset_ping_interval();
                if Pin::new(&mut self.ping_interval).poll(cx).is_pending() {
                    break;
                }
            }

            self.conn.enqueue_write_op(&ClientOp::Ping);
            self.pending_pings += 1;
            Ok(())
        } else {
            Err(HandlerOutput::Disconnected)
        }
    }

    #[cold]
    fn failed_unsubscribe(&mut self) {
        self.quick_info.store_is_failed_unsubscribe(false);

        if let Some(multiplexed_subscriptions) = &mut self.multiplexed_subscriptions {
            multiplexed_subscriptions.retain(|_subject, sender| !sender.is_closed());
        }

        let closed_subscription_ids = self
            .subscriptions
            .iter()
            .filter(|(_id, subscription)| {
                subscription.failed_subscribe || subscription.messages.is_closed()
            })
            .map(|(&id, _subscription)| id)
            .collect::<Vec<_>>();

        for closed_subscription_id in closed_subscription_ids {
            self.in_flight_commands
                .push_back(InFlightCommand::Unimportant);
            self.conn.enqueue_write_op(&ClientOp::Unsubscribe {
                id: closed_subscription_id,
                max_messages: None,
            });
            self.subscriptions.remove(&closed_subscription_id);
        }
    }

    fn reset_ping_interval(&mut self) {
        Sleep::reset(self.ping_interval.as_mut(), Instant::now() + PING_INTERVAL);
    }
}

impl Future for Handler {
    type Output = HandlerOutput;

    #[expect(clippy::too_many_lines)]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        #[derive(Debug, Copy, Clone)]
        enum FlushAction {
            Start,
            Stop,
        }

        let this = self.get_mut();
        if Pin::new(&mut this.ping_interval).poll(cx).is_ready() {
            if let Err(output) = this.ping(cx) {
                return Poll::Ready(output);
            }
        }

        if this.quick_info.get().is_failed_unsubscribe {
            this.failed_unsubscribe();
        }

        let mut handled_server_op = false;
        loop {
            match this.conn.poll_read_next(cx) {
                Poll::Pending => break,
                Poll::Ready(Ok(server_op)) => {
                    this.handle_server_op(server_op);
                    handled_server_op = true;
                }
                Poll::Ready(Err(_err)) => return Poll::Ready(HandlerOutput::Disconnected),
            }
        }
        if handled_server_op {
            this.reset_ping_interval();
        }

        loop {
            let receive_outcome = this.receive_command(cx);
            let write_waker_registered = match &mut this.conn {
                Connection::Streaming(streaming) => {
                    if streaming.may_write() {
                        match streaming.poll_write_next(cx) {
                            Poll::Pending => true,
                            Poll::Ready(Ok(_n)) => false,
                            Poll::Ready(Err(_err)) => {
                                return Poll::Ready(HandlerOutput::Disconnected);
                            }
                        }
                    } else {
                        true
                    }
                }
                Connection::Websocket(_) => true,
            };

            let flushes_automatically_when_full = this.conn.flushes_automatically_when_full();
            let should_flush = this.conn.should_flush();

            let flush_action = match (
                receive_outcome,
                flushes_automatically_when_full,
                should_flush,
            ) {
                (ReceiveOutcome::NoMoreCommands, _, true) => {
                    // We have written everything there was to write,
                    // and some data is buffered
                    FlushAction::Start
                }
                (ReceiveOutcome::NoMoreSpace, true, should_flush) => {
                    debug_assert!(should_flush, "the connection is out space for writing but doesn't report the need to flush");

                    // There's no more space to write, but the implementation automatically
                    // flushes so we're good
                    FlushAction::Stop
                }
                (ReceiveOutcome::NoMoreSpace, false, true) => {
                    // There's no more space to write, and the implementation doesn't
                    // flush automatically
                    FlushAction::Start
                }
                (_, _, false) => {
                    // There's nothing to flush
                    FlushAction::Stop
                }
            };

            match flush_action {
                FlushAction::Start => {
                    this.flushing = true;
                    if let Some(delayed_flusher) = &mut this.delayed_flusher {
                        if delayed_flusher.delay.is_none() {
                            delayed_flusher
                                .delay
                                .set(Some(time::sleep(delayed_flusher.interval)));
                        }
                    }
                }
                FlushAction::Stop => {
                    this.flushing = false;
                }
            }

            match (receive_outcome, write_waker_registered) {
                (ReceiveOutcome::NoMoreCommands, true) => {
                    // There are no more commands to receive and writing is blocked.
                    // There's no progress to be made
                    break;
                }
                (ReceiveOutcome::NoMoreSpace, true) => {
                    // There's no more space to write and writing is blocked.
                    // There's no progress to be made
                    break;
                }
                (_, false) => {
                    // At least the write waker must be registered
                    continue;
                }
            }
        }

        if this.flushing {
            let mut can_flush = true;
            if let Some(delay_flusher) = &mut this.delayed_flusher {
                if let Some(delay) = delay_flusher.delay.as_mut().as_pin_mut() {
                    if delay.poll(cx).is_ready() {
                        delay_flusher.delay.set(None);
                    } else {
                        can_flush = false;
                    }
                }
            }

            if can_flush {
                match this.conn.poll_flush(cx) {
                    Poll::Pending => {}
                    Poll::Ready(Ok(())) => this.flushing = false,
                    Poll::Ready(Err(_err)) => return Poll::Ready(HandlerOutput::Disconnected),
                }
            }
        }

        if this.shutting_down {
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
                                debug_assert!(reply_subject
                                    .starts_with(&*self.multiplexed_subscription_prefix));

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
                                debug_assert!(reply_subject
                                    .starts_with(&*self.multiplexed_subscription_prefix));

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
                            HandlerCommand::Close(sender) => {
                                self.shutting_down = true;
                                self.awaiting_close.push(sender);
                                self.commands.close();
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
    ) -> Self {
        Self {
            commands,
            quick_info,
            subscriptions: BTreeMap::new(),
            multiplexed_subscription_prefix: create_inbox_subject(&builder.inbox_prefix),
            awaiting_close: Vec::new(),
        }
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
