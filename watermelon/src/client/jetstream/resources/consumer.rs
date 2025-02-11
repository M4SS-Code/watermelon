use std::{
    collections::BTreeMap,
    num::{NonZeroU32, NonZeroU64},
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use watermelon_proto::{QueueGroup, Subject};

use super::{duration, duration_vec, nullable_number, option_nonzero};

/// A Jetstream consumer
#[derive(Debug, Serialize, Deserialize)]
pub struct Consumer {
    pub stream_name: String,
    pub config: ConsumerConfig,
    #[serde(rename = "created")]
    pub created_at: DateTime<Utc>,
}

/// A Jetstream consumer configuration
#[derive(Debug)]
pub struct ConsumerConfig {
    pub durability: ConsumerDurability,
    pub name: String,
    pub description: String,
    pub deliver_policy: DeliverPolicy,
    pub ack_policy: AckPolicy,
    pub max_deliver: Option<u32>,
    pub backoff: Vec<Duration>,
    pub filter_subjects: Vec<Subject>,
    pub replay_policy: ReplayPolicy,
    pub rate_limit: Option<NonZeroU64>,
    pub flow_control: Option<bool>,
    pub headers_only: bool,

    pub specs: ConsumerSpecificConfig,

    // Inactivity threshold.
    pub inactive_threshold: Duration,
    pub replicas: Option<NonZeroU32>,
    pub storage: ConsumerStorage,
    pub metadata: BTreeMap<String, String>,
}

/// Pull or Push configuration parameters for a consumer
#[derive(Debug)]
pub enum ConsumerSpecificConfig {
    Pull {
        max_waiting: Option<i32>,
        max_request_batch: Option<i32>,
        max_request_expires: Duration,
        max_request_max_bytes: Option<i32>,
    },
    Push {
        deliver_subject: Subject,
        deliver_group: Option<QueueGroup>,
        idle_heartbeat: Duration,
    },
}

/// The durability of the consumer
#[derive(Debug)]
pub enum ConsumerDurability {
    Ephemeral,
    Durable,
}

/// The delivery policy of the consumer
#[derive(Debug, Copy, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "deliver_policy")]
pub enum DeliverPolicy {
    #[default]
    #[serde(rename = "all")]
    All,
    #[serde(rename = "last")]
    Last,
    #[serde(rename = "last_per_subject")]
    LastPerSubject,
    #[serde(rename = "new")]
    New,
    #[serde(rename = "by_start_sequence")]
    StartSequence {
        #[serde(rename = "opt_start_seq")]
        sequence: u64,
    },
    #[serde(rename = "by_start_time")]
    StartTime {
        #[serde(rename = "opt_start_time")]
        from: DateTime<Utc>,
    },
}

/// The acknowledgment policy of the consumer
#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(tag = "ack_policy", rename_all = "lowercase")]
pub enum AckPolicy {
    Explicit {
        #[serde(rename = "ack_wait", with = "duration")]
        wait: Duration,
        #[serde(
            rename = "max_ack_pending",
            with = "nullable_number",
            skip_serializing_if = "Option::is_none"
        )]
        max_pending: Option<u32>,
    },
    All {
        #[serde(rename = "ack_wait", with = "duration")]
        wait: Duration,
        #[serde(
            rename = "max_ack_pending",
            with = "nullable_number",
            skip_serializing_if = "Option::is_none"
        )]
        max_pending: Option<u32>,
    },
    None,
}

/// The replay policy of the consumer
#[derive(Debug, Copy, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplayPolicy {
    #[default]
    Instant,
    Original,
}

/// Whether the consumer is kept on disk or in memory
#[derive(Debug, Copy, Clone, Default)]
pub enum ConsumerStorage {
    #[default]
    Disk,
    Memory,
}

#[derive(Debug, Serialize, Deserialize)]
struct RawConsumerConfig {
    #[serde(default)]
    name: String,
    #[serde(default)]
    durable_name: String,
    #[serde(default)]
    description: String,

    #[serde(flatten)]
    deliver_policy: DeliverPolicy,

    #[serde(flatten)]
    ack_policy: AckPolicy,

    #[serde(skip_serializing_if = "Option::is_none", with = "nullable_number")]
    max_deliver: Option<u32>,

    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "duration_vec")]
    backoff: Vec<Duration>,

    #[serde(skip_serializing_if = "Option::is_none")]
    filter_subject: Option<Subject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    filter_subjects: Vec<Subject>,
    replay_policy: ReplayPolicy,

    #[serde(rename = "rate_limit_bps", skip_serializing_if = "Option::is_none")]
    rate_limit: Option<NonZeroU64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    flow_control: Option<bool>,
    #[serde(default, skip_serializing_if = "Duration::is_zero", with = "duration")]
    idle_heartbeat: Duration,
    #[serde(default)]
    headers_only: bool,

    // Pull based options.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_waiting: Option<i32>,
    #[serde(rename = "max_batch", skip_serializing_if = "Option::is_none")]
    max_request_batch: Option<i32>,
    #[serde(
        default,
        rename = "max_expires",
        skip_serializing_if = "Duration::is_zero",
        with = "duration"
    )]
    max_request_expires: Duration,
    #[serde(rename = "max_bytes", skip_serializing_if = "Option::is_none")]
    max_request_max_bytes: Option<i32>,

    // Push based consumers.
    #[serde(rename = "deliver_subject", skip_serializing_if = "Option::is_none")]
    deliver_subject: Option<Subject>,
    #[serde(rename = "deliver_group", skip_serializing_if = "Option::is_none")]
    deliver_group: Option<QueueGroup>,

    #[serde(default, with = "duration")]
    inactive_threshold: Duration,
    #[serde(rename = "num_replicas", with = "option_nonzero")]
    replicas: Option<NonZeroU32>,
    #[serde(default, rename = "mem_storage")]
    storage: ConsumerStorage,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
}

