use alloc::{string::String, vec::Vec};
use core::{net::IpAddr, num::NonZero};

use serde::Deserialize;

use crate::ServerAddr;

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct ServerInfo {
    #[serde(rename = "server_id")]
    pub id: String,
    #[serde(rename = "server_name")]
    pub name: String,
    pub version: String,
    #[serde(rename = "go")]
    pub go_version: String,
    pub host: IpAddr,
    pub port: NonZero<u16>,
    #[serde(rename = "headers")]
    pub supports_headers: bool,
    pub max_payload: NonZero<u32>,
    #[serde(rename = "proto")]
    pub protocol_version: u32,
    #[serde(default)]
    pub client_id: Option<u64>,
    #[serde(default)]
    pub auth_required: bool,
    #[serde(default)]
    pub tls_required: bool,
    #[serde(default)]
    pub tls_verify: bool,
    #[serde(default)]
    pub tls_available: bool,
    #[serde(default)]
    pub connect_urls: Vec<ServerAddr>,
    #[serde(default, rename = "ws_connect_urls")]
    pub websocket_connect_urls: Vec<ServerAddr>,
    #[serde(default, rename = "ldm")]
    pub lame_duck_mode: bool,
    #[serde(default)]
    pub git_commit: Option<String>,
    #[serde(default, rename = "jetstream")]
    pub supports_jetstream: bool,
    #[serde(default)]
    pub ip: Option<IpAddr>,
    #[serde(default)]
    pub client_ip: Option<IpAddr>,
    #[serde(default)]
    pub nonce: Option<String>,
    #[serde(default, rename = "cluster")]
    pub cluster_name: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,

    #[serde(flatten)]
    pub non_standard: NonStandardServerInfo,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Default)]
#[non_exhaustive]
pub struct NonStandardServerInfo {
    #[cfg(feature = "non-standard-zstd")]
    #[serde(default, rename = "m4ss_zstd")]
    pub zstd: bool,
}
