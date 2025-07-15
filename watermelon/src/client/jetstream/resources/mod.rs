use serde::Deserialize;

pub use self::consumer::{
    AckPolicy, Consumer, ConsumerConfig, ConsumerDurability, ConsumerSpecificConfig,
    ConsumerStorage, DeliverPolicy, ReplayPolicy,
};
pub use self::stream::{
    Compression, DiscardPolicy, RetentionPolicy, Storage, Stream, StreamConfig, StreamState,
};

use super::JetstreamApiError;

mod consumer;
mod stream;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum Response<T> {
    Response(T),
    Error { error: JetstreamApiError },
}

mod nullable_number {
    use std::{any::type_name, fmt::Display};

    use serde::{
        Deserialize, Deserializer, Serialize, Serializer,
        de::{self, DeserializeOwned},
        ser,
    };

    pub(crate) trait NullableNumber: Copy + Display {
        const NULL_VALUE: Self::SignedValue;
        type SignedValue: Copy
            + TryFrom<Self>
            + TryInto<Self>
            + Display
            + Eq
            + Serialize
            + DeserializeOwned;
    }

    impl NullableNumber for u32 {
        const NULL_VALUE: Self::SignedValue = -1;
        type SignedValue = i32;
    }

    impl NullableNumber for u64 {
        const NULL_VALUE: Self::SignedValue = -1;
        type SignedValue = i64;
    }

    #[expect(clippy::ref_option)]
    pub(crate) fn serialize<S, N>(num: &Option<N>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        N: NullableNumber,
    {
        match *num {
            Some(num) => num.try_into().map_err(|_| {
                ser::Error::custom(format!(
                    "{num} can't be converted to {}",
                    type_name::<N::SignedValue>()
                ))
            })?,
            None => N::NULL_VALUE,
        }
        .serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>, N: NullableNumber>(
        deserializer: D,
    ) -> Result<Option<N>, D::Error> {
        let num = N::SignedValue::deserialize(deserializer)?;
        Ok(if num == N::NULL_VALUE {
            None
        } else {
            Some(num.try_into().map_err(|_| {
                de::Error::custom(format!("{num} can't be converted to {}", type_name::<N>()))
            })?)
        })
    }
}

mod option_nonzero {
    use std::num::NonZero;

    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};

    pub(crate) trait NonZeroNumber: Copy {
        type Inner: Copy + Default + From<Self> + TryInto<Self> + Serialize + DeserializeOwned;
    }

    impl NonZeroNumber for NonZero<u32> {
        type Inner = u32;
    }

    #[expect(clippy::ref_option)]
    pub(crate) fn serialize<S, N>(num: &Option<N>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        N: NonZeroNumber,
    {
        match *num {
            Some(num) => <N::Inner as From<N>>::from(num),
            None => Default::default(),
        }
        .serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>, N: NonZeroNumber>(
        deserializer: D,
    ) -> Result<Option<N>, D::Error> {
        let num = <N::Inner as Deserialize>::deserialize(deserializer)?;
        Ok(num.try_into().ok())
    }
}

mod nullable_datetime {
    use jiff::Timestamp;
    use serde::{Deserialize, Deserializer};

    // 0001-01-01T00:00:00Z
    const GOLANG_ZERO: Timestamp = Timestamp::constant(-62_135_596_800, 0);

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Timestamp>, D::Error> {
        let datetime = <Timestamp>::deserialize(deserializer)?;
        Ok(if datetime == GOLANG_ZERO {
            None
        } else {
            Some(datetime)
        })
    }
}

mod duration {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(crate) fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        duration.as_nanos().serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Duration, D::Error> {
        Ok(Duration::from_nanos(u64::deserialize(deserializer)?))
    }
}

mod duration_vec {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    #[expect(
        clippy::ptr_arg,
        reason = "this must follow the signature expected by serde"
    )]
    pub(crate) fn serialize<S>(durations: &Vec<Duration>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_seq(durations.iter().map(std::time::Duration::as_nanos))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Duration>, D::Error> {
        let durations = <Vec<u64> as Deserialize>::deserialize(deserializer)?;
        Ok(durations.into_iter().map(Duration::from_nanos).collect())
    }
}

mod compression {
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum CompressionInner {
        None,
        S2,
    }

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::Compression;

    #[expect(clippy::ref_option)]
    pub(crate) fn serialize<S>(
        compression: &Option<Compression>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match compression {
            None => CompressionInner::None,
            Some(Compression::S2) => CompressionInner::S2,
        }
        .serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Compression>, D::Error> {
        Ok(match CompressionInner::deserialize(deserializer)? {
            CompressionInner::None => None,
            CompressionInner::S2 => Some(Compression::S2),
        })
    }
}

mod opposite_bool {
    use std::ops::Not;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "this must follow the signature expected by serde"
    )]
    pub(crate) fn serialize<S>(val: &bool, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        val.not().serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<bool, D::Error> {
        bool::deserialize(deserializer).map(Not::not)
    }
}
