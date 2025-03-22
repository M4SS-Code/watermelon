use std::{num::NonZeroU32, time::Duration};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{compression, duration, nullable_datetime, nullable_number, opposite_bool};

/// A Jetstream stream
#[derive(Debug, Deserialize)]
pub struct Stream {
    pub config: StreamConfig,
    #[serde(rename = "created")]
    pub created_at: Timestamp,
    // TODO: `cluster`
}

/// The state of the stream
#[derive(Debug, Deserialize)]
pub struct StreamState {
    pub messages: u64,
    pub bytes: u64,
    pub first_sequence: u64,
    #[serde(with = "nullable_datetime", rename = "first_ts")]
    pub first_sequence_timestamp: Option<Timestamp>,
    pub last_sequence: u64,
    #[serde(with = "nullable_datetime", rename = "last_ts")]
    pub last_sequence_timestamp: Option<Timestamp>,
    pub consumer_count: u32,
}

/// A Jetstream stream configuration
#[derive(Debug, Serialize, Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "it is the actual config of a Jetstream"
)]
pub struct StreamConfig {
    pub name: String,
    pub subjects: Vec<String>,
    #[serde(with = "nullable_number")]
    pub max_consumers: Option<u32>,
    #[serde(with = "nullable_number", rename = "max_msgs")]
    pub max_messages: Option<u64>,
    #[serde(with = "nullable_number")]
    pub max_bytes: Option<u64>,
    #[serde(with = "duration")]
    pub max_age: Duration,
    #[serde(with = "nullable_number", rename = "max_msgs_per_subject")]
    pub max_messages_per_subject: Option<u64>,
    #[serde(with = "nullable_number", rename = "max_msg_size")]
    pub max_message_size: Option<u32>,
    #[serde(rename = "discard")]
    pub discard_policy: DiscardPolicy,
    pub storage: Storage,
    #[serde(rename = "num_replicas")]
    pub replicas: NonZeroU32,
    #[serde(with = "duration")]
    pub duplicate_window: Duration,
    #[serde(with = "compression")]
    pub compression: Option<Compression>,
    pub allow_direct: bool,
    pub mirror_direct: bool,
    pub sealed: bool,
    #[serde(with = "opposite_bool", rename = "deny_delete")]
    pub allow_delete: bool,
    #[serde(with = "opposite_bool", rename = "deny_purge")]
    pub allow_purge: bool,
    pub allow_rollup_hdrs: bool,
    // TODO: `consumer_limits` https://github.com/nats-io/nats-server/blob/e25d973a8f389ce3aa415e4bcdfba1f7d0834f7f/server/stream.go#L99
}

/// A streams retention policy
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionPolicy {
    Limits,
    Interest,
    WorkQueue,
}

/// A streams discard policy
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscardPolicy {
    Old,
    New,
}

/// Whether the disk is stored on disk or in memory
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Storage {
    File,
    Memory,
}

/// The compression algorithm used by a stream
#[derive(Debug)]
pub enum Compression {
    S2,
}
