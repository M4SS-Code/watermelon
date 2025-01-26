use std::{
    collections::VecDeque,
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
    future::BoxFuture,
};

/// A request to list streams
///
/// Obtained from [`JetstreamClient::streams`].
#[must_use = "streams do nothing unless polled"]
pub struct Streams {
    client: JetstreamClient,
    offset: u32,
    fetch: Option<BoxFuture<'static, Result<StreamsResponse, JetstreamError2>>>,
    buffer: VecDeque<client::Stream>,
    exhausted: bool,
}

#[derive(Debug, Deserialize)]
struct StreamsResponse {
    limit: u32,
    streams: VecDeque<client::Stream>,
}

impl Streams {
    pub(crate) fn new(client: JetstreamClient) -> Self {
        Self {
            client,
            offset: 0,
            fetch: None,
            buffer: VecDeque::new(),
            exhausted: false,
        }
    }
}

impl Stream for Streams {
    type Item = Result<client::Stream, JetstreamError2>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if let Some(stream) = this.buffer.pop_front() {
            return Poll::Ready(Some(Ok(stream)));
        }

        if this.exhausted {
            return Poll::Ready(None);
        }

        let fetch = this.fetch.get_or_insert_with(|| {
            let client = this.client.clone();
            let offset = this.offset;

            Box::pin(async move {
                let response_fut = client
                    .client()
                    .request(client.subject_for_request(&Subject::from_static("STREAM.LIST")))
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
                this.buffer = response.streams;
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

impl FusedStream for Streams {
    fn is_terminated(&self) -> bool {
        self.buffer.is_empty() && self.exhausted
    }
}
