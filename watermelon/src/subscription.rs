use std::{
    num::NonZero,
    pin::Pin,
    task::{Context, Poll},
};

use futures_core::{FusedStream, Stream};
use tokio::sync::mpsc;
use watermelon_proto::{ServerMessage, SubscriptionId, error::ServerError};

use crate::core::{Client, error::ClientClosedError};

const BATCH_RECEIVE_SIZE: usize = 16;

/// A NATS subscription
///
/// Receives messages coming from the NATS server with At Most Once Delivery.
///
/// Messages are yielded via the [`Stream`] implementation as they are received by client.
/// Errors can only occur immediately after subscribing or after the client reconnects.
///
/// The subscription MUST be polled continuously. If the subscription is not polled
/// for a relatively long period of time the internal buffers will fill up and any
/// further messages will be dropped.
///
/// Obtained from [`Client::subscribe`].
#[derive(Debug)]
pub struct Subscription {
    pub(crate) id: SubscriptionId,
    client: Client,
    receiver: mpsc::Receiver<Result<ServerMessage, ServerError>>,
    receiver_queue: Vec<Result<ServerMessage, ServerError>>,
    status: SubscriptionStatus,
}

#[derive(Debug, Copy, Clone)]
enum SubscriptionStatus {
    Subscribed,
    Unsubscribed,
}

impl Subscription {
    pub(crate) fn new(
        id: SubscriptionId,
        client: Client,
        receiver: mpsc::Receiver<Result<ServerMessage, ServerError>>,
    ) -> Self {
        Self {
            id,
            client,
            receiver,
            receiver_queue: Vec::with_capacity(BATCH_RECEIVE_SIZE),
            status: SubscriptionStatus::Subscribed,
        }
    }

    /// Immediately close the subscription
    ///
    /// The `Stream` implementation will continue to yield any remaining
    /// in-flight or otherwise buffered messages.
    ///
    /// Calling this method multiple times is a NOOP.
    ///
    /// # Errors
    ///
    /// It returns an error if the client is closed.
    pub async fn close(&mut self) -> Result<(), ClientClosedError> {
        match (self.status, self.receiver.is_closed()) {
            (SubscriptionStatus::Subscribed, true) => {
                self.status = SubscriptionStatus::Unsubscribed;
            }
            (SubscriptionStatus::Subscribed, false) => {
                self.client.unsubscribe(self.id, None).await?;
                self.status = SubscriptionStatus::Unsubscribed;
            }
            (SubscriptionStatus::Unsubscribed, _) => {}
        }

        Ok(())
    }

    /// Close the subscription after `max_messages` have been delivered
    ///
    /// Ask the NATS Server to automatically close the subscription after
    /// `max_messages` have been sent to the client.
    ///
    /// <div class="warning">
    ///    Calling this method does not guarantee that the <code>Stream</code> will
    ///    deliver the exact number of messages specified in <code>max_messages</code>.
    /// </div>
    ///
    /// More or less messages may be delivered to the client due to race conditions
    /// or data loss between it and the server.
    ///
    /// More messages could be delivered if the server receives the `close_after` command
    /// after it has already started buffering messages to send to the client.
    /// The same race condition could occur after a reconnect.
    ///
    /// Fewer messages could be delivered if the connection handler is faster
    /// at producing messages than the [`Stream`] is at reading them, causing the
    /// channel to fill up and starting to drop subsequent messages.
    ///
    /// # Errors
    ///
    /// It returns an error if the client is closed.
    pub async fn close_after(
        &mut self,
        max_messages: NonZero<u64>,
    ) -> Result<(), ClientClosedError> {
        match (self.status, self.receiver.is_closed()) {
            (SubscriptionStatus::Subscribed, true) => {
                self.status = SubscriptionStatus::Unsubscribed;
            }
            (SubscriptionStatus::Subscribed, false) => {
                self.client.unsubscribe(self.id, Some(max_messages)).await?;
            }
            (SubscriptionStatus::Unsubscribed, _) => {}
        }

        Ok(())
    }
}

impl Stream for Subscription {
    type Item = Result<ServerMessage, ServerError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if let Some(msg) = this.receiver_queue.pop() {
            return Poll::Ready(Some(msg));
        }

        match Pin::new(&mut this.receiver).poll_recv_many(
            cx,
            &mut this.receiver_queue,
            BATCH_RECEIVE_SIZE,
        ) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(n @ 1..) => {
                debug_assert_eq!(n, this.receiver_queue.len());
                this.receiver_queue.reverse();
                Poll::Ready(this.receiver_queue.pop())
            }
            Poll::Ready(0) => {
                this.status = SubscriptionStatus::Unsubscribed;
                Poll::Ready(None)
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.receiver_queue.len(), None)
    }
}

impl FusedStream for Subscription {
    fn is_terminated(&self) -> bool {
        self.receiver.is_closed() && self.receiver_queue.is_empty()
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if matches!(self.status, SubscriptionStatus::Unsubscribed) || self.receiver.is_closed() {
            return;
        }

        self.client.lazy_unsubscribe(self.id, None);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        pin::pin,
        task::{Context, Poll},
    };

    use bytes::Bytes;
    use claims::assert_matches;
    use futures_util::{StreamExt, task::noop_waker_ref};
    use tokio::sync::mpsc::error::TryRecvError;
    use watermelon_proto::{
        MessageBase, ServerMessage, StatusCode, Subject, SubscriptionId, headers::HeaderMap,
    };

    use crate::{core::Client, handler::HandlerCommand};

