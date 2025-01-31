#![forbid(unsafe_code)]

use std::sync::Arc;

use rustls_platform_verifier::Verifier;
use tokio::net::TcpStream;
use tokio_rustls::{
    rustls::{self, crypto::CryptoProvider, version::TLS13, ClientConfig},
    TlsConnector,
};
use watermelon_net::Connection;
use watermelon_proto::{ServerAddr, ServerInfo};

#[cfg(feature = "non-standard-zstd")]
pub use self::non_standard_zstd::ZstdStream;
use self::proto::connect;
pub use self::proto::{
    AuthenticationMethod, ConnectError, ConnectionCompression, ConnectionSecurity,
};

#[cfg(feature = "non-standard-zstd")]
pub(crate) mod non_standard_zstd;
mod proto;
mod util;

#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ConnectFlags {
    pub echo: bool,
    #[cfg(feature = "non-standard-zstd")]
    pub zstd: bool,
}

/// Connect to a given address with some reasonable presets.
///
/// The function is going to establish a TLS 1.3 connection, without the support of the client
/// authorization.
///
/// # Errors
///
/// This returns an error in case the connection fails.
#[expect(
    clippy::missing_panics_doc,
    reason = "the crypto_provider function always returns a provider that supports TLS 1.3"
)]
pub async fn easy_connect(
    addr: &ServerAddr,
    auth: Option<&AuthenticationMethod>,
    flags: ConnectFlags,
) -> Result<
    (
        Connection<
            ConnectionCompression<ConnectionSecurity<TcpStream>>,
            ConnectionSecurity<TcpStream>,
        >,
        Box<ServerInfo>,
    ),
    ConnectError,
> {
    let provider = Arc::new(crypto_provider());
    let connector = TlsConnector::from(Arc::new(
        ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_protocol_versions(&[&TLS13])
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Verifier::new().with_provider(provider)))
            .with_no_client_auth(),
    ));

    let (conn, info) = connect(&connector, addr, "watermelon".to_owned(), auth, flags).await?;
    Ok((conn, info))
}

fn crypto_provider() -> CryptoProvider {
    #[cfg(feature = "aws-lc-rs")]
    return rustls::crypto::aws_lc_rs::default_provider();
    #[cfg(all(not(feature = "aws-lc-rs"), feature = "ring"))]
    return rustls::crypto::ring::default_provider();
    #[cfg(not(any(feature = "aws-lc-rs", feature = "ring")))]
    compile_error!("Please enable the `aws-lc-rs` or the `ring` feature")
}
