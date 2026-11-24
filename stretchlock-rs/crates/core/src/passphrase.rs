use serde::{Deserialize, Serialize};
use sha2::Sha256;

use pbkdf2::pbkdf2_hmac;

pub const PBKDF2_ITERATIONS: u32 = 100_000;
pub const DEFAULT_PASSPHRASE: &str = "unlock";
pub const HASH_LEN: usize = 32;
pub const SALT_LEN: usize = 16;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PassphraseHash {
    pub salt_hex: String,
    pub iterations: u32,
    pub hash_hex: String,
}

impl PassphraseHash {
    pub fn generate(plain: &str) -> Self {
        let salt = random_salt();
        let hash = hash_passphrase(plain, &salt, PBKDF2_ITERATIONS);
        Self {
            salt_hex: hex::encode(salt),
            iterations: PBKDF2_ITERATIONS,
            hash_hex: hex::encode(hash),
        }
    }
}

pub fn hash_passphrase(plain: &str, salt: &[u8], iterations: u32) -> Vec<u8> {
    let mut out = vec![0u8; HASH_LEN];
    pbkdf2_hmac::<Sha256>(plain.as_bytes(), salt, iterations, &mut out);
    out
}

pub fn random_salt() -> [u8; SALT_LEN] {
    use rand::RngCore;
    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    salt
}

pub fn verify(plain: &str, stored: &PassphraseHash) -> bool {
    let (salt, expected) = match (
        hex::decode(&stored.salt_hex),
        hex::decode(&stored.hash_hex),
    ) {
        (Ok(salt), Ok(expected)) => (salt, expected),
        _ => return false,
    };
    let computed = hash_passphrase(plain, &salt, stored.iterations);
    constant_time_eq(&computed, &expected)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_verify_round_trip() {
        let salt = random_salt();
        let hash = hash_passphrase("pw", &salt, 1000);
        assert_eq!(hash.len(), HASH_LEN);
        let stored = PassphraseHash {
            salt_hex: hex::encode(salt),
            iterations: 1000,
            hash_hex: hex::encode(&hash),
        };
        assert!(verify("pw", &stored));
        assert!(!verify("wrong", &stored));
    }

    #[test]
    fn different_salts_differ() {
        let a = hash_passphrase("pw", &random_salt(), 1000);
        let b = hash_passphrase("pw", &random_salt(), 1000);
        assert_ne!(a, b);
    }

    #[test]
    fn generate_uses_default_iterations_and_verifies() {
        let stored = PassphraseHash::generate("unlock");
        assert_eq!(stored.iterations, PBKDF2_ITERATIONS);
        assert_eq!(hex::decode(&stored.salt_hex).unwrap().len(), SALT_LEN);
        assert!(verify("unlock", &stored));
        assert!(!verify("Unlock", &stored));
    }

    #[test]
    fn empty_passphrase_round_trips() {
        let stored = PassphraseHash::generate("");
        assert!(verify("", &stored));
        assert!(!verify("x", &stored));
    }

    #[test]
    fn malformed_stored_hash_is_rejected() {
        let stored = PassphraseHash {
            salt_hex: "zz".into(),
            iterations: 10,
            hash_hex: "nothex".into(),
        };
        assert!(!verify("pw", &stored));
    }
}
