use alloc::string::String;
use core::{
    fmt::{self, Display},
    str::FromStr,
};

use bytes::Bytes;
use bytestring::ByteString;

/// A string that can be used to represent an header value
///
/// `HeaderValue` contains raw bytes that are guaranteed [^1] to
/// contain a valid header value that meets the following requirements:
///
/// * The value does not contain `\r` or `\n` bytes
///
/// The value may be empty and may contain arbitrary non-UTF-8 bytes:
/// the NATS server relays header values verbatim, and other clients
/// (like `nats.go`) are able to produce such values.
///
/// `HeaderValue` can be constructed from [`HeaderValue::from_static`],
/// [`HeaderValue::from_bytes`], or any of the `TryFrom` implementations.
///
/// [^1]: Because [`HeaderValue::from_dangerous_value`] is safe to call,
///       unsafe code must not assume any of the above invariants.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct HeaderValue {
    inner: Bytes,
}

impl fmt::Debug for HeaderValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("HeaderValue").field(&self.inner).finish()
    }
}

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

    /// Construct a `HeaderValue` from raw bytes, validating the value
    ///
    /// # Errors
    ///
    /// Returns an error if `value` contains `\r` or `\n`.
    pub fn from_bytes(value: &[u8]) -> Result<Self, HeaderValueValidateError> {
        validate_header_value_bytes(value)?;
        Ok(Self::from_dangerous_value(Bytes::copy_from_slice(value)))
    }

    /// Construct a `HeaderValue` from a `Bytes`, without checking invariants
    ///
    /// This method bypasses invariants checks implemented by [`HeaderValue::from_static`],
    /// [`HeaderValue::from_bytes`], and all `TryFrom` implementations.
    ///
    /// # Security
    ///
    /// While calling this method can eliminate the runtime performance cost of
    /// checking the value, constructing `HeaderValue` with an invalid value and
    /// then calling the NATS server with it can cause serious security issues.
    /// When in doubt use [`HeaderValue::from_static`], [`HeaderValue::from_bytes`],
    /// or any of the `TryFrom` implementations.
    #[must_use]
    #[expect(
        clippy::missing_panics_doc,
        reason = "The header validation is only made in debug"
    )]
    pub fn from_dangerous_value(value: Bytes) -> Self {
        if cfg!(debug_assertions) {
            validate_header_value_bytes(&value).expect("invalid HeaderValue");
        }
        Self { inner: value }
    }

    /// Try to view this header value as a `&str`
    ///
    /// # Errors
    ///
    /// Returns [`HeaderValueToStrError`] if the value contains bytes that are
    /// not valid UTF-8.
    pub fn to_str(&self) -> Result<&str, HeaderValueToStrError> {
        core::str::from_utf8(&self.inner).map_err(|_| HeaderValueToStrError { _priv: () })
    }

    /// Returns the raw bytes of the header value
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.inner
    }
}

impl Display for HeaderValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Ok(s) = self.to_str() {
            f.write_str(s)
        } else {
            // Fallback: show escaped bytes
            for &b in &self.inner {
                if b.is_ascii_graphic() || b == b' ' {
                    write!(f, "{}", b as char)?;
                } else {
                    write!(f, "\\x{b:02x}")?;
                }
            }
            Ok(())
        }
    }
}

impl TryFrom<ByteString> for HeaderValue {
    type Error = HeaderValueValidateError;

    fn try_from(value: ByteString) -> Result<Self, Self::Error> {
        validate_header_value_bytes(value.as_bytes())?;
        Ok(Self::from_dangerous_value(value.into_bytes()))
    }
}

impl FromStr for HeaderValue {
    type Err = HeaderValueValidateError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        validate_header_value_bytes(value.as_bytes())?;
        Ok(Self::from_dangerous_value(Bytes::copy_from_slice(
            value.as_bytes(),
        )))
    }
}

impl TryFrom<String> for HeaderValue {
    type Error = HeaderValueValidateError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_header_value_bytes(value.as_bytes())?;
        Ok(Self::from_dangerous_value(value.into()))
    }
}

impl AsRef<[u8]> for HeaderValue {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// An error returned when a header value contains non-UTF-8 bytes
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("header value contains non-UTF-8 bytes")]
pub struct HeaderValueToStrError {
    _priv: (),
}

/// An error encountered while validating [`HeaderValue`]
#[derive(Debug, thiserror::Error)]
pub enum HeaderValueValidateError {
    /// The value contains `\r` or `\n`
    #[error("HeaderValue contained an illegal character")]
    IllegalCharacter,
}

fn validate_header_value_bytes(value: &[u8]) -> Result<(), HeaderValueValidateError> {
    // The NATS server treats the header block as an opaque blob and relays
    // header values verbatim: any bytes — including obs-text (0x80–0xFF),
    // invalid UTF-8 and empty values — pass through. It also doesn't enforce
    // per-field length limits; the header block as a whole only counts
    // towards the maximum payload size.
    // `\r` and `\n` must still be rejected as they would break the
    // line-based framing of the header block itself.
    for &b in value {
        if b == b'\r' || b == b'\n' {
            return Err(HeaderValueValidateError::IllegalCharacter);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use claims::assert_matches;

    use super::{HeaderValue, HeaderValueValidateError};

    #[test]
    fn valid_header_values() {
        let values = [
            "value",
            "value with spaces",
            "value\twith\ttabs",
            "non-àscíì ütf-8",
        ];
        for value in values {
            let header_value: HeaderValue = value.parse().unwrap();
            assert_eq!(header_value.as_bytes(), value.as_bytes());
            assert_eq!(header_value.to_str().unwrap(), value);
            assert_eq!(header_value.to_string(), value);
        }
    }

    #[test]
    fn empty_header_value() {
        let header_value = HeaderValue::from_bytes(b"").unwrap();
        assert_eq!(header_value.as_bytes(), b"");
        assert_eq!(header_value.to_str().unwrap(), "");
    }

    #[test]
    fn non_utf8_header_value() {
        let header_value = HeaderValue::from_bytes(b"\x00\x80\xffabc").unwrap();
        assert_eq!(header_value.as_bytes(), b"\x00\x80\xffabc");
        assert!(header_value.to_str().is_err());
        assert_eq!(header_value.to_string(), "\\x00\\x80\\xffabc");
    }

    #[test]
    fn invalid_header_values() {
        let values: [&[u8]; 5] = [b"\r", b"\n", b"value\r", b"value\nvalue", b"value\r\n"];
        for value in values {
            assert_matches!(
                HeaderValue::from_bytes(value),
                Err(HeaderValueValidateError::IllegalCharacter),
                "{value:?}"
            );
        }
    }
}
