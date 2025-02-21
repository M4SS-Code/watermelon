use std::path::PathBuf;

use serde::{Deserialize, Deserializer, de};
use watermelon_nkeys::KeyPair;
use watermelon_proto::Subject;

#[derive(Debug, Deserialize)]
pub(super) struct FromEnv {
    #[serde(flatten)]
    pub(super) auth: AuthenticationMethod,
    pub(super) inbox_prefix: Option<Subject>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum AuthenticationMethod {
    Creds {
        #[serde(rename = "nats_jwt")]
        jwt: String,
        #[serde(rename = "nats_nkey", deserialize_with = "deserialize_nkey")]
        nkey: KeyPair,
    },
    CredsFile {
        #[serde(rename = "nats_creds_file")]
        creds_file: PathBuf,
    },
    UserAndPassword {
        #[serde(rename = "nats_username")]
        username: String,
        #[serde(rename = "nats_password")]
        password: String,
    },
    None,
}

fn deserialize_nkey<'de, D>(deserializer: D) -> Result<KeyPair, D::Error>
where
    D: Deserializer<'de>,
{
    let secret = String::deserialize(deserializer)?;
    KeyPair::from_encoded_seed(&secret).map_err(de::Error::custom)
}
