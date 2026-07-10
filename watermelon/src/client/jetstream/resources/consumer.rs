use std::{collections::BTreeMap, num::NonZero, time::Duration};

use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use watermelon_proto::{QueueGroup, Subject};

use super::{duration, duration_vec, nullable_number, option_nonzero};

/// A Jetstream consumer
#[derive(Debug, Serialize, Deserialize)]
pub struct Consumer {
    pub stream_name: String,
    pub config: ConsumerConfig,
    #[serde(rename = "created")]
    pub created_at: Timestamp,
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
    pub rate_limit: Option<NonZero<u64>>,
    pub headers_only: bool,

    pub specs: ConsumerSpecificConfig,

    // Inactivity threshold.
    pub inactive_threshold: Duration,
    pub replicas: Option<NonZero<u32>>,
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
        flow_control: bool,
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
        from: Timestamp,
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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    filter_subject: Option<Subject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    filter_subjects: Vec<Subject>,
    replay_policy: ReplayPolicy,

    #[serde(rename = "rate_limit_bps", skip_serializing_if = "Option::is_none")]
    rate_limit: Option<NonZero<u64>>,
    #[serde(default, skip_serializing_if = "is_false")]
    flow_control: bool,
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
    replicas: Option<NonZero<u32>>,
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

        // A single filter must be serialized via the singular `filter_subject`:
        // the server rejects the subject based create API when the plural
        // `filter_subjects` is set, no matter how many entries it contains
        let (filter_subject, filter_subjects) = match &*self.filter_subjects {
            [filter_subject] => (Some(filter_subject.clone()), Vec::new()),
            _ => (None, self.filter_subjects.clone()),
        };

        let (
            max_waiting,
            max_request_batch,
            max_request_expires,
            max_request_max_bytes,
            deliver_subject,
            deliver_group,
            flow_control,
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
                false,
                Duration::ZERO,
            ),
            ConsumerSpecificConfig::Push {
                deliver_subject,
                deliver_group,
                flow_control,
                idle_heartbeat,
            } => (
                None,
                None,
                Duration::ZERO,
                None,
                Some(deliver_subject.clone()),
                deliver_group.clone(),
                *flow_control,
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
            flow_control,
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
        let filter_subjects = match (filter_subject, filter_subjects) {
            (Some(filter_subject), filter_subjects) if filter_subjects.is_empty() => {
                vec![filter_subject]
            }
            (None, filter_subjects) => filter_subjects,
            (Some(_), _) => {
                return Err(de::Error::custom(
                    "consumer has both filter_subject and filter_subjects set",
                ));
            }
        };
        let (durability, name) = if !durable_name.is_empty() {
            (ConsumerDurability::Durable, durable_name)
        } else if !name.is_empty() {
            (ConsumerDurability::Ephemeral, name)
        } else {
            return Err(de::Error::custom(
                "consumer neither has a name or a durable name",
            ));
        };

        let specs = match deliver_subject {
            Some(deliver_subject) => ConsumerSpecificConfig::Push {
                deliver_subject,
                deliver_group,
                idle_heartbeat,
                flow_control,
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

#[expect(clippy::trivially_copy_pass_by_ref, reason = "serde works this way")]
fn is_false(val: &bool) -> bool {
    !*val
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, time::Duration};

    use serde_json::json;
    use watermelon_proto::Subject;

    use super::{
        AckPolicy, ConsumerConfig, ConsumerDurability, ConsumerSpecificConfig, ConsumerStorage,
        DeliverPolicy, ReplayPolicy,
    };

    fn config(filter_subjects: Vec<Subject>) -> ConsumerConfig {
        ConsumerConfig {
            durability: ConsumerDurability::Durable,
            name: "test".to_owned(),
            description: String::new(),
            deliver_policy: DeliverPolicy::All,
            // `Some`: the serialized form always contains `max_ack_pending` and
            // `max_deliver`, mirroring the NATS Server which sets defaults for both
            ack_policy: AckPolicy::Explicit {
                wait: Duration::from_secs(30),
                max_pending: Some(1000),
            },
            max_deliver: Some(3),
            backoff: Vec::new(),
            filter_subjects,
            replay_policy: ReplayPolicy::Instant,
            rate_limit: None,
            headers_only: false,
            specs: ConsumerSpecificConfig::Pull {
                max_waiting: None,
                max_request_batch: None,
                max_request_expires: Duration::ZERO,
                max_request_max_bytes: None,
            },
            inactive_threshold: Duration::ZERO,
            replicas: None,
            storage: ConsumerStorage::Disk,
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn no_filters() {
        let value = serde_json::to_value(config(Vec::new())).unwrap();
        assert_eq!(None, value.get("filter_subject"));
        assert_eq!(None, value.get("filter_subjects"));

        let config = serde_json::from_value::<ConsumerConfig>(value).unwrap();
        assert!(config.filter_subjects.is_empty());
    }

    #[test]
    fn single_filter_uses_singular_field() {
        let value = serde_json::to_value(config(vec![Subject::from_static("orders.new")])).unwrap();
        assert_eq!(Some(&json!("orders.new")), value.get("filter_subject"));
        assert_eq!(None, value.get("filter_subjects"));

        let config = serde_json::from_value::<ConsumerConfig>(value).unwrap();
        assert_eq!(
            vec![Subject::from_static("orders.new")],
            config.filter_subjects
        );
    }

    #[test]
    fn multiple_filters_use_plural_field() {
        let filter_subjects = vec![
            Subject::from_static("orders.new"),
            Subject::from_static("orders.shipped"),
        ];

        let value = serde_json::to_value(config(filter_subjects.clone())).unwrap();
        assert_eq!(None, value.get("filter_subject"));
        assert_eq!(
            Some(&json!(["orders.new", "orders.shipped"])),
            value.get("filter_subjects")
        );

        let config = serde_json::from_value::<ConsumerConfig>(value).unwrap();
        assert_eq!(filter_subjects, config.filter_subjects);
    }

    #[test]
    fn both_filter_fields_are_rejected() {
        let mut value =
            serde_json::to_value(config(vec![Subject::from_static("orders.new")])).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("filter_subjects".to_owned(), json!(["orders.shipped"]));

        assert!(serde_json::from_value::<ConsumerConfig>(value).is_err());
    }
}