impl Serialize for ConsumerConfig {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (name, durable_name) = match self.durability {
            ConsumerDurability::Ephemeral => (self.name.clone(), String::new()),
            ConsumerDurability::Durable => (self.name.clone(), self.name.clone()),
        };

        let (filter_subject, filter_subjects) = if let [filter_subject] = &*self.filter_subjects {
            (Some(filter_subject.clone()), Vec::new())
        } else {
            (None, self.filter_subjects.clone())
        };

        let (
            max_waiting,
            max_request_batch,
            max_request_expires,
            max_request_max_bytes,
            deliver_subject,
            deliver_group,
            idle_heartbeat,
        ) = match &self.specs {
            ConsumerSpecificConfig::Pull {
                max_waiting,
                max_request_batch,
                max_request_expires,
                max_request_max_bytes,
            } => (
                *max_waiting,
                *max_request_batch,
                *max_request_expires,
                *max_request_max_bytes,
                None,
                None,
                Duration::ZERO,
            ),
            ConsumerSpecificConfig::Push {
                deliver_subject,
                deliver_group,
                idle_heartbeat,
            } => (
                None,
                None,
                Duration::ZERO,
                None,
                Some(deliver_subject.clone()),
                deliver_group.clone(),
                *idle_heartbeat,
            ),
        };

        RawConsumerConfig {
            name,
            durable_name,
            description: self.description.clone(),

            deliver_policy: self.deliver_policy,
            ack_policy: self.ack_policy,
            max_deliver: self.max_deliver,
            backoff: self.backoff.clone(),
            filter_subject,
            filter_subjects,
            replay_policy: self.replay_policy,
            rate_limit: self.rate_limit,
            flow_control: self.flow_control,
            idle_heartbeat,
            headers_only: self.headers_only,

            // Pull based options.
            max_waiting,
            max_request_batch,
            max_request_expires,
            max_request_max_bytes,

            // Push based consumers.
            deliver_subject,
            deliver_group,

            inactive_threshold: self.inactive_threshold,
            replicas: self.replicas,
            storage: self.storage,
            metadata: self.metadata.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ConsumerConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let RawConsumerConfig {
            name,
            durable_name,
            description,
            deliver_policy,
            ack_policy,
            max_deliver,
            backoff,
            filter_subject,
            filter_subjects,
            replay_policy,
            rate_limit,
            flow_control,
            idle_heartbeat,
            headers_only,
            max_waiting,
            max_request_batch,
            max_request_expires,
            max_request_max_bytes,
            deliver_subject,
            deliver_group,
            inactive_threshold,
            replicas,
            storage,
            metadata,
        } = RawConsumerConfig::deserialize(deserializer)?;
        let (durability, name) = if !durable_name.is_empty() {
            (ConsumerDurability::Durable, durable_name)
        } else if !name.is_empty() {
            (ConsumerDurability::Ephemeral, name)
        } else {
            return Err(de::Error::custom(
                "consumer neither has a name or a durable name",
            ));
        };

        let filter_subjects = if let Some(filter_subject) = filter_subject {
            vec![filter_subject]
        } else {
            filter_subjects
        };

        let specs = match deliver_subject {
            Some(deliver_subject) => ConsumerSpecificConfig::Push {
                deliver_subject,
                deliver_group,
                idle_heartbeat,
            },
            None => ConsumerSpecificConfig::Pull {
                max_waiting,
                max_request_batch,
                max_request_expires,
                max_request_max_bytes,
            },
        };

        Ok(Self {
            durability,
            name,
            description,
            deliver_policy,
            ack_policy,
            max_deliver,
            backoff,
            filter_subjects,
            replay_policy,
            rate_limit,
            flow_control,
            headers_only,

            specs,

            inactive_threshold,
            replicas,
            storage,
            metadata,
        })
    }
}

impl Serialize for ConsumerStorage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        matches!(self, Self::Memory).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ConsumerStorage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let b = bool::deserialize(deserializer)?;
        Ok(if b {
            ConsumerStorage::Memory
        } else {
            ConsumerStorage::Disk
        })
    }
}

impl Default for AckPolicy {
    fn default() -> Self {
        Self::All {
            wait: Duration::ZERO,
            max_pending: None,
        }
    }
}
