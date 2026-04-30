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
    None {},
}

fn deserialize_nkey<'de, D>(deserializer: D) -> Result<KeyPair, D::Error>
where
    D: Deserializer<'de>,
{
    let secret = String::deserialize(deserializer)?;
    KeyPair::from_encoded_seed(&secret).map_err(de::Error::custom)
}

#[cfg(test)]
mod tests {
    use claims::{assert_matches, assert_ok};

    use super::{AuthenticationMethod, FromEnv};

    const TEST_SEED: &str = "SAAPN4W3EG6KCJGUQTKTJ5GSB5NHK5CHAJL4DBGFUM3HHROI4XUEP4OBK4";
    const TEST_PUBLIC_KEY: &str = "SAAJYMSGSUUUC3GAOKL2IFAAKQDV32K4X45HPCPC4EBM7F7N76HQGR4C2I";

    fn from_iter<const N: usize>(vars: [(&str, &str); N]) -> FromEnv {
        let iter = vars.into_iter().map(|(k, v)| (k.to_owned(), v.to_owned()));
        assert_ok!(envy::from_iter::<_, FromEnv>(iter))
    }

    #[test]
    fn deserialize_creds() {
        let env = from_iter([
            ("NATS_JWT", "eyJhbGciOiJlZDI1NTE5In0"),
            ("NATS_NKEY", TEST_SEED),
        ]);
        assert_matches!(
            env.auth,
            AuthenticationMethod::Creds { jwt, nkey }
                if jwt == "eyJhbGciOiJlZDI1NTE5In0"
                    && nkey.public_key().to_string() == TEST_PUBLIC_KEY
        );
        assert!(env.inbox_prefix.is_none());
    }

    #[test]
    fn deserialize_creds_file() {
        let env = from_iter([("NATS_CREDS_FILE", "/etc/nats/user.creds")]);
        assert_matches!(
            env.auth,
            AuthenticationMethod::CredsFile { creds_file }
                if creds_file.as_os_str() == "/etc/nats/user.creds"
        );
        assert!(env.inbox_prefix.is_none());
    }

    #[test]
    fn deserialize_user_and_password() {
        let env = from_iter([("NATS_USERNAME", "alice"), ("NATS_PASSWORD", "hunter2")]);
        assert_matches!(
            env.auth,
            AuthenticationMethod::UserAndPassword { username, password }
                if username == "alice" && password == "hunter2"
        );
        assert!(env.inbox_prefix.is_none());
    }

    #[test]
    fn deserialize_none() {
        let env = from_iter([]);
        assert_matches!(env.auth, AuthenticationMethod::None {});
        assert!(env.inbox_prefix.is_none());
    }

    #[test]
    fn deserialize_none_with_inbox_prefix() {
        let env = from_iter([("INBOX_PREFIX", "_CUSTOM_INBOX")]);
        assert_matches!(env.auth, AuthenticationMethod::None {});
        assert_eq!(env.inbox_prefix.unwrap().as_str(), "_CUSTOM_INBOX");
    }
}
