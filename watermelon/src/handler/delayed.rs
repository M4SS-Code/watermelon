use std::{
    future::Future,
    mem,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use tokio::time::{Instant, Sleep, sleep};

#[derive(Debug)]
pub(super) struct Delayed {
    inner: Option<DelayedInner>,
}

#[derive(Debug)]
struct DelayedInner {
    // INVARIANT: `duration != Duration::ZERO`
    duration: Duration,
    delay: Pin<Box<Sleep>>,
    delay_consumed: bool,
}

impl Delayed {
    pub(super) fn new(duration: Duration) -> Self {
        let inner = if duration.is_zero() {
            None
        } else {
            Some(DelayedInner {
                duration,
                delay: Box::pin(sleep(duration)),
                delay_consumed: true,
            })
        };

        Self { inner }
    }

    pub(super) fn poll_can_proceed(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if let Some(inner) = &mut self.inner {
            if mem::take(&mut inner.delay_consumed) {
                inner.delay.as_mut().reset(Instant::now() + inner.duration);
            }

            if inner.delay.as_mut().poll(cx).is_ready() {
                inner.delay_consumed = true;
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        } else {
            Poll::Ready(())
        }
    }
}
