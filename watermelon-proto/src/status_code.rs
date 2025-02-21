use core::{
    fmt::{self, Display, Formatter},
    num::NonZeroU16,
    str::FromStr,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::util;

/// A NATS status code
///
/// Constants are provided for known and accurately status codes
/// within the NATS Server.
///
/// Values are guaranteed to be in range `100..1000`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StatusCode(NonZeroU16);

impl StatusCode {
    /// The Jetstream consumer hearthbeat timeout has been reached with no new messages to deliver
    ///
    /// See [ADR-9].
    ///
    /// [ADR-9]: https://github.com/nats-io/nats-architecture-and-design/blob/main/adr/ADR-9.md
    pub const IDLE_HEARTBEAT: StatusCode = Self::new_internal(100);
    /// The request has successfully been sent
    pub const OK: StatusCode = Self::new_internal(200);
    /// The requested Jetstream resource doesn't exist
    pub const NOT_FOUND: StatusCode = Self::new_internal(404);
    /// The pull consumer batch reached the timeout
    pub const TIMEOUT: StatusCode = Self::new_internal(408);
    /// The request was sent to a subject that does not appear to have any subscribers listening
    pub const NO_RESPONDERS: StatusCode = Self::new_internal(503);

    /// Decodes a status code from a slice of ASCII characters.
    ///
    /// The ASCII representation is expected to be in the form of `"NNN"`, where `N` is a numeric
    /// digit.
    ///
    /// # Errors
    ///
    /// It returns an error if the slice of bytes does not contain a valid status code.
    pub fn from_ascii_bytes(buf: &[u8]) -> Result<Self, StatusCodeError> {
        if buf.len() != 3 {
            return Err(StatusCodeError);
        }

        util::parse_u16(buf)
            .map_err(|_| StatusCodeError)?
            .try_into()
            .map(Self)
            .map_err(|_| StatusCodeError)
    }

    const fn new_internal(val: u16) -> Self {
        match NonZeroU16::new(val) {
            Some(val) => Self(val),
            None => unreachable!(),
        }
    }
}

impl FromStr for StatusCode {
    type Err = StatusCodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_ascii_bytes(s.as_bytes())
    }
}

impl Display for StatusCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl TryFrom<u16> for StatusCode {
    type Error = StatusCodeError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        if (100..1000).contains(&value) {
            Ok(Self(NonZeroU16::new(value).unwrap()))
        } else {
            Err(StatusCodeError)
        }
    }
}

impl From<StatusCode> for u16 {
    fn from(value: StatusCode) -> Self {
        value.0.get()
    }
}

impl Serialize for StatusCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        u16::from(*self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StatusCode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let n = u16::deserialize(deserializer)?;
        n.try_into().map_err(de::Error::custom)
    }
}

/// An error encountered while parsing [`StatusCode`]
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
#[error("invalid status code")]
pub struct StatusCodeError;

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use claims::assert_err;

    use super::StatusCode;

    #[test]
    fn valid_status_codes() {
        let status_codes = [100, 200, 404, 408, 409, 503];

        for status_code in status_codes {
            assert_eq!(
                status_code,
                u16::from(StatusCode::try_from(status_code).unwrap())
            );

            let s = status_code.to_string();
            assert_eq!(
                status_code,
                u16::from(StatusCode::from_ascii_bytes(s.as_bytes()).unwrap())
            );
        }
    }

    #[test]
    fn invalid_status_codes() {
        let status_codes = [0, 5, 55, 9999];

        for status_code in status_codes {
            assert_err!(StatusCode::try_from(status_code));

            let s = status_code.to_string();
            assert_err!(StatusCode::from_ascii_bytes(s.as_bytes()));
        }
    }
}
