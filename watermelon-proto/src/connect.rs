use alloc::string::String;

use serde::Serialize;

#[derive(Debug, Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct Connect {
    pub verbose: bool,
    pub pedantic: bool,
    #[serde(rename = "tls_required")]
    pub require_tls: bool,
    pub auth_token: Option<String>,
    #[serde(rename = "user")]
    pub username: Option<String>,
    #[serde(rename = "pass")]
    pub password: Option<String>,
    #[serde(rename = "name", skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    #[serde(rename = "lang")]
    pub client_lang: &'static str,
    #[serde(rename = "version")]
    pub client_version: &'static str,
    pub protocol: u8,
    pub echo: bool,
    #[serde(rename = "sig")]
    pub signature: Option<String>,
    pub jwt: Option<String>,
    #[serde(rename = "no_responders")]
    pub supports_no_responders: bool,
    #[serde(rename = "headers")]
    pub supports_headers: bool,
    pub nkey: Option<String>,

    #[serde(flatten)]
    pub non_standard: NonStandardConnect,
}

#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct NonStandardConnect {
    #[cfg(feature = "non-standard-zstd")]
    #[serde(
        rename = "m4ss_zstd",
        skip_serializing_if = "skip_serializing_if_false"
    )]
    pub zstd: bool,
}

#[allow(clippy::derivable_impls)]
impl Default for NonStandardConnect {
    fn default() -> Self {
        Self {
            #[cfg(feature = "non-standard-zstd")]
            zstd: false,
        }
    }
}

#[cfg(feature = "non-standard-zstd")]
#[allow(clippy::trivially_copy_pass_by_ref)]
fn skip_serializing_if_false(val: &bool) -> bool {
    !*val
}
