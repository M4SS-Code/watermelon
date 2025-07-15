use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use futures_core::{FusedStream, Stream};
use pin_project_lite::pin_project;
use watermelon_proto::ServerMessage;

use crate::{
    client::{Consumer, JetstreamClient, JetstreamError},
    util::BoxFuture,
};

use super::{ConsumerBatch, consumer_batch::ConsumerBatchError};

pin_project! {
    /// A consumer stream of batch requests
    ///
    /// Obtained from [`JetstreamClient::consumer_stream`].
    #[must_use = "streams do nothing unless polled"]
    pub struct ConsumerStream {
        #[pin]
        status: ConsumerStreamStatus,
        consumer: Consumer,
        client: JetstreamClient,

        expires: Duration,
        max_msgs: usize,
    }
}

pin_project! {
    #[project = ConsumerStreamStatusProj]
    enum ConsumerStreamStatus {
        Polling {
            future: BoxFuture<'static, Result<ConsumerBatch, JetstreamError>>,
        },
        RunningBatch {
            #[pin]
            batch: ConsumerBatch,
        },
        Broken,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConsumerStreamError {
    #[error("consumer batch error")]
    BatchError(#[source] ConsumerBatchError),
    #[error("jetstream error")]
    Jetstream(#[source] JetstreamError),
}

impl ConsumerStream {
    pub(crate) fn new(
        consumer: Consumer,
        client: JetstreamClient,
        expires: Duration,
        max_msgs: usize,
    ) -> Self {
        let poll_fut = {
            let client = client.clone();
            Box::pin(ConsumerBatch::new(&consumer, client, expires, max_msgs))
        };

        Self {
            status: ConsumerStreamStatus::Polling { future: poll_fut },
            consumer,
            client,

            expires,
            max_msgs,
        }
    }
}

impl Stream for ConsumerStream {
    type Item = Result<ServerMessage, ConsumerStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        match this.status.as_mut().project() {
            ConsumerStreamStatusProj::RunningBatch { batch } => match batch.poll_next(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Some(Ok(msg))) => Poll::Ready(Some(Ok(msg))),
                Poll::Ready(Some(Err(err))) => {
                    this.status.set(ConsumerStreamStatus::Broken);
                    Poll::Ready(Some(Err(ConsumerStreamError::BatchError(err))))
                }
                Poll::Ready(None) => {
                    this.status.set(ConsumerStreamStatus::Polling {
                        future: Box::pin(ConsumerBatch::new(
                            this.consumer,
                            this.client.clone(),
                            *this.expires,
                            *this.max_msgs,
                        )),
                    });

                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            },
            ConsumerStreamStatusProj::Polling { future: fut } => match Pin::new(fut).poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Ok(batch)) => {
                    this.status
                        .set(ConsumerStreamStatus::RunningBatch { batch });

                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
                Poll::Ready(Err(err)) => {
                    this.status.set(ConsumerStreamStatus::Broken);
                    Poll::Ready(Some(Err(ConsumerStreamError::Jetstream(err))))
                }
            },
            ConsumerStreamStatusProj::Broken => Poll::Ready(None),
        }
    }
}

impl FusedStream for ConsumerStream {
    fn is_terminated(&self) -> bool {
        matches!(self.status, ConsumerStreamStatus::Broken)
    }
}
