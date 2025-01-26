use std::{future::Future, pin::Pin};

/// An alternative to [`future_core::future::BoxFuture`] that is also `Sync`
pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + Sync + 'a>>;
