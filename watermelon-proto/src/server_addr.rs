use alloc::{str::FromStr, string::String};
use core::{
    fmt::{self, Debug, Display, Write},
    net::IpAddr,
    ops::Deref,
};

use bytestring::ByteString;
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, percent_encode};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use url::Url;

/// Address of a NATS server
#[derive(Clone, PartialEq, Eq)]
pub struct ServerAddr {
    protocol: Protocol,
    transport: Transport,
    host: Host,
    port: u16,
    username: ByteString,
    password: ByteString,
}

/// The protocol of the NATS server
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Protocol {
    /// Plaintext with the option to later upgrade to TLS
    ///
    /// This option should only be used when esplicity wanting to
    /// connect using a plaintext connection. Using this option
    /// over the public internet or other untrusted networks
    /// leaves the client open to MITM attacks.
    ///
    /// Corresponds to the `nats` scheme.
    PossiblyPlain,
    /// TLS connection
    ///
    /// Requires the TCP connection to successfully upgrade to TLS.
    ///
    /// Corresponds to the `tls` scheme.
    TLS,
}

/// The transport protocol of the NATS server
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Transport {
    /// Transmit data over a TCP stream
    TCP,
    /// Transmit data over WebSocket frames
    Websocket,
}

/// The hostname of the NATS server
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    /// An IPv4 or IPv6 address
    Ip(IpAddr),
    /// A DNS hostname
    Dns(ByteString),
}

impl ServerAddr {
    /// Get the connection protocol
    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// Get the transport protocol
    pub fn transport(&self) -> Transport {
        self.transport
    }

    /// Get the hostname
    pub fn host(&self) -> &Host {
        &self.host
    }

    /// Get the port
    pub fn port(&self) -> u16 {
        self.port
    }

    fn is_default_port(&self) -> bool {
        self.port == protocol_transport_to_port(self.protocol, self.transport)
    }

    /// Get the username
    pub fn username(&self) -> Option<&str> {
        if self.username.is_empty() {
            None
        } else {
            Some(&self.username)
        }
    }

    /// Get the password
    pub fn password(&self) -> Option<&str> {
        if self.password.is_empty() {
            None
        } else {
            Some(&self.password)
        }
    }
}

impl FromStr for ServerAddr {
    type Err = ServerAddrError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let url = value.parse::<Url>().map_err(ServerAddrError::InvalidUrl)?;

        let (protocol, transport) = match url.scheme() {
            "nats" => (Protocol::PossiblyPlain, Transport::TCP),
            "tls" => (Protocol::TLS, Transport::TCP),
            "ws" => (Protocol::PossiblyPlain, Transport::Websocket),
            "wss" => (Protocol::TLS, Transport::Websocket),
            _ => return Err(ServerAddrError::InvalidScheme),
        };

        let host = match url.host() {
            Some(url::Host::Ipv4(addr)) => Host::Ip(IpAddr::V4(addr)),
            Some(url::Host::Ipv6(addr)) => Host::Ip(IpAddr::V6(addr)),
            Some(url::Host::Domain(host)) => {
                // TODO: this shouldn't be necessary
                let host = host
                    .strip_prefix('[')
                    .and_then(|host| host.strip_suffix(']'))
                    .unwrap_or(host);
                match host.parse::<IpAddr>() {
                    Ok(ip) => Host::Ip(ip),
                    Err(_) => Host::Dns(host.into()),
                }
            }
            None => return Err(ServerAddrError::MissingHost),
        };

        let port = if let Some(port) = url.port() {
            port
        } else {
            protocol_transport_to_port(protocol, transport)
        };

        let username = percent_decode_str(url.username())
            .decode_utf8()
            .map_err(|_| ServerAddrError::UsernameInvalidUtf8)?
            .deref()
            .into();
        let password = percent_decode_str(url.password().unwrap_or_default())
            .decode_utf8()
            .map_err(|_| ServerAddrError::PasswordInvalidUtf8)?
            .deref()
            .into();

        Ok(Self {
            protocol,
            transport,
            host,
            port,
            username,
            password,
        })
    }
}

impl Debug for ServerAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let username = if self.username.is_empty() {
            "<none>"
        } else {
            "<redacted>"
        };
        let password = if self.password.is_empty() {
            "<none>"
        } else {
            "<redacted>"
        };
        f.debug_struct("ServerAddr")
            .field("protocol", &self.protocol)
            .field("transport", &self.transport)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &username)
            .field("password", &password)
            .finish()
    }
}

impl Display for ServerAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match (self.protocol, self.transport) {
            (Protocol::PossiblyPlain, Transport::TCP) => "nats",
            (Protocol::TLS, Transport::TCP) => "tls",
            (Protocol::PossiblyPlain, Transport::Websocket) => "ws",
            (Protocol::TLS, Transport::Websocket) => "wss",
        })?;
        f.write_str("://")?;

        if let Some(username) = self.username() {
            Display::fmt(&percent_encode(username.as_bytes(), NON_ALPHANUMERIC), f)?;

            if let Some(password) = self.password() {
                write!(
                    f,
                    ":{}",
                    percent_encode(password.as_bytes(), NON_ALPHANUMERIC)
                )?;
            }
            f.write_char('@')?;
        }

        match &self.host {
            Host::Ip(IpAddr::V4(addr)) => Display::fmt(addr, f)?,
            Host::Ip(IpAddr::V6(addr)) => write!(f, "[{addr}]")?,
            Host::Dns(record) => Display::fmt(record, f)?,
        }
        if !self.is_default_port() {
            write!(f, ":{}", self.port)?;
        }

        Ok(())
    }
}

