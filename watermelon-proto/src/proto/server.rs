use alloc::boxed::Box;

use crate::{ServerInfo, error::ServerError, message::ServerMessage};

#[derive(Debug, PartialEq, Eq)]
pub enum ServerOp {
    Info { info: Box<ServerInfo> },
    Message { message: ServerMessage },
    Success,
    Error { error: ServerError },
    Ping,
    Pong,
}
