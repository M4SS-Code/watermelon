use std::{
    future::Future,
    mem,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use tokio::time::{Instant, Sleep, sleep};

/// A delay mechanism that enforces a minimum duration between operations.
///
/// `Delayed` ensures that successive calls to `poll_can_proceed` are separated
/// by at least the specified duration.
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
    /// Create a new `Delayed` with the specified duration.
    ///
    /// If `duration` is zero, the delay is effectively disabled and all
    /// calls to `poll_can_proceed` will return `Poll::Ready(())` immediately.
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

    /// Poll whether the operation can proceed based on the configured delay.
    ///
    /// This method implements a rate-limiting mechanism:
    ///
    /// - On first call or after a delay has elapsed, returns `Poll::Pending`
    /// - If called again before the delay duration has passed, returns `Poll::Pending`
    /// - Automatically resets the delay timer when the delay is polled again *after* it
    ///   has previously completed.
    ///
    /// When configured with [`Duration::ZERO`], this method always returns `Poll::Ready(())`.
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

#[cfg(test)]
mod tests {
    use std::{
        future,
        task::{Context, Waker},
        time::Duration,
    };

    use claims::assert_ready;
    use tokio::time::{Instant, sleep};

    use super::Delayed;

    #[test]
    fn zero_interval_always_ready() {
        let mut delayed = Delayed::new(Duration::ZERO);

        for _ in 0..100 {
            let mut cx = Context::from_waker(Waker::noop());
            assert_ready!(delayed.poll_can_proceed(&mut cx));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn delay_behaviour() {
        const INTERVAL: Duration = Duration::from_millis(250);

        let mut delayed = Delayed::new(INTERVAL);
        let before = Instant::now();
        future::poll_fn(|cx| delayed.poll_can_proceed(cx)).await;
        assert_eq!(before.elapsed(), INTERVAL);

        sleep(INTERVAL * 3).await;

        let before = Instant::now();
        future::poll_fn(|cx| delayed.poll_can_proceed(cx)).await;
        assert_eq!(before.elapsed(), INTERVAL);
    }
}
