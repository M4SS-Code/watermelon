use std::io;

use tokio::net::TcpStream;
use tokio_rustls::{
    rustls::pki_types::{InvalidDnsNameError, ServerName},
    TlsConnector,
};
use watermelon_net::{
    connect_tcp,
    error::{ConnectionReadError, StreamingReadError},
    proto_connect, Connection, StreamingConnection,
};
#[cfg(feature = "websocket")]
use watermelon_net::{error::WebsocketReadError, WebsocketConnection};
#[cfg(feature = "websocket")]
use watermelon_proto::proto::error::FrameDecoderError;
use watermelon_proto::{
    proto::{error::DecoderError, ServerOp},
    Connect, Host, NonStandardConnect, Protocol, ServerAddr, ServerInfo, Transport,
};

use crate::{util::MaybeConnection, ConnectFlags, ConnectionCompression};

use super::{
    authenticator::{AuthenticationError, AuthenticationMethod},
    connection::ConnectionSecurity,
};

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("io error")]
    Io(#[source] io::Error),
    #[error("invalid DNS name")]
    InvalidDnsName(#[source] InvalidDnsNameError),
    #[error("websocket not supported")]
    WebsocketUnsupported,
    #[error("unexpected ServerOp")]
    UnexpectedServerOp,
    #[error("decoder error")]
    Decoder(#[source] DecoderError),
    #[error("authentication error")]
    Authentication(#[source] AuthenticationError),
    #[error("connect")]
    Connect(#[source] watermelon_net::error::ConnectError),
}

#[expect(clippy::too_many_lines)]
pub(crate) async fn connect(
    connector: &TlsConnector,
    addr: &ServerAddr,
    client_name: String,
    auth_method: Option<&AuthenticationMethod>,
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
    let conn = connect_tcp(addr).await.map_err(ConnectError::Io)?;
    conn.set_nodelay(true).map_err(ConnectError::Io)?;
    let mut conn = ConnectionSecurity::Plain(conn);

    if matches!(addr.protocol(), Protocol::TLS) {
        let domain = rustls_server_name_from_addr(addr).map_err(ConnectError::InvalidDnsName)?;
        conn = conn
            .upgrade_tls(connector, domain.to_owned())
            .await
            .map_err(ConnectError::Io)?;
    }

    let mut conn = match addr.transport() {
        Transport::TCP => Connection::Streaming(StreamingConnection::new(conn)),
        #[cfg(feature = "websocket")]
        Transport::Websocket => {
            let uri = addr.to_string().parse().unwrap();
            Connection::Websocket(
                WebsocketConnection::new(uri, conn)
                    .await
                    .map_err(ConnectError::Io)?,
            )
        }
        #[cfg(not(feature = "websocket"))]
        Transport::Websocket => return Err(ConnectError::WebsocketUnsupported),
    };
    let info = match conn.read_next().await {
        Ok(ServerOp::Info { info }) => info,
        Ok(_) => return Err(ConnectError::UnexpectedServerOp),
        Err(ConnectionReadError::Streaming(StreamingReadError::Io(err))) => {
            return Err(ConnectError::Io(err))
        }
        Err(ConnectionReadError::Streaming(StreamingReadError::Decoder(err))) => {
            return Err(ConnectError::Decoder(err))
        }
        #[cfg(feature = "websocket")]
        Err(ConnectionReadError::Websocket(WebsocketReadError::Io(err))) => {
            return Err(ConnectError::Io(err))
        }
        #[cfg(feature = "websocket")]
        Err(ConnectionReadError::Websocket(WebsocketReadError::Decoder(
            FrameDecoderError::Decoder(err),
        ))) => return Err(ConnectError::Decoder(err)),
        #[cfg(feature = "websocket")]
        Err(ConnectionReadError::Websocket(WebsocketReadError::Decoder(
            FrameDecoderError::IncompleteFrame,
        ))) => todo!(),
        #[cfg(feature = "websocket")]
        Err(ConnectionReadError::Websocket(WebsocketReadError::Closed)) => todo!(),
    };

    let conn = match conn {
        Connection::Streaming(streaming) => Connection::Streaming(
            if matches!(
                (addr.protocol(), info.tls_required),
                (Protocol::PossiblyPlain, true)
            ) {
                let domain =
                    rustls_server_name_from_addr(addr).map_err(ConnectError::InvalidDnsName)?;
                StreamingConnection::new(
                    streaming
                        .into_inner()
                        .upgrade_tls(connector, domain.to_owned())
                        .await
                        .map_err(ConnectError::Io)?,
                )
            } else {
                streaming
            },
        ),
        Connection::Websocket(websocket) => Connection::Websocket(websocket),
    };

    let auth;
    let auth_method = if let Some(auth_method) = auth_method {
        Some(auth_method)
    } else if let Some(auth_method) = AuthenticationMethod::try_from_addr(addr) {
        auth = auth_method;
        Some(&auth)
    } else {
        None
    };

    #[allow(unused_mut)]
    let mut non_standard = NonStandardConnect::default();
    #[cfg(feature = "non-standard-zstd")]
    if matches!(conn, Connection::Streaming(_)) {
        non_standard.zstd = flags.zstd_compression_level.is_some() && info.non_standard.zstd;
    }

    let mut connect = Connect {
        verbose: true,
        pedantic: false,
        require_tls: false,
        auth_token: None,
        username: None,
        password: None,
        client_name: Some(client_name),
        client_lang: "rust-watermelon",
        client_version: env!("CARGO_PKG_VERSION"),
        protocol: 1,
        echo: flags.echo,
        signature: None,
        jwt: None,
        supports_no_responders: true,
        supports_headers: true,
        nkey: None,
        non_standard,
    };
    if let Some(auth_method) = auth_method {
        auth_method
            .prepare_for_auth(&info, &mut connect)
            .map_err(ConnectError::Authentication)?;
    }

    let mut conn = match conn {
        Connection::Streaming(streaming) => {
            Connection::Streaming(streaming.replace_socket(|stream| {
                MaybeConnection(Some(ConnectionCompression::Plain(stream)))
            }))
        }
        Connection::Websocket(websocket) => Connection::Websocket(websocket),
    };

    #[cfg(feature = "non-standard-zstd")]
    let zstd = connect.non_standard.zstd;

    proto_connect(&mut conn, connect, |conn| {
        #[cfg(feature = "non-standard-zstd")]
        match conn {
            Connection::Streaming(streaming) => {
                if zstd {
                    if let Some(zstd_compression_level) = flags.zstd_compression_level {
                        let stream = streaming.socket_mut().0.take().unwrap();
                        streaming.socket_mut().0 =
                            Some(stream.upgrade_zstd(zstd_compression_level));
                    }
                }
            }
            Connection::Websocket(_websocket) => {}
        }

        let _ = conn;
    })
    .await
    .map_err(ConnectError::Connect)?;

    let conn = match conn {
        Connection::Streaming(streaming) => {
            Connection::Streaming(streaming.replace_socket(|stream| stream.0.unwrap()))
        }
        Connection::Websocket(websocket) => Connection::Websocket(websocket),
    };

    Ok((conn, info))
}

fn rustls_server_name_from_addr(addr: &ServerAddr) -> Result<ServerName<'_>, InvalidDnsNameError> {
    match addr.host() {
        Host::Ip(addr) => Ok(ServerName::IpAddress((*addr).into())),
        Host::Dns(name) => <_ as AsRef<str>>::as_ref(name).try_into(),
    }
}
