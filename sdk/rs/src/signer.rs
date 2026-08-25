//! Who signs the account transaction, and where that key lives.
//!
//! Two different keys sign two different things in this SDK, and conflating them is the
//! usual mistake. The **pool key** is the STRK20 identity: it derives channel keys and note
//! locations, and it travels in `compile_actions` calldata. The **account key** signs the
//! Starknet invoke that carries a proof to the chain, and it never leaves this process. Only
//! the account key is abstracted here. The pool key is not a signer and must not become one.
//!
//! ## Why an interface rather than a `Felt`
//!
//! The account key used to be read in the client, passed as a `Felt` through several stack
//! frames, and handed to `sign`. Behind this trait it is read at the moment of signing and
//! dropped immediately, so the window in which key material exists in memory is one function
//! call rather than the length of a write. That is a custody improvement on its own,
//! independent of who implements the trait.
//!
//! It also makes the thing `plan.md` and `custody-design.md` both point at possible: a
//! hardware wallet, a browser wallet, or a scoped session key can implement `AccountSigner`
//! without the SDK ever seeing a private key at all. `CLAUDE.md` constraint 6 says key
//! material never leaves the SDK boundary; a signer lets it never enter.
//!
//! ## Why signing is async
//!
//! A local key signs instantly, so async buys nothing for [`LocalKeySigner`]. Every other
//! implementation needs I/O: a hardware wallet is a USB round trip, a browser wallet is a
//! user pressing a button, a session key may be an HTTP call. A synchronous trait would
//! force those to block a runtime thread, which is how a signing prompt becomes a deadlock.
//!
//! The signature is a boxed future rather than `async fn`, because the trait is used as
//! `&dyn AccountSigner` and native `async fn` in traits is not object safe. Written by hand
//! rather than with `async-trait` to avoid a dependency for one method.

use std::fmt::Debug;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use starknet_crypto::Signature;
use starknet_types_core::felt::Felt;

use crate::signing::{self, SigningError};

/// Errors from producing an account signature.
#[derive(Debug, thiserror::Error)]
pub enum SignerError {
    /// The key material could not be read.
    #[error("account key unavailable: {0}")]
    Unavailable(String),
    /// The signer refused. A hardware or wallet signer rejects rather than fails.
    #[error("the signer declined to sign")]
    Declined,
    /// The underlying signature operation failed.
    #[error(transparent)]
    Signing(#[from] SigningError),
}

/// A boxed, borrowing future. See the module note on object safety.
pub type SignFuture<'a> = Pin<Box<dyn Future<Output = Result<Signature, SignerError>> + Send + 'a>>;

/// Produces the account signature over a Starknet transaction hash.
///
/// Implementations must not retain key material beyond a call. `Debug` is required and must
/// never print a key: the SDK logs signer state on failure paths.
pub trait AccountSigner: Debug + Send + Sync {
    /// The account address whose signature this produces.
    ///
    /// The pool validates against this address's own account contract
    /// (`utils.cairo:383`), so a signer that signs for a different address produces a
    /// signature the pool rejects. Exposed so a caller can check the two agree before
    /// paying for a proof.
    fn address(&self) -> Felt;

    /// Signs `message_hash`.
    fn sign<'a>(&'a self, message_hash: &'a Felt) -> SignFuture<'a>;
}

/// Signs with a Stark private key held in a local file.
///
/// The default, and the only one the SDK ships. The file is read on each call and the key is
/// dropped when the call returns, so it is not resident between writes.
#[derive(Debug, Clone)]
pub struct LocalKeySigner {
    address: Felt,
    key_file: PathBuf,
}

impl LocalKeySigner {
    /// Binds a key file to the account address it signs for.
    pub fn new(address: Felt, key_file: impl AsRef<Path>) -> Self {
        Self {
            address,
            key_file: key_file.as_ref().to_path_buf(),
        }
    }

    /// The file this signer reads. The path, never the contents.
    pub fn key_file(&self) -> &Path {
        &self.key_file
    }
}

impl AccountSigner for LocalKeySigner {
    fn address(&self) -> Felt {
        self.address
    }

    fn sign<'a>(&'a self, message_hash: &'a Felt) -> SignFuture<'a> {
        Box::pin(async move {
            // Read here rather than in the caller: the key exists for this frame only.
            let text = std::fs::read_to_string(&self.key_file).map_err(|source| {
                SignerError::Unavailable(format!("{}: {source}", self.key_file.display()))
            })?;
            let key = Felt::from_hex(text.trim()).map_err(|_| {
                SignerError::Unavailable(format!(
                    "{} does not contain a hex Stark key",
                    self.key_file.display()
                ))
            })?;
            let signature = signing::sign(&key, message_hash)?;
            // `key` drops here. Nothing above this frame ever holds it.
            Ok(signature)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A signer holds a path, not a key, so its `Debug` cannot leak one.
    ///
    /// The address it prints is public by definition. What must never appear is the file's
    /// contents, which is why this writes a real secret and looks for it.
    #[test]
    fn a_signer_debug_cannot_leak_the_key_it_signs_with() {
        let secret = "0xdeadbeefcafe";
        let path = std::env::temp_dir().join(format!(
            "erebus-signer-debug-{}-{}.key",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::write(&path, format!("{secret}\n")).expect("key file");

        let signer = LocalKeySigner::new(Felt::from(1u8), &path);
        let rendered = format!("{signer:?}");

        assert!(
            !rendered.contains(secret),
            "the key reached a Debug rendering: {rendered}"
        );
        // The path is deliberately visible: an operator debugging a signer needs to know
        // which file it was pointed at, and a path is not a secret.
        assert!(rendered.contains("erebus-signer-debug"), "{rendered}");

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn a_missing_key_file_is_unavailable_rather_than_a_panic() {
        let signer = LocalKeySigner::new(Felt::from(1u8), "/tmp/erebus-absent-key-file");
        let error = signer
            .sign(&Felt::from(7u8))
            .await
            .expect_err("no key file");

        assert!(matches!(error, SignerError::Unavailable(_)), "{error:?}");
    }

    #[tokio::test]
    async fn a_local_signer_agrees_with_signing_directly() {
        let path = std::env::temp_dir().join(format!("erebus-signer-{}.key", std::process::id()));
        std::fs::write(&path, "0x1234\n").expect("key file");
        let hash = Felt::from(0x99u8);

        let signer = LocalKeySigner::new(Felt::from(1u8), &path);
        let through_signer = signer.sign(&hash).await.expect("signs");
        let directly = signing::sign(&Felt::from_hex_unchecked("0x1234"), &hash).expect("signs");

        assert_eq!(through_signer.r, directly.r);
        assert_eq!(through_signer.s, directly.s);

        std::fs::remove_file(&path).ok();
    }
}
