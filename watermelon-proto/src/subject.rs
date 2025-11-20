use alloc::string::String;
use core::{
    fmt::{self, Display},
    ops::Deref,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use bytestring::ByteString;

/// A string that can be used to represent a subject
///
/// `Subject` contains a string that is guaranteed [^1] to
/// contain a valid subject that meets the following requirements:
///
/// * The value is not empty
/// * The value has a length less than or equal to 256 [^2]
/// * The value does not contain any whitespace characters or `:`
/// * The value does not contain wrongly placed `*` or `>` characters
///
/// `Subject` can be constructed from [`Subject::from_static`]
/// or any of the `TryFrom` implementations.
///
/// [^1]: Because [`Subject::from_dangerous_value`] is safe to call,
///       unsafe code must not assume any of the above invariants.
/// [^2]: Messages coming from the NATS server are allowed to violate this rule.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Subject(ByteString);

impl Subject {
    /// Construct `Subject` from a static string
    ///
    /// # Panics
    ///
    /// Will panic if `value` isn't a valid `Subject`
    #[must_use]
    pub fn from_static(value: &'static str) -> Self {
        Self::try_from(ByteString::from_static(value)).expect("invalid Subject")
    }

    /// Construct a `Subject` from a string, without checking invariants
    ///
    /// This method bypasses invariants checks implemented by [`Subject::from_static`]
    /// and all `TryFrom` implementations.
    ///
    /// # Security
    ///
    /// While calling this method can eliminate the runtime performance cost of
    /// checking the string, constructing `Subject` with an invalid string and
    /// then calling the NATS server with it can cause serious security issues.
    /// When in doubt use the [`Subject::from_static`] or any of the `TryFrom`
    /// implementations.
    #[expect(
        clippy::missing_panics_doc,
        reason = "The subject validation is only made in debug"
    )]
    #[must_use]
    pub fn from_dangerous_value(value: ByteString) -> Self {
        if cfg!(debug_assertions)
            && let Err(err) = validate_subject(&value)
        {
            panic!("Subject {value:?} isn't valid {err:?}");
        }
        Self(value)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl TryFrom<ByteString> for Subject {
    type Error = SubjectValidateError;

    fn try_from(value: ByteString) -> Result<Self, Self::Error> {
        validate_subject(&value)?;
        Ok(Self::from_dangerous_value(value))
    }
}

impl TryFrom<String> for Subject {
    type Error = SubjectValidateError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_subject(&value)?;
        Ok(Self::from_dangerous_value(value.into()))
    }
}

impl From<Subject> for ByteString {
    fn from(value: Subject) -> Self {
        value.0
    }
}

impl AsRef<[u8]> for Subject {
    fn as_ref(&self) -> &[u8] {
        self.as_str().as_bytes()
    }
}

impl AsRef<str> for Subject {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Deref for Subject {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl Serialize for Subject {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_str().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Subject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = ByteString::deserialize(deserializer)?;
        s.try_into().map_err(de::Error::custom)
    }
}

/// An error encountered while validating [`Subject`]
#[derive(Debug, thiserror::Error)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub enum SubjectValidateError {
    /// The value is empty
    #[error("Subject is empty")]
    Empty,
    /// The value has a length greater than 256
    #[error("Subject is too long")]
    TooLong,
    /// The value contains an Unicode whitespace character
    #[error("Subject contained an illegal whitespace character")]
    IllegalCharacter,
    /// The value contains consecutive `.` characters
    #[error("Subject contained a broken token")]
    BrokenToken,
    /// The value contains `.` or `>` together with other characters
    /// in the same token, or the `>` is in the non-last token
    #[error("Subject contained a broken wildcard")]
    BrokenWildcard,
}

