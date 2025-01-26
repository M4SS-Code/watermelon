use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures_core::Stream;

#[derive(Debug, Clone)]
pub(crate) struct IterToStream<I> {
    pub(crate) iter: I,
}

impl<I> Unpin for IterToStream<I> {}

impl<I: Iterator> Stream for IterToStream<I> {
    type Item = I::Item;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<I::Item>> {
        Poll::Ready(self.iter.next())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}
