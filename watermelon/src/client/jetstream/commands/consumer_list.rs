use std::{
    collections::VecDeque,
    fmt::Display,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use futures_core::{FusedStream, Stream};
use serde::Deserialize;
use serde_json::json;
use watermelon_proto::Subject;

use crate::{
    client::{self, jetstream::JetstreamError2, JetstreamClient},
    util::BoxFuture,
};

/// A request to list consumers of a stream
///
/// Obtained from [`JetstreamClient::consumers`].
#[must_use = "streams do nothing unless polled"]
pub struct Consumers {
    client: JetstreamClient,
    offset: u32,
    partial_subject: Subject,
    fetch: Option<BoxFuture<'static, Result<ConsumersResponse, JetstreamError2>>>,
    buffer: VecDeque<client::Consumer>,
    exhausted: bool,
}

#[derive(Debug, Deserialize)]
struct ConsumersResponse {
    limit: u32,
    consumers: VecDeque<client::Consumer>,
}

impl Consumers {
    pub(crate) fn new(client: JetstreamClient, stream_name: impl Display) -> Self {
        let partial_subject = format!("CONSUMER.LIST.{stream_name}")
            .try_into()
            .expect("stream name is valid");
        Self {
            client,
            offset: 0,
            partial_subject,
            fetch: None,
            buffer: VecDeque::new(),
            exhausted: false,
        }
    }
}

impl Stream for Consumers {
    type Item = Result<client::Consumer, JetstreamError2>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if let Some(consumer) = this.buffer.pop_front() {
            return Poll::Ready(Some(Ok(consumer)));
        }

        if this.exhausted {
            return Poll::Ready(None);
        }

        let fetch = this.fetch.get_or_insert_with(|| {
            let client = this.client.clone();
            let partial_subject = this.partial_subject.clone();
            let offset = this.offset;

            Box::pin(async move {
                let response_fut = client
                    .client()
                    .request(client.subject_for_request(&partial_subject))
                    .response_timeout(client.request_timeout)
                    .payload(
                        serde_json::to_vec(&json!({
                            "offset": offset,
                        }))
                        .unwrap()
                        .into(),
                    )
                    .await
                    .map_err(JetstreamError2::ClientClosed)?;
                let response = response_fut.await.map_err(JetstreamError2::ResponseError)?;
                let payload = serde_json::from_slice(&response.base.payload)
                    .map_err(JetstreamError2::Json)?;
                Ok(payload)
            })
        });

        match Pin::new(fetch).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(response)) => {
                this.fetch = None;
                this.buffer = response.consumers;
                if this.buffer.len() < response.limit as usize {
                    this.exhausted = true;
                } else if !this.buffer.is_empty() {
                    this.offset += 1;
                }

                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Poll::Ready(Err(err)) => {
                this.fetch = None;
                Poll::Ready(Some(Err(err)))
            }
        }
    }
}

impl FusedStream for Consumers {
    fn is_terminated(&self) -> bool {
        self.buffer.is_empty() && self.exhausted
    }
}
