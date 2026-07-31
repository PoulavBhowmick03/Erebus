//! Secret-key file creation for direct SDK operators.
//!
//! The pool identity key is not a Starknet account key and has no external issuer. Erebus
//! generates it from OS entropy and writes it to a caller-chosen file. Python and agent
//! processes do not receive the value.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use rand::{rngs::OsRng, RngCore};
use starknet_crypto::get_public_key;
use starknet_types_core::felt::Felt;

/// Non-secret metadata for a newly created pool identity key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedPoolKey {
    /// Absolute path containing the private key.
    pub path: PathBuf,
    /// Public half that the pool registers for this identity.
    pub public_key: Felt,
}

/// Creates a new pool identity key file without exposing the private value.
///
/// The destination must be absolute and its parent directory must already exist. Existing
/// files are never overwritten. On Unix the file is created mode `0600`; callers should
/// keep its parent directory mode `0700`.
pub fn generate_pool_key_file(path: impl AsRef<Path>) -> Result<GeneratedPoolKey, KeyFileError> {
    let path = path.as_ref();
    if !path.is_absolute() {
        return Err(KeyFileError::RelativePath(path.to_path_buf()));
    }

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| KeyFileError::MissingParent(path.to_path_buf()))?;
    if !parent.is_dir() {
        return Err(KeyFileError::MissingParent(parent.to_path_buf()));
    }

    let private_key = random_pool_key();
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    set_create_mode(&mut options);
    let mut file = options.open(path).map_err(|source| KeyFileError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    if let Err(source) = write_key(&mut file, private_key) {
        return Err(KeyFileError::Io {
            path: path.to_path_buf(),
            source,
        });
    }

    Ok(GeneratedPoolKey {
        path: path.to_path_buf(),
        public_key: get_public_key(&private_key),
    })
}

fn random_pool_key() -> Felt {
    loop {
        // 31 bytes are below the Stark-curve scalar order and Felt modulus. This avoids
        // reduction bias while retaining 248 bits of OS entropy.
        let mut bytes = [0u8; 31];
        OsRng.fill_bytes(&mut bytes);
        let key = Felt::from_bytes_be_slice(&bytes);
        if key != Felt::ZERO {
            return key;
        }
    }
}

fn write_key(file: &mut File, private_key: Felt) -> std::io::Result<()> {
    writeln!(file, "{private_key:#x}")?;
    file.sync_all()
}

#[cfg(unix)]
fn set_create_mode(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_create_mode(_options: &mut OpenOptions) {}

/// Pool-key file creation failure.
#[derive(Debug, thiserror::Error)]
pub enum KeyFileError {
    /// Key paths must be absolute because CLI calls can use different working directories.
    #[error("pool key path must be absolute: {}", .0.display())]
    RelativePath(PathBuf),
    /// The generator does not create parent directories.
    #[error("pool key parent directory does not exist: {}", .0.display())]
    MissingParent(PathBuf),
    /// Filesystem failure, including refusal to overwrite an existing key.
    #[error("pool key file I/O at {}: {source}", path.display())]
    Io {
        /// Affected path.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "erebus-key-test-{}-{}-{name}",
            std::process::id(),
            rand::random::<u64>()
        ))
    }

    #[test]
    fn creates_a_nonzero_hex_key_without_overwriting() {
        let root = temporary_path("create");
        std::fs::create_dir(&root).expect("temporary key directory");
        let path = root.join("pool.key");

        let generated = generate_pool_key_file(&path).expect("pool key");
        let text = std::fs::read_to_string(&path).expect("key file");
        let private_key = Felt::from_hex(text.trim()).expect("hex felt");

        assert_ne!(private_key, Felt::ZERO);
        assert_eq!(generated.path, path);
        assert_eq!(generated.public_key, get_public_key(&private_key));

        let original = text;
        assert!(generate_pool_key_file(&path).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).expect("unchanged key file"),
            original
        );

        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn creates_the_key_mode_0600() {
        use std::os::unix::fs::PermissionsExt;

        let root = temporary_path("permissions");
        std::fs::create_dir(&root).expect("temporary key directory");
        let path = root.join("pool.key");
        generate_pool_key_file(&path).expect("pool key");

        let mode = std::fs::metadata(&path)
            .expect("key metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn rejects_relative_paths_and_missing_parents() {
        assert!(matches!(
            generate_pool_key_file("pool.key"),
            Err(KeyFileError::RelativePath(_))
        ));

        let root = temporary_path("missing");
        let path = root.join("pool.key");
        assert!(matches!(
            generate_pool_key_file(path),
            Err(KeyFileError::MissingParent(_))
        ));
    }
}
