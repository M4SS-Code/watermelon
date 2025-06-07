use std::{pin::Pin, task::Context, time::Duration};

use tokio::time::{Instant, Sleep, sleep};

const INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub(super) struct Pinger {
    interval: Pin<Box<Sleep>>,
    /// Number of unacknowledged pings
    pending_pings: u8,
}

#[derive(Debug)]
pub(super) enum PingOutcome {
    /// Either the PING was sent or no ping was needed
    Ok,
    /// Too many unacknowledged pings are in flight
    TooManyInFlightPings,
}

impl Pinger {
    /// Create a new pinger
    pub(super) fn new() -> Self {
        Self {
            interval: Box::pin(sleep(INTERVAL)),
            pending_pings: 0,
        }
    }

    /// Reset the ping timer to trigger after the full interval
    pub(super) fn reset(&mut self) {
        self.interval.as_mut().reset(Instant::now() + INTERVAL);
    }

    /// Handle a received PONG response
    pub(super) fn handle_pong(&mut self) {
        self.pending_pings = self.pending_pings.saturating_sub(1);
    }

    /// Poll for ping readiness and send a PING if the interval has elapsed
    pub(super) fn poll(&mut self, cx: &mut Context<'_>, send_ping: impl FnOnce()) -> PingOutcome {
        if self.interval.as_mut().poll(cx).is_pending() {
            PingOutcome::Ok
        } else {
            self.do_ping(cx, send_ping)
        }
    }

    #[cold]
    fn do_ping(&mut self, cx: &mut Context<'_>, send_ping: impl FnOnce()) -> PingOutcome {
        if self.pending_pings < 2 {
            send_ping();
            self.pending_pings += 1;

            // register the waker for the next ping
            loop {
                self.reset();
                if self.interval.as_mut().poll(cx).is_pending() {
                    break;
                }
            }

            PingOutcome::Ok
        } else {
            PingOutcome::TooManyInFlightPings
        }
    }
}
