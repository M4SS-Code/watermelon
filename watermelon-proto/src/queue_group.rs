use alloc::string::String;
use core::{
    fmt::{self, Display},
    ops::Deref,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use bytestring::ByteString;

/// A string that can be used to represent an queue group
///
/// `QueueGroup` contains a string that is guaranteed [^1] to
/// contain a valid header name that meets the following requirements:
///
/// * The value is not empty
/// * The value has a length less than or equal to 64 [^2]
/// * The value does not contain any whitespace characters or `:`
///
/// `QueueGroup` can be constructed from [`QueueGroup::from_static`]
/// or any of the `TryFrom` implementations.
///
/// [^1]: Because [`QueueGroup::from_dangerous_value`] is safe to call,
///       unsafe code must not assume any of the above invariants.
/// [^2]: Messages coming from the NATS server are allowed to violate this rule.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct QueueGroup(ByteString);

impl QueueGroup {
    /// Construct `QueueGroup` from a static string
    ///
    /// # Panics
    ///
    /// Will panic if `value` isn't a valid `QueueGroup`
    #[must_use]
    pub fn from_static(value: &'static str) -> Self {
        Self::try_from(ByteString::from_static(value)).expect("invalid QueueGroup")
    }

    /// Construct a `QueueGroup` from a string, without checking invariants
    ///
    /// This method bypasses invariants checks implemented by [`QueueGroup::from_static`]
    /// and all `TryFrom` implementations.
    ///
    /// # Security
    ///
    /// While calling this method can eliminate the runtime performance cost of
    /// checking the string, constructing `QueueGroup` with an invalid string and
    /// then calling the NATS server with it can cause serious security issues.
    /// When in doubt use the [`QueueGroup::from_static`] or any of the `TryFrom`
    /// implementations.
    #[must_use]
    #[expect(
        clippy::missing_panics_doc,
        reason = "The queue group validation is only made in debug"
    )]
    pub fn from_dangerous_value(value: ByteString) -> Self {
        if cfg!(debug_assertions) {
            if let Err(err) = validate_queue_group(&value) {
                panic!("QueueGroup {value:?} isn't valid {err:?}");
            }
        }
        Self(value)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for QueueGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl TryFrom<ByteString> for QueueGroup {
    type Error = QueueGroupValidateError;

    fn try_from(value: ByteString) -> Result<Self, Self::Error> {
        validate_queue_group(&value)?;
        Ok(Self::from_dangerous_value(value))
    }
}

impl TryFrom<String> for QueueGroup {
    type Error = QueueGroupValidateError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_queue_group(&value)?;
        Ok(Self::from_dangerous_value(value.into()))
    }
}

impl From<QueueGroup> for ByteString {
    fn from(value: QueueGroup) -> Self {
        value.0
    }
}

impl AsRef<[u8]> for QueueGroup {
    fn as_ref(&self) -> &[u8] {
        self.as_str().as_bytes()
    }
}

impl AsRef<str> for QueueGroup {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Deref for QueueGroup {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl Serialize for QueueGroup {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_str().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for QueueGroup {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = ByteString::deserialize(deserializer)?;
        s.try_into().map_err(de::Error::custom)
    }
}

/// An error encountered while validating [`QueueGroup`]
#[derive(Debug, thiserror::Error)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub enum QueueGroupValidateError {
    /// The value is empty
    #[error("QueueGroup is empty")]
    Empty,
    /// The value has a length greater than 64
    #[error("QueueGroup is too long")]
    TooLong,
    /// The value contains an Unicode whitespace character
    #[error("QueueGroup contained an illegal whitespace character")]
    IllegalCharacter,
}

fn validate_queue_group(queue_group: &str) -> Result<(), QueueGroupValidateError> {
    if queue_group.is_empty() {
        return Err(QueueGroupValidateError::Empty);
    }

    if queue_group.len() > 64 {
        // This is an arbitrary limit, but I guess the server must also have one
        return Err(QueueGroupValidateError::TooLong);
    }

    if queue_group.chars().any(char::is_whitespace) {
        // The theoretical security limit is just ` `, `\t`, `\r` and `\n`.
        // Let's be more careful.
        return Err(QueueGroupValidateError::IllegalCharacter);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use bytestring::ByteString;

    use super::{QueueGroup, QueueGroupValidateError};

    #[test]
    fn valid_queue_groups() {
        let queue_groups = ["importer", "importer.thing", "blablabla:itworks"];
        for queue_group in queue_groups {
            let q = QueueGroup::try_from(ByteString::from_static(queue_group)).unwrap();
            assert_eq!(queue_group, q.as_str());
        }
    }

    #[test]
    fn invalid_queue_groups() {
        let queue_groups = [
            ("", QueueGroupValidateError::Empty),
            (
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                QueueGroupValidateError::TooLong,
            ),
            ("importer ", QueueGroupValidateError::IllegalCharacter),
            ("importer .thing", QueueGroupValidateError::IllegalCharacter),
            (" importer", QueueGroupValidateError::IllegalCharacter),
            ("importer.thing ", QueueGroupValidateError::IllegalCharacter),
            (
                "importer.thing.works ",
                QueueGroupValidateError::IllegalCharacter,
            ),
            (
                "importer.thing.works\r",
                QueueGroupValidateError::IllegalCharacter,
            ),
            (
                "importer.thing.works\n",
                QueueGroupValidateError::IllegalCharacter,
            ),
            (
                "importer.thing.works\t",
                QueueGroupValidateError::IllegalCharacter,
            ),
            (
                "importer.thi ng.works",
                QueueGroupValidateError::IllegalCharacter,
            ),
            (
                "importer.thi\rng.works",
                QueueGroupValidateError::IllegalCharacter,
            ),
            (
                "importer.thi\nng.works",
                QueueGroupValidateError::IllegalCharacter,
            ),
            (
                "importer.thi\tng.works",
                QueueGroupValidateError::IllegalCharacter,
            ),
            (
                "importer.thing .works",
                QueueGroupValidateError::IllegalCharacter,
            ),
            (
                "importer.thing\r.works",
                QueueGroupValidateError::IllegalCharacter,
            ),
            (
                "importer.thing\n.works",
                QueueGroupValidateError::IllegalCharacter,
            ),
            (
                "importer.thing\t.works",
                QueueGroupValidateError::IllegalCharacter,
            ),
            (" ", QueueGroupValidateError::IllegalCharacter),
            ("\r", QueueGroupValidateError::IllegalCharacter),
            ("\n", QueueGroupValidateError::IllegalCharacter),
            ("\t", QueueGroupValidateError::IllegalCharacter),
        ];
        for (queue_group, expected_err) in queue_groups {
            let err = QueueGroup::try_from(ByteString::from_static(queue_group)).unwrap_err();
            assert_eq!(expected_err, err);
        }
    }
}