impl<'de> Deserialize<'de> for ServerAddr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let val = String::deserialize(deserializer)?;
        val.parse().map_err(de::Error::custom)
    }
}

impl Serialize for ServerAddr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

/// An error encountered while parsing [`ServerAddr`]
#[derive(Debug, thiserror::Error)]
pub enum ServerAddrError {
    /// The Url could not be parsed
    #[error("invalid Url")]
    InvalidUrl(#[source] url::ParseError),
    /// The Url has a bad scheme
    #[error("invalid Url scheme")]
    InvalidScheme,
    /// The Url is missing the hostname
    #[error("missing host")]
    MissingHost,
    /// The Url contains a non-utf8 username
    #[error("username is not utf-8")]
    UsernameInvalidUtf8,
    /// The Url contains a non-utf8 password
    #[error("password is not utf-8")]
    PasswordInvalidUtf8,
}

fn protocol_transport_to_port(protocol: Protocol, transport: Transport) -> u16 {
    match (protocol, transport) {
        (Protocol::PossiblyPlain | Protocol::TLS, Transport::TCP) => 4222,
        (Protocol::PossiblyPlain, Transport::Websocket) => 80,
        (Protocol::TLS, Transport::Websocket) => 443,
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use core::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::{Host, Protocol, ServerAddr, Transport};

    #[test]
    fn nats() {
        let server_addr = "nats://127.0.0.1".parse::<ServerAddr>().unwrap();
        assert_eq!(server_addr.transport(), Transport::TCP);
        assert_eq!(server_addr.protocol(), Protocol::PossiblyPlain);
        assert_eq!(
            server_addr.host(),
            &Host::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
        );
        assert_eq!(server_addr.port(), 4222);
        assert_eq!(server_addr.username(), None);
        assert_eq!(server_addr.password(), None);
        assert_eq!(server_addr.to_string(), "nats://127.0.0.1");
    }

    #[test]
    fn nats_non_default_port() {
        let server_addr = "nats://127.0.0.1:4321".parse::<ServerAddr>().unwrap();
        assert_eq!(server_addr.transport(), Transport::TCP);
        assert_eq!(server_addr.protocol(), Protocol::PossiblyPlain);
        assert_eq!(
            server_addr.host(),
            &Host::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
        );
        assert_eq!(server_addr.port(), 4321);
        assert_eq!(server_addr.username(), None);
        assert_eq!(server_addr.password(), None);
        assert_eq!(server_addr.to_string(), "nats://127.0.0.1:4321");
    }

    #[test]
    fn nats_ipv6() {
        let server_addr = "nats://[::1]".parse::<ServerAddr>().unwrap();
        assert_eq!(server_addr.transport(), Transport::TCP);
        assert_eq!(server_addr.protocol(), Protocol::PossiblyPlain);
        assert_eq!(
            server_addr.host(),
            &Host::Ip(IpAddr::V6(Ipv6Addr::LOCALHOST))
        );
        assert_eq!(server_addr.port(), 4222);
        assert_eq!(server_addr.username(), None);
        assert_eq!(server_addr.password(), None);
        assert_eq!(server_addr.to_string(), "nats://[::1]");
    }

    #[test]
    fn tls() {
        let server_addr = "tls://127.0.0.1".parse::<ServerAddr>().unwrap();
        assert_eq!(server_addr.transport(), Transport::TCP);
        assert_eq!(server_addr.protocol(), Protocol::TLS);
        assert_eq!(
            server_addr.host(),
            &Host::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
        );
        assert_eq!(server_addr.port(), 4222);
        assert_eq!(server_addr.username(), None);
        assert_eq!(server_addr.password(), None);
        assert_eq!(server_addr.to_string(), "tls://127.0.0.1");
    }

    #[test]
    fn ws() {
        let server_addr = "ws://127.0.0.1".parse::<ServerAddr>().unwrap();
        assert_eq!(server_addr.transport(), Transport::Websocket);
        assert_eq!(server_addr.protocol(), Protocol::PossiblyPlain);
        assert_eq!(
            server_addr.host(),
            &Host::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
        );
        assert_eq!(server_addr.port(), 80);
        assert_eq!(server_addr.username(), None);
        assert_eq!(server_addr.password(), None);
        assert_eq!(server_addr.to_string(), "ws://127.0.0.1");
    }

    #[test]
    fn wss() {
        let server_addr = "wss://127.0.0.1".parse::<ServerAddr>().unwrap();
        assert_eq!(server_addr.transport(), Transport::Websocket);
        assert_eq!(server_addr.protocol(), Protocol::TLS);
        assert_eq!(
            server_addr.host(),
            &Host::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
        );
        assert_eq!(server_addr.port(), 443);
        assert_eq!(server_addr.username(), None);
        assert_eq!(server_addr.password(), None);
        assert_eq!(server_addr.to_string(), "wss://127.0.0.1");
    }
}
