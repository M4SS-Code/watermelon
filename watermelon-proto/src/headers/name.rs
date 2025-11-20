use alloc::string::String;
use core::{
    fmt::{self, Display},
    ops::Deref,
};
use unicase::UniCase;

use bytestring::ByteString;

/// A string that can be used to represent an header name
///
/// `HeaderName` contains a string that is guaranteed [^1] to
/// contain a valid header name that meets the following requirements:
///
/// * The value is not empty
/// * The value has a length less than or equal to 64 [^2]
/// * The value does not contain any whitespace characters or `:`
///
/// `HeaderName` can be constructed from [`HeaderName::from_static`]
/// or any of the `TryFrom` implementations.
///
/// [^1]: Because [`HeaderName::from_dangerous_value`] is safe to call,
///       unsafe code must not assume any of the above invariants.
/// [^2]: Messages coming from the NATS server are allowed to violate this rule.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct HeaderName(UniCase<ByteString>);

impl HeaderName {
    /// Client-defined unique identifier for a message that will be used by the server apply de-duplication within the configured Jetstream _Duplicate Window_
    pub const MESSAGE_ID: Self = Self::new_internal("Nats-Msg-Id");
    /// Have Jetstream assert that the published message is received by the expected stream
    pub const EXPECTED_STREAM: Self = Self::new_internal("Nats-Expected-Stream");
    /// Have Jetstream assert that the last expected [`HeaderName::MESSAGE_ID`] matches this ID
    pub const EXPECTED_LAST_MESSAGE_ID: Self = Self::new_internal("Nats-Expected-Last-Msg-Id");
    /// Have Jetstream assert that the last sequence ID matches this ID
    pub const EXPECTED_LAST_SEQUENCE: Self = Self::new_internal("Nats-Expected-Last-Sequence");
    /// Purge all prior messages in the stream (`all` value) or at the subject-level (`sub` value)
    pub const ROLLUP: Self = Self::new_internal("Nats-Rollup");

    /// Name of the stream the message was republished from
    pub const STREAM: Self = Self::new_internal("Nats-Stream");
    /// Original subject to which the message was republished from
    pub const SUBJECT: Self = Self::new_internal("Nats-Subject");
    /// Original sequence ID the message was republished from
    pub const SEQUENCE: Self = Self::new_internal("Nats-Sequence");
    /// Last sequence ID of the message having the same subject, or zero if this is the first message for the subject
    pub const LAST_SEQUENCE: Self = Self::new_internal("Nats-Last-Sequence");
    /// The original RFC3339 timestamp of the message
    pub const TIMESTAMP: Self = Self::new_internal("Nats-Time-Stamp");

    /// Origin stream name, subject, sequence number, subject filter and destination transform of the message being sourced
    pub const STREAM_SOURCE: Self = Self::new_internal("Nats-Stream-Source");

    /// Size of the message payload in bytes for an headers-only message
    pub const MESSAGE_SIZE: Self = Self::new_internal("Nats-Msg-Size");

    /// Construct `HeaderName` from a static string
    ///
    /// # Panics
    ///
    /// Will panic if `value` isn't a valid `HeaderName`
    #[must_use]
    pub fn from_static(value: &'static str) -> Self {
        Self::try_from(ByteString::from_static(value)).expect("invalid HeaderName")
    }

    /// Construct a `HeaderName` from a string, without checking invariants
    ///
    /// This method bypasses invariants checks implemented by [`HeaderName::from_static`]
    /// and all `TryFrom` implementations.
    ///
    /// # Security
    ///
    /// While calling this method can eliminate the runtime performance cost of
    /// checking the string, constructing `HeaderName` with an invalid string and
    /// then calling the NATS server with it can cause serious security issues.
    /// When in doubt use the [`HeaderName::from_static`] or any of the `TryFrom`
    /// implementations.
    #[expect(
        clippy::missing_panics_doc,
        reason = "The header validation is only made in debug"
    )]
    #[must_use]
    pub fn from_dangerous_value(value: ByteString) -> Self {
        if cfg!(debug_assertions)
            && let Err(err) = validate_header_name(&value)
        {
            panic!("HeaderName {value:?} isn't valid {err:?}");
        }
        Self(UniCase::new(value))
    }

    const fn new_internal(value: &'static str) -> Self {
        if value.is_ascii() {
            Self(UniCase::ascii(ByteString::from_static(value)))
        } else {
            Self(UniCase::unicode(ByteString::from_static(value)))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for HeaderName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl TryFrom<ByteString> for HeaderName {
    type Error = HeaderNameValidateError;

    fn try_from(value: ByteString) -> Result<Self, Self::Error> {
        validate_header_name(&value)?;
        Ok(Self::from_dangerous_value(value))
    }
}

impl TryFrom<String> for HeaderName {
    type Error = HeaderNameValidateError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_header_name(&value)?;
        Ok(Self::from_dangerous_value(value.into()))
    }
}

impl From<HeaderName> for ByteString {
    fn from(value: HeaderName) -> Self {
        value.0.into_inner()
    }
}

impl AsRef<[u8]> for HeaderName {
    fn as_ref(&self) -> &[u8] {
        self.as_str().as_bytes()
    }
}

impl AsRef<str> for HeaderName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Deref for HeaderName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

/// An error encountered while validating [`HeaderName`]
#[derive(Debug, thiserror::Error)]
pub enum HeaderNameValidateError {
    /// The value is empty
    #[error("HeaderName is empty")]
    Empty,
    /// The value has a length greater than 64
    #[error("HeaderName is too long")]
    TooLong,
    /// The value contains an Unicode whitespace character or `:`
    #[error("HeaderName contained an illegal whitespace character")]
    IllegalCharacter,
}

fn validate_header_name(header_name: &str) -> Result<(), HeaderNameValidateError> {
    if header_name.is_empty() {
        return Err(HeaderNameValidateError::Empty);
    }

    if header_name.len() > 64 {
        // This is an arbitrary limit, but I guess the server must also have one
        return Err(HeaderNameValidateError::TooLong);
    }

    if header_name.chars().any(|c| c.is_whitespace() || c == ':') {
        // The theoretical security limit is just ` `, `\t`, `\r`, `\n` and `:`.
        // Let's be more careful.
        return Err(HeaderNameValidateError::IllegalCharacter);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use core::cmp::Ordering;

    use super::HeaderName;

    #[test]
    fn eq() {
        let cased = HeaderName::from_static("Nats-Message-Id");
        let lowercase = HeaderName::from_static("nats-message-id");
        assert_eq!(cased, lowercase);
        assert_eq!(cased.cmp(&lowercase), Ordering::Equal);
    }
}
