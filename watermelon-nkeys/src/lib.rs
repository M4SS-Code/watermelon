#![forbid(unsafe_code)]

pub use self::seed::{KeyPair, KeyPairFromSeedError, PublicKey};

mod crc;
mod seed;
