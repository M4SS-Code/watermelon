#![forbid(unsafe_code)]

pub use self::seed::{KeyPair, KeyPairFromSeedError, PublicKey, Signature};

mod crc;
mod seed;
