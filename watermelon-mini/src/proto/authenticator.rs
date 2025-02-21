use std::fmt::{self, Debug, Formatter};

use watermelon_nkeys::{KeyPair, KeyPairFromSeedError};
use watermelon_proto::{Connect, ServerAddr, ServerInfo};

pub enum AuthenticationMethod {
    UserAndPassword { username: String, password: String },
    Creds { jwt: String, nkey: KeyPair },
}

#[derive(Debug, thiserror::Error)]
pub enum AuthenticationError {
    #[error("missing nonce")]
    MissingNonce,
}

#[derive(Debug, thiserror::Error)]
pub enum CredsParseError {
    #[error("contents are truncated")]
    Truncated,
    #[error("missing closing for JWT")]
    MissingJwtClosing,
    #[error("missing closing for nkey")]
    MissingNkeyClosing,
    #[error("missing JWT")]
    MissingJwt,
    #[error("missing nkey")]
    MissingNkey,
    #[error("invalid nkey")]
    InvalidKey(#[source] KeyPairFromSeedError),
}

impl AuthenticationMethod {
    pub(crate) fn try_from_addr(addr: &ServerAddr) -> Option<Self> {
        if let (Some(username), Some(password)) = (addr.username(), addr.password()) {
            Some(Self::UserAndPassword {
                username: username.to_owned(),
                password: password.to_owned(),
            })
        } else {
            None
        }
    }

    pub(crate) fn prepare_for_auth(
        &self,
        info: &ServerInfo,
        connect: &mut Connect,
    ) -> Result<(), AuthenticationError> {
        match self {
            Self::UserAndPassword { username, password } => {
                connect.username = Some(username.clone());
                connect.password = Some(password.clone());
            }
            Self::Creds { jwt, nkey } => {
                let nonce = info
                    .nonce
                    .as_deref()
                    .ok_or(AuthenticationError::MissingNonce)?;
                let signature = nkey.sign(nonce.as_bytes()).to_string();

                connect.jwt = Some(jwt.clone());
                connect.nkey = Some(nkey.public_key().to_string());
                connect.signature = Some(signature);
            }
        }

        Ok(())
    }

    /// Creates an `AuthenticationMethod` from the content of a credentials file.
    ///
    /// # Errors
    ///
    /// It returns an error if the content is not valid.
    pub fn from_creds(contents: &str) -> Result<Self, CredsParseError> {
        let mut jtw = None;
        let mut secret = None;

        let mut lines = contents.lines();
        while let Some(line) = lines.next() {
            if line == "-----BEGIN NATS USER JWT-----" {
                jtw = Some(lines.next().ok_or(CredsParseError::Truncated)?);

                let line = lines.next().ok_or(CredsParseError::Truncated)?;
                if line != "------END NATS USER JWT------" {
                    return Err(CredsParseError::MissingJwtClosing);
                }
            } else if line == "-----BEGIN USER NKEY SEED-----" {
                secret = Some(lines.next().ok_or(CredsParseError::Truncated)?);

                let line = lines.next().ok_or(CredsParseError::Truncated)?;
                if line != "------END USER NKEY SEED------" {
                    return Err(CredsParseError::MissingNkeyClosing);
                }
            }
        }

        let jtw = jtw.ok_or(CredsParseError::MissingJwt)?;
        let nkey = secret.ok_or(CredsParseError::MissingNkey)?;
        let nkey = KeyPair::from_encoded_seed(nkey).map_err(CredsParseError::InvalidKey)?;

        Ok(Self::Creds {
            jwt: jtw.to_owned(),
            nkey,
        })
    }
}

impl Debug for AuthenticationMethod {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthenticationMethod")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::AuthenticationMethod;

    #[test]
    fn parse_creds() {
        let creds = r"-----BEGIN NATS USER JWT-----
eyJ0eXAiOiJqd3QiLCJhbGciOiJlZDI1NTE5In0.eyJqdGkiOiJUVlNNTEtTWkJBN01VWDNYQUxNUVQzTjRISUw1UkZGQU9YNUtaUFhEU0oyWlAzNkVMNVJBIiwiaWF0IjoxNTU4MDQ1NTYyLCJpc3MiOiJBQlZTQk0zVTQ1REdZRVVFQ0tYUVM3QkVOSFdHN0tGUVVEUlRFSEFKQVNPUlBWV0JaNEhPSUtDSCIsIm5hbWUiOiJvbWVnYSIsInN1YiI6IlVEWEIyVk1MWFBBU0FKN1pEVEtZTlE3UU9DRldTR0I0Rk9NWVFRMjVIUVdTQUY3WlFKRUJTUVNXIiwidHlwZSI6InVzZXIiLCJuYXRzIjp7InB1YiI6e30sInN1YiI6e319fQ.6TQ2ilCDb6m2ZDiJuj_D_OePGXFyN3Ap2DEm3ipcU5AhrWrNvneJryWrpgi_yuVWKo1UoD5s8bxlmwypWVGFAA
------END NATS USER JWT------

************************* IMPORTANT *************************
NKEY Seed printed below can be used to sign and prove identity.
NKEYs are sensitive and should be treated as secrets.

-----BEGIN USER NKEY SEED-----
SUAOY5JZ2WJKVR4UO2KJ2P3SW6FZFNWEOIMAXF4WZEUNVQXXUOKGM55CYE
------END USER NKEY SEED------

*************************************************************";

        let AuthenticationMethod::Creds { jwt, nkey } =
            AuthenticationMethod::from_creds(creds).unwrap()
        else {
            panic!("invalid auth method");
        };
        assert_eq!(
            jwt,
            "eyJ0eXAiOiJqd3QiLCJhbGciOiJlZDI1NTE5In0.eyJqdGkiOiJUVlNNTEtTWkJBN01VWDNYQUxNUVQzTjRISUw1UkZGQU9YNUtaUFhEU0oyWlAzNkVMNVJBIiwiaWF0IjoxNTU4MDQ1NTYyLCJpc3MiOiJBQlZTQk0zVTQ1REdZRVVFQ0tYUVM3QkVOSFdHN0tGUVVEUlRFSEFKQVNPUlBWV0JaNEhPSUtDSCIsIm5hbWUiOiJvbWVnYSIsInN1YiI6IlVEWEIyVk1MWFBBU0FKN1pEVEtZTlE3UU9DRldTR0I0Rk9NWVFRMjVIUVdTQUY3WlFKRUJTUVNXIiwidHlwZSI6InVzZXIiLCJuYXRzIjp7InB1YiI6e30sInN1YiI6e319fQ.6TQ2ilCDb6m2ZDiJuj_D_OePGXFyN3Ap2DEm3ipcU5AhrWrNvneJryWrpgi_yuVWKo1UoD5s8bxlmwypWVGFAA"
        );
        assert_eq!(
            nkey.public_key().to_string(),
            "SAAO4HKVRO54CIBH7EONLBWD6BYIW2IYHQVZTCCDLU6C2IAX7GBEQGJDYE"
        );
    }
}
