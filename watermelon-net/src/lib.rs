#![forbid(unsafe_code)]

#[cfg(feature = "websocket")]
pub use self::connection::WebsocketConnection;
pub use self::connection::{Connection, StreamingConnection, connect as proto_connect};
pub use self::happy_eyeballs::connect as connect_tcp;

mod connection;
mod future;
mod happy_eyeballs;

pub mod error {
    #[cfg(feature = "websocket")]
    pub use super::connection::WebsocketReadError;
    pub use super::connection::{ConnectError, ConnectionReadError, StreamingReadError};
}