fn validate_subject(subject: &str) -> Result<(), SubjectValidateError> {
    if subject.is_empty() {
        return Err(SubjectValidateError::Empty);
    }

    if subject.len() > 256 {
        // This is an arbitrary limit, but I guess the server must also have one
        return Err(SubjectValidateError::TooLong);
    }

    if subject.chars().any(char::is_whitespace) {
        // The theoretical security limit is just ` `, `\t`, `\r` and `\n`.
        // Let's be more careful.
        return Err(SubjectValidateError::IllegalCharacter);
    }

    let mut tokens = subject.split('.').peekable();
    while let Some(token) = tokens.next() {
        if token.is_empty() || token.contains("..") {
            return Err(SubjectValidateError::BrokenToken);
        }

        if token.len() > 1 && (token.contains(['*', '>'])) {
            return Err(SubjectValidateError::BrokenWildcard);
        }

        if token == ">" && tokens.peek().is_some() {
            return Err(SubjectValidateError::BrokenWildcard);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use bytestring::ByteString;

    use super::{Subject, SubjectValidateError};

    #[test]
    fn valid_subjects() {
        let subjects = [
            "cmd",
            "cmd.endpoint",
            "cmd.endpoint.detail",
            "cmd.*.detail",
            "cmd.*.*",
            "cmd.endpoint.>",
        ];
        for subject in subjects {
            let s = Subject::try_from(ByteString::from_static(subject)).unwrap();
            assert_eq!(subject, s.as_str());
        }
    }

    #[test]
    fn invalid_subjects() {
        let subjects = [
            ("", SubjectValidateError::Empty),
            (
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                SubjectValidateError::TooLong,
            ),
            ("cmd ", SubjectValidateError::IllegalCharacter),
            ("cmd .endpoint", SubjectValidateError::IllegalCharacter),
            (" cmd", SubjectValidateError::IllegalCharacter),
            ("cmd.endpoint ", SubjectValidateError::IllegalCharacter),
            (
                "cmd.endpoint.detail ",
                SubjectValidateError::IllegalCharacter,
            ),
            (
                "cmd.endpoint.detail\r",
                SubjectValidateError::IllegalCharacter,
            ),
            (
                "cmd.endpoint.detail\n",
                SubjectValidateError::IllegalCharacter,
            ),
            (
                "cmd.endpoint.detail\t",
                SubjectValidateError::IllegalCharacter,
            ),
            (
                "cmd.endp oint.detail",
                SubjectValidateError::IllegalCharacter,
            ),
            (
                "cmd.endp\roint.detail",
                SubjectValidateError::IllegalCharacter,
            ),
            (
                "cmd.endp\noint.detail",
                SubjectValidateError::IllegalCharacter,
            ),
            (
                "cmd.endp\toint.detail",
                SubjectValidateError::IllegalCharacter,
            ),
            (
                "cmd.endpoint .detail",
                SubjectValidateError::IllegalCharacter,
            ),
            (
                "cmd.endpoint\r.detail",
                SubjectValidateError::IllegalCharacter,
            ),
            (
                "cmd.endpoint\n.detail",
                SubjectValidateError::IllegalCharacter,
            ),
            (
                "cmd.endpoint\t.detail",
                SubjectValidateError::IllegalCharacter,
            ),
            (" ", SubjectValidateError::IllegalCharacter),
            ("\r", SubjectValidateError::IllegalCharacter),
            ("\n", SubjectValidateError::IllegalCharacter),
            ("\t", SubjectValidateError::IllegalCharacter),
            ("cmd..endpoint", SubjectValidateError::BrokenToken),
            (".cmd.endpoint", SubjectValidateError::BrokenToken),
            ("cmd.endpoint.", SubjectValidateError::BrokenToken),
            ("cmd.**", SubjectValidateError::BrokenWildcard),
            ("cmd.**.endpoint", SubjectValidateError::BrokenWildcard),
            ("cmd.a*.endpoint", SubjectValidateError::BrokenWildcard),
            ("cmd.*a.endpoint", SubjectValidateError::BrokenWildcard),
            ("cmd.>.endpoint", SubjectValidateError::BrokenWildcard),
            ("cmd.a>.endpoint", SubjectValidateError::BrokenWildcard),
            ("cmd.endpoint.a>", SubjectValidateError::BrokenWildcard),
            ("cmd.endpoint.>a", SubjectValidateError::BrokenWildcard),
        ];
        for (subject, expected_err) in subjects {
            let err = Subject::try_from(ByteString::from_static(subject)).unwrap_err();
            assert_eq!(expected_err, err);
        }
    }
}
