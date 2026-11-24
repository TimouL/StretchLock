//! StretchLock core: configuration, break statistics, passphrase hashing,
//! logging setup, and the daemon IPC protocol.

pub mod config;
pub mod ipc;
pub mod logging;
pub mod passphrase;
pub mod scene;
pub mod stats;

pub use config::{Config, ConfigStore};
pub use passphrase::PassphraseHash;

#[cfg(test)]
pub(crate) mod testutil {
    use std::fs;
    use std::path::PathBuf;

    pub(crate) fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "stretchlock-core-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
