//! Run tests against isolated, disposable `nats-server` instances
//!
//! Every test gets its own [`NatsServer`], bound to random ports on
//! `127.0.0.1` and backed by a private temporary directory, so tests can
//! run in parallel without sharing any state.
//!
//! ```no_run
//! use watermelon_testkit::NatsServer;
//!
//! # async fn example() {
//! let Some(server) = NatsServer::builder().jetstream().start().await else {
//!     // nats-server isn't installed: skip the test
//!     return;
//! };
//! println!("connect to {}", server.client_url());
//! # }
//! ```
//!
//! # Locating `nats-server`
//!
//! The binary is taken from the `NATS_SERVER_BIN` environment variable when
//! it points to an existing file, otherwise it's looked up in `PATH`.
//!
//! When no binary can be found [`NatsServerBuilder::start`] returns `None`,
//! so that the test can be skipped. If the `CI` environment variable is set
//! it panics instead, so that a misconfigured CI can't silently skip tests.
//!
//! # Debugging failures
//!
//! The server runs with trace logging enabled. When a test panics while a
//! [`NatsServer`] is alive, the tail of the server log is printed and the
//! server's temporary directory, containing the configuration and the full
//! log, is kept.

#![cfg(unix)]

pub use self::server::{NatsServer, NatsServerBuilder};
pub use self::time::{TimeoutExt, eventually};

mod http;
mod server;
mod time;
