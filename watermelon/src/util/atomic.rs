#[cfg(feature = "portable-atomic")]
pub(crate) use portable_atomic::*;
#[cfg(not(feature = "portable-atomic"))]
pub(crate) use std::sync::atomic::*;
