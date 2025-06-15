use std::fmt::{self, Display};

#[cfg(feature = "aws-lc-rs")]
use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair as _, Signature as LlSignature};
use data_encoding::{BASE32_NOPAD, BASE64URL_NOPAD};
#[cfg(all(not(feature = "aws-lc-rs"), feature = "ring"))]
use ring::signature::{Ed25519KeyPair, KeyPair as _, Signature as LlSignature};

#[cfg(not(any(feature = "aws-lc-rs", feature = "ring")))]
compile_error!("Please enable the `aws-lc-rs` or the `ring` feature");

use crate::crc::Crc16;

const SEED_PREFIX_BYTE: u8 = 18 << 3;

/// A `NKey` private/public key pair.
#[derive(Debug)]
pub struct KeyPair {
    kind: u8,
    key: Ed25519KeyPair,
}

/// The public key within an `NKey` private/public key pair.
#[derive(Debug)]
pub struct PublicKey<'a>(&'a KeyPair);

/// An error encountered while decoding an `NKey`.
#[derive(Debug, thiserror::Error)]
pub enum KeyPairFromSeedError {
    /// The string rapresentation of the seed has an invalid length.
    #[error("invalid length of the seed's string the string rapresentation")]
    InvalidSeedLength,
    /// The string rapresentation of the seed contains characters that are not part of the base32 dictionary.
    #[error("the seed contains non-base32 characters")]
    InvalidBase32,
    /// The decoded base32 rapresentation of the seed has an invalid length.
    #[error("invalid base32 decoded seed length")]
    InvalidRawSeedLength,
    /// The CRC does not match the crc calculated for the seed payload.
    #[error("invalid CRC")]
    BadCrc,
    /// The prefix for the seed is invalid
    #[error("invalid seed prefix")]
    InvalidPrefix,
    /// the seed could not be decoded by the crypto backend
    #[error("decode error")]
    DecodeError,
}

/// A payload signed via a [`KeyPair`].
///
/// Obtained from [`KeyPair::sign`].
pub struct Signature(LlSignature);

impl KeyPair {
    /// Decode a key from an `NKey` seed.
    ///
    /// # Errors
    ///
    /// Returns an error if `seed` is invalid.
    #[expect(
        clippy::missing_panics_doc,
        reason = "the array `TryInto` calls cannot panic"
    )]
    pub fn from_encoded_seed(seed: &str) -> Result<Self, KeyPairFromSeedError> {
        if seed.len() != 58 {
            return Err(KeyPairFromSeedError::InvalidSeedLength);
        }

        let mut full_raw_seed = [0; 36];
        let len = BASE32_NOPAD
            .decode_mut(seed.as_bytes(), &mut full_raw_seed)
            .map_err(|_| KeyPairFromSeedError::InvalidBase32)?;
        if len != full_raw_seed.len() {
            return Err(KeyPairFromSeedError::InvalidRawSeedLength);
        }

        let (raw_seed, crc) = full_raw_seed.split_at(full_raw_seed.len() - 2);
        let raw_seed_crc = Crc16::compute(raw_seed);
        let expected_crc = Crc16::from_raw_encoded(crc.try_into().unwrap());
        if raw_seed_crc != expected_crc {
            return Err(KeyPairFromSeedError::BadCrc);
        }

        Self::from_raw_seed(raw_seed.try_into().unwrap())
    }

    fn from_raw_seed(raw_seed: [u8; 34]) -> Result<Self, KeyPairFromSeedError> {
        if raw_seed[0] & 248 != SEED_PREFIX_BYTE {
            println!("{:x}", raw_seed[0]);
            return Err(KeyPairFromSeedError::InvalidPrefix);
        }

        let kind = raw_seed[1];

        let key = Ed25519KeyPair::from_seed_unchecked(&raw_seed[2..])
            .map_err(|_| KeyPairFromSeedError::DecodeError)?;
        Ok(Self { kind, key })
    }

    #[must_use]
    pub fn public_key(&self) -> PublicKey<'_> {
        PublicKey(self)
    }

    #[must_use]
    pub fn sign(&self, buf: &[u8]) -> Signature {
        Signature(self.key.sign(buf))
    }
}

impl Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&BASE64URL_NOPAD.encode_display(self.0.as_ref()), f)
    }
}

impl Display for PublicKey<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut full_raw_seed = [0; 36];
        full_raw_seed[0] = SEED_PREFIX_BYTE;
        full_raw_seed[1] = self.0.kind;
        full_raw_seed[2..34].copy_from_slice(self.0.key.public_key().as_ref());
        let crc = Crc16::compute(&full_raw_seed[..34]);
        full_raw_seed[34..36].copy_from_slice(&crc.to_raw_encoded());
        Display::fmt(&BASE32_NOPAD.encode_display(&full_raw_seed), f)
    }
}

#[cfg(test)]
mod tests {
    use claims::assert_matches;

    use super::{KeyPair, KeyPairFromSeedError};

    #[test]
    fn sign() {
        let key = KeyPair::from_encoded_seed(
            "SAAPN4W3EG6KCJGUQTKTJ5GSB5NHK5CHAJL4DBGFUM3HHROI4XUEP4OBK4",
        )
        .unwrap();
        assert_eq!(
            "HuHkn4SHFW1ibjQzmqyNw8KUZDWB0bKciDbK7YmNyqyyvC3k4s0AqimAz6jMt0xhLqGAOyj30UaUol2xMVpsBQ",
            key.sign(b"fwD9iyDvqxpcj3ii").to_string()
        );
    }

    #[test]
    fn gen_public_key() {
        let key = KeyPair::from_encoded_seed(
            "SAAPN4W3EG6KCJGUQTKTJ5GSB5NHK5CHAJL4DBGFUM3HHROI4XUEP4OBK4",
        )
        .unwrap();
        assert_eq!(
            "SAAJYMSGSUUUC3GAOKL2IFAAKQDV32K4X45HPCPC4EBM7F7N76HQGR4C2I",
            key.public_key().to_string()
        );
    }

    #[test]
    fn invalid_len() {
        assert_matches!(
            KeyPair::from_encoded_seed(""),
            Err(KeyPairFromSeedError::InvalidSeedLength)
        );
    }

    #[test]
    fn invalid_base32() {
        assert_matches!(
            KeyPair::from_encoded_seed(
                "SAAPN4W3EG6KCJGUQTKTJ5!#B5NHK5CHAJL4DBGFUM3HHROI4XUEP4OBK4"
            ),
            Err(KeyPairFromSeedError::InvalidBase32)
        )
    }

    #[test]
    fn invalid_crc() {
        assert_matches!(
            KeyPair::from_encoded_seed(
                "FAAPN4W3EG6KCJGUQTKTJ5GSB5NHK5CHAJL4DBGFUM3HHROI4XUEP4OBK4"
            ),
            Err(KeyPairFromSeedError::BadCrc)
        )
    }

    #[test]
    fn invalid_prefix() {
        assert_matches!(
            KeyPair::from_encoded_seed(
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
            ),
            Err(KeyPairFromSeedError::InvalidPrefix)
        )
    }
}
