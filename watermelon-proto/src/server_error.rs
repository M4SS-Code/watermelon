use bytestring::ByteString;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ServerError {
    #[error("subject is invalid")]
    InvalidSubject,
    #[error("permissions violation for publish")]
    PublishPermissionViolation,
    #[error("permissions violation for subscription")]
    SubscribePermissionViolation,

    #[error("unknown protocol operation")]
    UnknownProtocolOperation,

    #[error("attempted to connect to route port")]
    ConnectionAttemptedToWrongPort,

    #[error("authorization violation")]
    AuthorizationViolation,
    #[error("authorization timeout")]
    AuthorizationTimeout,
    #[error("invalid client protocol")]
    InvalidClientProtocol,
    #[error("maximum control line exceeded")]
    MaximumControlLineExceeded,
    #[error("parser error")]
    ParseError,
    #[error("secure connection, tls required")]
    TlsRequired,
    #[error("stale connection")]
    StaleConnection,
    #[error("maximum connections exceeded")]
    MaximumConnectionsExceeded,
    #[error("slow consumer")]
    SlowConsumer,
    #[error("maximum payload violation")]
    MaximumPayloadViolation,

    #[error("unknown error: {raw_message}")]
    Other { raw_message: ByteString },
}

impl ServerError {
    pub fn is_fatal(&self) -> Option<bool> {
        match self {
            Self::InvalidSubject
            | Self::PublishPermissionViolation
            | Self::SubscribePermissionViolation => Some(false),

            Self::UnknownProtocolOperation
            | Self::ConnectionAttemptedToWrongPort
            | Self::AuthorizationViolation
            | Self::AuthorizationTimeout
            | Self::InvalidClientProtocol
            | Self::MaximumControlLineExceeded
            | Self::ParseError
            | Self::TlsRequired
            | Self::StaleConnection
            | Self::MaximumConnectionsExceeded
            | Self::SlowConsumer
            | Self::MaximumPayloadViolation => Some(true),

            Self::Other { .. } => None,
        }
    }

    pub(crate) fn parse(raw_message: ByteString) -> Self {
        const PUBLISH_PERMISSIONS: &[u8] = b"Permissions Violation for Publish";
        const SUBSCRIPTION_PERMISSIONS: &[u8] = b"Permissions Violation for Subscription";

        // The message comes from the server, so the permission prefixes are
        // compared byte-wise: a multi-byte character in the prefix region is
        // just a byte mismatch, i.e. a non-match (`Other` below) — `&str`
        // indexing at these offsets would panic instead.
        let m = raw_message.trim();
        let bytes = m.as_bytes();
        let matches_prefix = |needle: &[u8]| {
            bytes.len() > needle.len()
                && bytes[..needle.len()]
                    .iter()
                    .zip(needle)
                    .all(|(a, b)| a.eq_ignore_ascii_case(b))
        };

        if m.eq_ignore_ascii_case("Invalid Subject") {
            Self::InvalidSubject
        } else if matches_prefix(PUBLISH_PERMISSIONS) {
            Self::PublishPermissionViolation
        } else if matches_prefix(SUBSCRIPTION_PERMISSIONS) {
            Self::SubscribePermissionViolation
        } else if m.eq_ignore_ascii_case("Unknown Protocol Operation") {
            Self::UnknownProtocolOperation
        } else if m.eq_ignore_ascii_case("Attempted To Connect To Route Port") {
            Self::ConnectionAttemptedToWrongPort
        } else if m.eq_ignore_ascii_case("Authorization Violation") {
            Self::AuthorizationViolation
        } else if m.eq_ignore_ascii_case("Authorization Timeout") {
            Self::AuthorizationTimeout
        } else if m.eq_ignore_ascii_case("Invalid Client Protocol") {
            Self::InvalidClientProtocol
        } else if m.eq_ignore_ascii_case("Maximum Control Line Exceeded") {
            Self::MaximumControlLineExceeded
        } else if m.eq_ignore_ascii_case("Parser Error") {
            Self::ParseError
        } else if m.eq_ignore_ascii_case("Secure Connection - TLS Required") {
            Self::TlsRequired
        } else if m.eq_ignore_ascii_case("Stale Connection") {
            Self::StaleConnection
        } else if m.eq_ignore_ascii_case("Maximum Connections Exceeded") {
            Self::MaximumConnectionsExceeded
        } else if m.eq_ignore_ascii_case("Slow Consumer") {
            Self::SlowConsumer
        } else if m.eq_ignore_ascii_case("Maximum Payload Violation") {
            Self::MaximumPayloadViolation
        } else {
            Self::Other { raw_message }
        }
    }
}

#[cfg(test)]
mod tests {
    use bytestring::ByteString;

    use super::ServerError;

    #[test]
    fn parse_permission_violations() {
        assert_eq!(
            ServerError::parse(ByteString::from_static(
                "Permissions Violation for Publish for subject \"a.b\"",
            )),
            ServerError::PublishPermissionViolation,
        );
        assert_eq!(
            ServerError::parse(ByteString::from_static(
                "Permissions Violation for Subscription for subject \"a.b\"",
            )),
            ServerError::SubscribePermissionViolation,
        );
    }

    // A multi-byte character straddling the prefix length must not panic on
    // byte indexing but be reported as an unknown error.
    #[test]
    fn parse_prefix_straddling_char_boundary() {
        // `€` occupies bytes 32..35, so the 33-byte `Publish` prefix ends
        // mid-character.
        let msg = format!("{}€b", "a".repeat(32));
        assert_eq!(
            ServerError::parse(msg.clone().into()),
            ServerError::Other {
                raw_message: msg.clone().into(),
            },
        );

        // Same for the 38-byte `Subscription` prefix.
        let msg = format!("{}€b", "a".repeat(37));
        assert_eq!(
            ServerError::parse(msg.clone().into()),
            ServerError::Other {
                raw_message: msg.into(),
            },
        );
    }
}
