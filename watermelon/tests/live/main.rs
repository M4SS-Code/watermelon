//! Tests running against a live `nats-server`
//!
//! See the `watermelon-testkit` crate for how the server binary is located.

#![cfg(unix)]

mod core;
mod jetstream;
mod util;
