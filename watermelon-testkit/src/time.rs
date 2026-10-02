use std::{
    future::{Future, IntoFuture},
    panic::Location,
    time::Duration,
};

use tokio::time::Instant;

/// Bound how long a test is willing to wait on a future
pub trait TimeoutExt: IntoFuture + Sized {
    /// Await `self`, panicking if it doesn't complete within `timeout`
    ///
    /// The panic message points at the caller, so that a hung test
    /// reports what it was waiting on instead of stalling CI.
    #[track_caller]
    fn within(self, timeout: Duration) -> impl Future<Output = Self::Output> {
        let caller = Location::caller();
        async move {
            tokio::time::timeout(timeout, self)
                .await
                .unwrap_or_else(|_elapsed| {
                    panic!("future at {caller} did not complete within {timeout:?}")
                })
        }
    }
}

impl<F: IntoFuture> TimeoutExt for F {}

/// Poll `condition` until it returns `true`, panicking after `timeout`
///
/// Only use this for state the test cannot synchronize with directly,
/// like the server noticing that a TCP connection was closed.
///
/// # Panics
///
/// Panics if `condition` is still `false` after `timeout`.
#[track_caller]
pub fn eventually(
    timeout: Duration,
    mut condition: impl AsyncFnMut() -> bool,
) -> impl Future<Output = ()> {
    let caller = Location::caller();
    async move {
        let deadline = Instant::now() + timeout;
        while !condition().await {
            assert!(
                Instant::now() < deadline,
                "condition at {caller} did not become true within {timeout:?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
