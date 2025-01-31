#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub use self::connect::{Connect, NonStandardConnect};
pub use self::message::{MessageBase, ServerMessage};
pub use self::queue_group::QueueGroup;
pub use self::server_addr::{Host, Protocol, ServerAddr, Transport};
pub use self::server_info::{NonStandardServerInfo, ServerInfo};
pub use self::status_code::StatusCode;
pub use self::subject::Subject;
pub use self::subscription_id::SubscriptionId;

mod connect;
pub mod headers;
mod message;
pub mod proto;
mod queue_group;
mod server_addr;
mod server_error;
mod server_info;
mod status_code;
mod subject;
mod subscription_id;
#[cfg(test)]
mod tests;
mod util;

pub mod error {
    pub use super::queue_group::QueueGroupValidateError;
    pub use super::server_addr::ServerAddrError;
    pub use super::server_error::ServerError;
    pub use super::status_code::StatusCodeError;
    pub use super::subject::SubjectValidateError;
    pub use super::util::ParseUintError;
}
