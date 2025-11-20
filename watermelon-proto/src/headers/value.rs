use alloc::string::String;
use core::{
    fmt::{self, Display},
    ops::Deref,
};

use bytestring::ByteString;

/// A string that can be used to represent an header value
///
/// `HeaderValue` contains a string that is guaranteed [^1] to
/// contain a valid header value that meets the following requirements:
///
/// * The value is not empty
/// * The value has a length less than or equal to 1024 [^2]
/// * The value does not contain any whitespace characters
///
/// `HeaderValue` can be constructed from [`HeaderValue::from_static`]
/// or any of the `TryFrom` implementations.
///
/// [^1]: Because [`HeaderValue::from_dangerous_value`] is safe to call,
///       unsafe code must not assume any of the above invariants.
/// [^2]: Messages coming from the NATS server are allowed to violate this rule.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct HeaderValue(ByteString);

impl HeaderValue {
    /// Construct `HeaderValue` from a static string
    ///
    /// # Panics
    ///
    /// Will panic if `value` isn't a valid `HeaderValue`
    #[must_use]
    pub fn from_static(value: &'static str) -> Self {
        Self::try_from(ByteString::from_static(value)).expect("invalid HeaderValue")
    }

    /// Construct a `HeaderValue` from a string, without checking invariants
    ///
    /// This method bypasses invariants checks implemented by [`HeaderValue::from_static`]
    /// and all `TryFrom` implementations.
    ///
    /// # Security
    ///
    /// While calling this method can eliminate the runtime performance cost of
    /// checking the string, constructing `HeaderValue` with an invalid string and
    /// then calling the NATS server with it can cause serious security issues.
    /// When in doubt use the [`HeaderValue::from_static`] or any of the `TryFrom`
    /// implementations.
    #[must_use]
    #[expect(
        clippy::missing_panics_doc,
        reason = "The header validation is only made in debug"
    )]
    pub fn from_dangerous_value(value: ByteString) -> Self {
        if cfg!(debug_assertions)
            && let Err(err) = validate_header_value(&value)
        {
            panic!("HeaderValue {value:?} isn't valid {err:?}");
        }
        Self(value)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for HeaderValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl TryFrom<ByteString> for HeaderValue {
    type Error = HeaderValueValidateError;

    fn try_from(value: ByteString) -> Result<Self, Self::Error> {
        validate_header_value(&value)?;
        Ok(Self::from_dangerous_value(value))
    }
}

impl TryFrom<String> for HeaderValue {
    type Error = HeaderValueValidateError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_header_value(&value)?;
        Ok(Self::from_dangerous_value(value.into()))
    }
}

impl From<HeaderValue> for ByteString {
    fn from(value: HeaderValue) -> Self {
        value.0
    }
}

impl AsRef<[u8]> for HeaderValue {
    fn as_ref(&self) -> &[u8] {
        self.as_str().as_bytes()
    }
}

impl AsRef<str> for HeaderValue {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Deref for HeaderValue {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

/// An error encountered while validating [`HeaderValue`]
#[derive(Debug, thiserror::Error)]
pub enum HeaderValueValidateError {
    /// The value is empty
    #[error("HeaderValue is empty")]
    Empty,
    /// The value has a length greater than 64
    #[error("HeaderValue is too long")]
    TooLong,
    /// The value contains an Unicode whitespace character
    #[error("HeaderValue contained an illegal whitespace character")]
    IllegalCharacter,
}

fn validate_header_value(header_value: &str) -> Result<(), HeaderValueValidateError> {
    if header_value.is_empty() {
        return Err(HeaderValueValidateError::Empty);
    }

    if header_value.len() > 1024 {
        // This is an arbitrary limit, but I guess the server must also have one
        return Err(HeaderValueValidateError::TooLong);
    }

    if header_value.chars().any(char::is_whitespace) {
        // The theoretical security limit is just ` `, `\t`, `\r` and `\n`.
        // Let's be more careful.
        return Err(HeaderValueValidateError::IllegalCharacter);
    }

    Ok(())
}