    #[tokio::test]
    async fn subscribe() {
        let (client, mut handler) = Client::test(1);

        let mut subscription = client
            .subscribe(Subject::from_static("abcd.>"), None)
            .await
            .unwrap();

        let subscribe_command = handler.receiver.try_recv().unwrap();
        let HandlerCommand::Subscribe {
            id,
            subject,
            queue_group,
            messages,
        } = subscribe_command
        else {
            unreachable!()
        };
        assert_eq!(SubscriptionId::from(1), id);
        assert_eq!(Subject::from_static("abcd.>"), subject);
        assert_eq!(None, queue_group);

        // Messages are delivered as expected

        let (flag, waker) = crate::tests::FlagWaker::new();
        let mut cx = Context::from_waker(&waker);

        let mut expected_wakes = 0;
        for num_messages in 0..32 {
            assert!(subscription.poll_next_unpin(&mut cx).is_pending());
            assert_eq!(expected_wakes, flag.wakes());

            let msgs = (0..num_messages)
                .map(|num| ServerMessage {
                    status_code: Some(StatusCode::OK),
                    subscription_id: SubscriptionId::from(1),
                    base: MessageBase {
                        subject: format!("abcd.{num}").try_into().unwrap(),
                        reply_subject: None,
                        headers: HeaderMap::new(),
                        payload: Bytes::from_static(b"test"),
                    },
                })
                .collect::<Vec<_>>();
            for msg in &msgs {
                messages.try_send(Ok(msg.clone())).unwrap();
            }
            if num_messages > 0 {
                expected_wakes += 1;
            }

            assert_eq!(expected_wakes, flag.wakes());
            for msg in msgs {
                assert_eq!(
                    Poll::Ready(Some(Ok(msg))),
                    subscription.poll_next_unpin(&mut cx)
                );
            }
            assert!(subscription.poll_next_unpin(&mut cx).is_pending());
        }

        drop(messages);
        expected_wakes += 1;

        assert_eq!(expected_wakes, flag.wakes());
        assert_eq!(Poll::Ready(None), subscription.poll_next_unpin(&mut cx));
    }

    #[tokio::test]
    async fn unsubscribe() {
        let (client, mut handler) = Client::test(1);

        let mut subscription = client
            .subscribe(Subject::from_static("abcd.>"), None)
            .await
            .unwrap();

        let subscribe_command = handler.receiver.try_recv().unwrap();
        assert_matches!(subscribe_command, HandlerCommand::Subscribe { .. });

        // Closing the subscription sends `Unsubscribe`

        subscription.close().await.unwrap();
        let HandlerCommand::Unsubscribe {
            id,
            max_messages: None,
        } = handler.receiver.try_recv().unwrap()
        else {
            unreachable!()
        };
        assert_eq!(SubscriptionId::from(1), id);

        // Unsubscribing again is a NOOP

        subscription.close().await.unwrap();
        assert_eq!(
            TryRecvError::Empty,
            handler.receiver.try_recv().unwrap_err()
        );

        // Same when dropping the subscription

        drop(subscription);
        assert_eq!(
            TryRecvError::Empty,
            handler.receiver.try_recv().unwrap_err()
        );
    }

    #[tokio::test]
    async fn drop_unsubscribe() {
        let (client, mut handler) = Client::test(1);

        let subscription = client
            .subscribe(Subject::from_static("abcd.>"), None)
            .await
            .unwrap();

        let subscribe_command = handler.receiver.try_recv().unwrap();
        let HandlerCommand::Subscribe {
            id,
            subject,
            queue_group,
            messages: _,
        } = subscribe_command
        else {
            unreachable!()
        };
        assert_eq!(SubscriptionId::from(1), id);
        assert_eq!(Subject::from_static("abcd.>"), subject);
        assert_eq!(None, queue_group);

        // Dropping `Subscription` sends `Unsubscribe`

        drop(subscription);
        let HandlerCommand::Unsubscribe {
            id,
            max_messages: None,
        } = handler.receiver.try_recv().unwrap()
        else {
            unreachable!()
        };
        assert_eq!(SubscriptionId::from(1), id);
    }

    #[tokio::test]
    async fn subscribe_is_cancel_safe() {
        let (client, mut handler) = Client::test(1);

        let subscription = client
            .subscribe(Subject::from_static("abcd.>"), None)
            .await
            .unwrap();

        {
            let subscribe_future = pin!(client.subscribe(Subject::from_static("dcba.>"), None));

            let mut cx = Context::from_waker(noop_waker_ref());
            assert!(subscribe_future.poll(&mut cx).is_pending());
        }

        let subscribe_command = handler.receiver.try_recv().unwrap();
        let HandlerCommand::Subscribe { id, .. } = subscribe_command else {
            unreachable!()
        };
        assert_eq!(SubscriptionId::from(1), id);

        let subscription2 = client
            .subscribe(Subject::from_static("abcd.>"), None)
            .await
            .unwrap();

        let subscribe_command = handler.receiver.try_recv().unwrap();
        let HandlerCommand::Subscribe { id, .. } = subscribe_command else {
            unreachable!()
        };
        assert_eq!(SubscriptionId::from(2), id);

        // Failing to unsubscribe triggers `is_failed_unsubscribe = true`

        assert!(!handler.quick_info.get().is_failed_unsubscribe);
        drop(subscription);
        assert!(!handler.quick_info.get().is_failed_unsubscribe);
        drop(subscription2);
        assert!(handler.quick_info.get().is_failed_unsubscribe);
    }
}
