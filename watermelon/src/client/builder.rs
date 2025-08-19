use std::time::Duration;

use watermelon_mini::AuthenticationMethod;
use watermelon_proto::{ServerAddr, Subject};

#[cfg(feature = "from-env")]
use super::from_env::FromEnv;
use crate::{core::Client, handler::ConnectHandlerError};

/// A builder for [`Client`]
///
/// Obtained from [`Client::builder`].
#[derive(Debug)]
pub struct ClientBuilder {
    pub(crate) tcp_nodelay: bool,
    pub(crate) auth_method: Option<AuthenticationMethod>,
    pub(crate) connect_timeout: Duration,
    pub(crate) write_delay: Duration,
    pub(crate) inbox_prefix: Subject,
    pub(crate) echo: Echo,
    pub(crate) default_response_timeout: Duration,
    #[cfg(feature = "non-standard-zstd")]
    pub(crate) non_standard_zstd_compression_level: Option<u8>,
}

/// Whether or not to allow messages published by this client to be echoed back to it's own subscriptions
#[derive(Debug, Copy, Clone, Default)]
pub enum Echo {
    /// Do not allow messages published by this client to be echoed back to it's own [`Subscription`]s
    ///
    /// [`Subscription`]: crate::core::Subscription
    #[default]
    Prevent,
    /// Allow messages published by this client to be echoed back to it's own [`Subscription`]s
    ///
    /// [`Subscription`]: crate::core::Subscription
    Allow,
}

impl ClientBuilder {
    pub(super) fn new() -> Self {
        Self {
            tcp_nodelay: true,
            auth_method: None,
            connect_timeout: Duration::from_secs(30),
            write_delay: Duration::ZERO,
            inbox_prefix: Subject::from_static("_INBOX"),
            echo: Echo::Prevent,
            default_response_timeout: Duration::from_secs(5),
            #[cfg(feature = "non-standard-zstd")]
            non_standard_zstd_compression_level: Some(3),
        }
    }

    /// Construct [`ClientBuilder`] from environment variables
    ///
    /// Reads the following environment variables into [`ClientBuilder`]:
    ///
    /// Authentication:
    ///
    /// * `NATS_JWT` and `NATS_NKEY`: use nkey authentication
    /// * `NATS_CREDS_FILE`: read JWT and NKEY from the provided `.creds` file
    /// * `NATS_USERNAME` and `NATS_PASSWORD`: use username and password authentication
    ///
    /// # Panics
    ///
    /// It panics if:
    ///
    /// - it is not possible to get the environment variables;
    /// - an error occurs when trying to read the credentials file;
    /// - the credentials file is invalid.
    #[cfg(feature = "from-env")]
    #[must_use]
    pub fn from_env() -> Self {
        use super::from_env;

        let env = envy::from_env::<FromEnv>().expect("FromEnv deserialization error");

        let mut this = Self::new();

        match env.auth {
            from_env::AuthenticationMethod::Creds { jwt, nkey } => {
                this = this.authentication_method(Some(AuthenticationMethod::Creds { jwt, nkey }));
            }
            from_env::AuthenticationMethod::CredsFile { creds_file } => {
                let contents = std::fs::read_to_string(creds_file).expect("read credentials file");
                let auth =
                    AuthenticationMethod::from_creds(&contents).expect("parse credentials file");
                this = this.authentication_method(Some(auth));
            }
            from_env::AuthenticationMethod::UserAndPassword { username, password } => {
                this = this.authentication_method(Some(AuthenticationMethod::UserAndPassword {
                    username,
                    password,
                }));
            }
            from_env::AuthenticationMethod::None => {
                this = this.authentication_method(None);
            }
        }

        if let Some(inbox_prefix) = env.inbox_prefix {
            this = this.inbox_prefix(inbox_prefix);
        }

        this
    }

    /// Controls the Nagle algorithm for kernel-level bandwidth vs latency optimization
    ///
    /// Setting this to `true` disables Nagle's algorithm. The kernel
    /// will send packets immediately, reducing latency but potentially increasing
    /// bandwidth usage due to smaller but more frequent packets.
    ///
    /// Setting this to `false` enables the Nagle algorithm. The kernel
    /// will delay small writes while unacknowledged packets are in flight,
    /// increasing bandwidth efficiency and latency.
    ///
    /// Default: true
    #[must_use]
    pub fn tcp_nodelay(mut self, tcp_nodelay: bool) -> Self {
        self.tcp_nodelay = tcp_nodelay;
        self
    }

