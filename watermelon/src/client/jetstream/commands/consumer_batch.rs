use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use futures_core::{FusedStream, Future, Stream};
use pin_project_lite::pin_project;
use serde_json::json;
use tokio::time::{Sleep, sleep};
use watermelon_proto::{ServerMessage, StatusCode, error::ServerError};

use crate::{
    client::{Consumer, JetstreamClient, JetstreamError2},
    subscription::Subscription,
};

pin_project! {
    /// A consumer batch request
    ///
    /// Obtained from [`JetstreamClient::consumer_batch`].
    #[derive(Debug)]
    #[must_use = "streams do nothing unless polled"]
    pub struct ConsumerBatch {
        subscription: Subscription,
        #[pin]
        timeout: Sleep,
        pending_msgs: usize,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConsumerBatchError {
    #[error("an error returned by the server")]
    ServerError(#[source] ServerError),
    #[error("unexpected status code")]
    UnexpectedStatus(ServerMessage),
}

impl ConsumerBatch {
    pub(crate) fn new(
        consumer: &Consumer,
        client: JetstreamClient,
        expires: Duration,
        max_msgs: usize,
    ) -> impl Future<Output = Result<Self, JetstreamError2>> {
        let subject = format!(
            "{}.CONSUMER.MSG.NEXT.{}.{}",
            client.prefix, consumer.stream_name, consumer.config.name
        )
        .try_into();

        async move {
            let subject = subject.map_err(JetstreamError2::Subject)?;
            let incoming_subject = client.client.create_inbox_subject();
            let payload = serde_json::to_vec(&if expires.is_zero() {
                json!({
                    "batch": max_msgs,
                    "no_wait": true,
                })
            } else {
                json!({
                    "batch": max_msgs,
                    "expires": expires.as_nanos(),
                    "no_wait": true
                })
            })
            .map_err(JetstreamError2::Json)?;

            let subscription = client
                .client
                .subscribe(incoming_subject.clone(), None)
                .await
                .map_err(JetstreamError2::ClientClosed)?;
            client
                .client
                .publish(subject)
                .reply_subject(Some(incoming_subject.clone()))
                .payload(payload.into())
                .await
                .map_err(JetstreamError2::ClientClosed)?;

            let timeout = sleep(expires.saturating_add(client.request_timeout));
            Ok(Self {
                subscription,
                timeout,
                pending_msgs: max_msgs,
            })
        }
    }
}

impl Stream for ConsumerBatch {
    type Item = Result<ServerMessage, ConsumerBatchError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();

        if *this.pending_msgs == 0 {
            return Poll::Ready(None);
        }

        match Pin::new(this.subscription).poll_next(cx) {
            Poll::Pending => match this.timeout.poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(()) => {
                    *this.pending_msgs = 0;
                    Poll::Ready(None)
                }
            },
            Poll::Ready(Some(Ok(msg))) => match msg.status_code {
                None | Some(StatusCode::OK) => {
                    *this.pending_msgs -= 1;

                    Poll::Ready(Some(Ok(msg)))
                }
                Some(StatusCode::IDLE_HEARTBEAT) => {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
                Some(StatusCode::TIMEOUT | StatusCode::NOT_FOUND) => {
                    *this.pending_msgs = 0;
                    Poll::Ready(None)
                }
                _ => Poll::Ready(Some(Err(ConsumerBatchError::UnexpectedStatus(msg)))),
            },
            Poll::Ready(Some(Err(err))) => {
                *this.pending_msgs = 0;
                Poll::Ready(Some(Err(ConsumerBatchError::ServerError(err))))
            }
            Poll::Ready(None) => {
                *this.pending_msgs = 0;
                Poll::Ready(None)
            }
        }
    }
}

impl FusedStream for ConsumerBatch {
    fn is_terminated(&self) -> bool {
        self.pending_msgs == 0
    }
}
