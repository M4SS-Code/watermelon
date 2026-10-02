use std::{error::Error, net::SocketAddr, time::Duration};

use bytes::Bytes;
use http_body_util::{BodyExt as _, Empty};
use hyper::{StatusCode, Uri};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use tokio::time::timeout;

const TIMEOUT: Duration = Duration::from_secs(5);

type BoxError = Box<dyn Error + Send + Sync>;

/// HTTP client for the `nats-server` monitoring endpoint
#[derive(Debug, Clone)]
pub(crate) struct HttpClient {
    client: Client<HttpConnector, Empty<Bytes>>,
}

impl HttpClient {
    pub(crate) fn new() -> Self {
        Self {
            client: Client::builder(TokioExecutor::new()).build_http(),
        }
    }

    /// `GET` `path_and_query` from `addr`, returning the status code and body
    pub(crate) async fn get(
        &self,
        addr: SocketAddr,
        path_and_query: &str,
    ) -> Result<(StatusCode, Bytes), BoxError> {
        let uri = format!("http://{addr}{path_and_query}").parse::<Uri>()?;
        timeout(TIMEOUT, async {
            let response = self.client.get(uri).await?;
            let status = response.status();
            let body = response.into_body().collect().await?.to_bytes();
            Ok((status, body))
        })
        .await?
    }
}