    /// Define an authentication method
    #[must_use]
    pub fn authentication_method(mut self, auth_method: Option<AuthenticationMethod>) -> Self {
        self.auth_method = auth_method;
        self
    }

    /// Define the timeout for TCP connect and handshake
    ///
    /// Default: 30 seconds
    #[must_use]
    pub fn connect_timeout(mut self, connect_timeout: Duration) -> Self {
        self.connect_timeout = connect_timeout;
        self
    }

    /// Define a delay for small writes
    ///
    /// Setting a non-zero delay allows the client to generate larger
    /// TLS and TCP packets at the cost of increased latency. Using
    /// a value greater than a few seconds may break the client in
    /// unexpected ways.
    ///
    /// Compared to [`ClientBuilder::tcp_nodelay`], buffering happens
    /// at the message serialization layer, improving the bandwidth
    /// efficiency of TLS connections and
    /// [`ClientBuilder::non_standard_zstd_compression_level`].
    ///
    /// Setting this to [`Duration::ZERO`] causes the client to send messages
    /// as fast as the network will allow, trading off smaller packets for
    /// lower latency.
    ///
    /// Default: 0
    #[must_use]
    pub fn write_delay(mut self, write_delay: Duration) -> Self {
        self.write_delay = write_delay;
        self
    }

    /// Configure the inbox prefix to which replies from the NATS server will be received
    ///
    /// Default: `_INBOX`
    #[must_use]
    pub fn inbox_prefix(mut self, inbox_prefix: Subject) -> Self {
        self.inbox_prefix = inbox_prefix;
        self
    }

    /// Whether or not to allow messages published by this client to be echoed back to it's own [`Subscription`]s
    ///
    /// Setting this option to [`Echo::Allow`] will allow [`Subscription`]s created by
    /// this client to receive messages by itself published.
    ///
    /// Default: [`Echo::Prevent`].
    ///
    /// [`Subscription`]: crate::core::Subscription
    #[must_use]
    pub fn echo(mut self, echo: Echo) -> Self {
        self.echo = echo;
        self
    }

    /// The default timeout for [`ResponseFut`]
    ///
    /// Defines how long we should wait for a response in [`Client::request`].
    ///
    /// Default: 5 seconds.
    ///
    /// [`ResponseFut`]: crate::core::request::ResponseFut
    #[must_use]
    pub fn default_response_timeout(mut self, timeout: Duration) -> Self {
        self.default_response_timeout = timeout;
        self
    }

    /// Have the client compress the connection using zstd when talking to a NATS server
    /// behind a custom zstd proxy
    ///
    /// The NATS protocol and applications developed on top of it can make inefficient
    /// use of the network, making applications running on extremely slow or expensive internet
    /// connections infeasible. This option adds a non-standard zstd compression
    /// feature on top of the client which, when used in conjunction with a custom zstd reverse proxy
    /// put in front of the NATS server allows for large bandwidth savings.
    ///
    /// `Some(number)` enables compression with the specified compression level. Out-of-range
    /// values are clamped into range. `None` disables compression.
    ///
    /// This option is particularly powerful when combined with [`ClientBuilder::write_delay`].
    ///
    /// This option is automatically disabled when connecting to an unsupported server.
    ///
    /// Default: `3` when compiled with the `non-standard-zstd` option.
    #[cfg(feature = "non-standard-zstd")]
    #[must_use]
    pub fn non_standard_zstd_compression_level(
        mut self,
        zstd_compression_level: Option<u8>,
    ) -> Self {
        self.non_standard_zstd_compression_level = zstd_compression_level;
        self
    }

    /// Creates a new [`Client`], connecting to the given address.
    ///
    /// # Errors
    ///
    /// It returns an error if the connection fails.
    pub async fn connect(self, addr: ServerAddr) -> Result<Client, ConnectHandlerError> {
        Client::connect(addr, self).await
    }

    /// Creates a new [`Client`], connecting to the given address in the background.
    ///
    /// This method is for applications that want to construct a client without
    /// waiting for the server connection to be established. This may result in
    /// the connection never succeeding, despite the continuous attempts
    /// made by the client, and the [`Client`] buffer filling up with requests
    /// and blocking all subsequent commands forever.
    pub fn connect_lazy(self, addr: ServerAddr) -> Client {
        Client::connect_lazy(addr, self)
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}
