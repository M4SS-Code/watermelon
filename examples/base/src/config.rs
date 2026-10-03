//! NATS connection configuration.
//!
//! Add `#[command(flatten)]` on a field in your own `#[derive(Parser)]` to
//! include these fields in your CLI config. Every field can be set either
//! through a command line flag or through its environment variable.
//!
//! **Env vars:** `NATS_URL`, `NATS_USERNAME` + `NATS_PASSWORD`,
//! `NATS_JWT` + `NATS_NKEY`, `NATS_CREDS_FILE`, `NATS_INBOX_PREFIX`.

use std::{io, path::PathBuf};

use clap::Args;
use watermelon::{
    core::{AuthenticationMethod, Client, ClientBuilder, error::CredsParseError},
    proto::{ServerAddr, Subject},
};
use watermelon_nkeys::{KeyPair, KeyPairFromSeedError};

#[derive(Debug, Args)]
#[command(next_help_heading = "NATS")]
pub(crate) struct NatsConfig {
    /// Server address.
    #[arg(
        long = "nats-url",
        env = "NATS_URL",
        default_value = "nats://demo.nats.io"
    )]
    pub(crate) url: ServerAddr,

    /// Username for NATS authentication.
    #[arg(long = "nats-username", env = "NATS_USERNAME", requires = "password")]
    pub(crate) username: Option<String>,

    /// Password for NATS authentication.
    #[arg(
        long = "nats-password",
        env = "NATS_PASSWORD",
        requires = "username",
        hide_env_values = true
    )]
    pub(crate) password: Option<String>,

    /// JWT for NATS authentication.
    #[arg(
        long = "nats-jwt",
        env = "NATS_JWT",
        requires = "nkey",
        conflicts_with = "username",
        hide_env_values = true
    )]
    pub(crate) jwt: Option<String>,

    /// NKEY seed for NATS authentication.
    #[arg(
        long = "nats-nkey",
        env = "NATS_NKEY",
        requires = "jwt",
        hide_env_values = true
    )]
    pub(crate) nkey: Option<String>,

    /// Path to a `.creds` file for NATS authentication.
    #[arg(
        long = "nats-creds-file",
        env = "NATS_CREDS_FILE",
        conflicts_with_all = ["username", "jwt"]
    )]
    pub(crate) creds_file: Option<PathBuf>,

    /// Custom inbox prefix for NATS reply subjects.
    #[arg(long = "nats-inbox-prefix", env = "NATS_INBOX_PREFIX")]
    pub(crate) inbox_prefix: Option<Subject>,
}

impl NatsConfig {
    /// Apply this configuration to `builder` and connect to the NATS server.
    pub(crate) async fn connect(&self, mut builder: ClientBuilder) -> Result<Client, ConnectError> {
        builder = builder.authentication_method(self.auth_method()?);
        if let Some(inbox_prefix) = &self.inbox_prefix {
            builder = builder.inbox_prefix(inbox_prefix.clone());
        }

        Box::pin(builder.connect(self.url.clone()))
            .await
            .map_err(ConnectError::Nats)
    }

    fn auth_method(&self) -> Result<Option<AuthenticationMethod>, ConnectError> {
        if let Some(creds_file) = &self.creds_file {
            let contents =
                std::fs::read_to_string(creds_file).map_err(ConnectError::ReadCredsFile)?;
            let auth = AuthenticationMethod::from_creds(&contents)
                .map_err(ConnectError::ParseCredsFile)?;
            Ok(Some(auth))
        } else if let (Some(jwt), Some(nkey)) = (&self.jwt, &self.nkey) {
            let nkey = KeyPair::from_encoded_seed(nkey).map_err(ConnectError::InvalidNkey)?;
            Ok(Some(AuthenticationMethod::Creds {
                jwt: jwt.clone(),
                nkey,
            }))
        } else if let (Some(username), Some(password)) = (&self.username, &self.password) {
            Ok(Some(AuthenticationMethod::UserAndPassword {
                username: username.clone(),
                password: password.clone(),
            }))
        } else {
            Ok(None)
        }
    }
}

/// Errors that can occur when connecting to NATS.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ConnectError {
    #[error("NATS connection failed")]
    Nats(#[source] watermelon::core::error::ConnectHandlerError),
    #[error("failed to read credentials file")]
    ReadCredsFile(#[source] io::Error),
    #[error("failed to parse credentials file")]
    ParseCredsFile(#[source] CredsParseError),
    #[error("invalid NKEY seed")]
    InvalidNkey(#[source] KeyPairFromSeedError),
}
