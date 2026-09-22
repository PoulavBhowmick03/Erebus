//! Transport and agreement identities.
//!
//! Two roles are involved in offchain Eleusis and they are deliberately separate (decisions
//! D03). The **transport identity** is an X25519 keypair used only by the Noise handshake; the
//! **authorization identity** is a secp256k1 key whose address is the agreement's seller or
//! buyer authorization key under suite 1. The transport key is bound to the authorization
//! identity by the signed service descriptor (see [`crate::descriptor`]); neither key is ever
//! used for the other's purpose.
//!
//! Key material never appears in `Debug` output, CLI logs, or model-visible strings. Both types
//! implement `Debug` as a redacted placeholder.

use core::fmt;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use snow::params::DHChoice;
use snow::resolvers::{CryptoResolver, DefaultResolver};
use snow::types::Dh;

/// Length of an X25519 private or public key.
pub const TRANSPORT_KEY_BYTES: usize = 32;
/// Length of a suite-1 secp256k1 authorization key (an Ethereum-style address).
pub const AUTHORIZATION_KEY_BYTES: usize = 20;
/// Length of a suite-1 authorization signature (`r || s || v`).
pub const AUTHORIZATION_SIGNATURE_BYTES: usize = 65;

/// A transport or authorization key could not be created.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    /// The supplied private key was not the expected length.
    #[error("transport private key must be {expected} bytes, found {actual}")]
    KeyLength {
        /// Expected length.
        expected: usize,
        /// Actual length.
        actual: usize,
    },
    /// The cryptographic resolver could not generate a keypair.
    #[error("transport key generation failed: {0}")]
    Generation(String),
    /// A secp256k1 private key did not parse.
    #[error("authorization signing key is not valid")]
    InvalidSigningKey,
    /// Persistent transport-key storage failed.
    #[error("transport identity I/O error: {0}")]
    Io(String),
    /// A transport-key file is accessible to users other than its owner.
    #[error("transport identity file must have owner-only permissions")]
    InsecurePermissions,
}

/// The X25519 keypair used only by the Noise handshake.
#[derive(Clone, PartialEq, Eq)]
pub struct TransportIdentity {
    private: [u8; TRANSPORT_KEY_BYTES],
    public: [u8; TRANSPORT_KEY_BYTES],
}

impl fmt::Debug for TransportIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransportIdentity")
            .field("public", &hex::encode(self.public))
            .field("private", &"<redacted>")
            .finish()
    }
}

impl TransportIdentity {
    /// Generates a fresh identity from the operating system's entropy source.
    ///
    /// Uses the same resolver the Noise handshake uses, so the public key this returns is
    /// exactly the one the handshake will authenticate.
    pub fn generate() -> Result<Self, IdentityError> {
        let resolver = DefaultResolver;
        let mut diffie_hellman = resolver
            .resolve_dh(&DHChoice::Curve25519)
            .ok_or_else(|| IdentityError::Generation("no X25519 provider".to_owned()))?;
        let mut random = resolver
            .resolve_rng()
            .ok_or_else(|| IdentityError::Generation("no entropy provider".to_owned()))?;
        diffie_hellman.generate(random.as_mut());
        Self::from_derived(diffie_hellman.as_ref())
    }

    /// Rebuilds an identity from a stored private key.
    ///
    /// This is key material. Callers persist it under the same rules as other spending-adjacent
    /// secrets and never print it.
    pub fn from_private_key(private: [u8; TRANSPORT_KEY_BYTES]) -> Result<Self, IdentityError> {
        let resolver = DefaultResolver;
        let mut diffie_hellman = resolver
            .resolve_dh(&DHChoice::Curve25519)
            .ok_or_else(|| IdentityError::Generation("no X25519 provider".to_owned()))?;
        diffie_hellman.set(&private);
        let public = to_array(diffie_hellman.pubkey())?;
        Ok(Self { private, public })
    }

    /// Generates an identity and stores its private key without overwriting an existing key.
    pub fn generate_and_store(path: impl AsRef<Path>) -> Result<Self, IdentityError> {
        let identity = Self::generate()?;
        identity.store(path)?;
        Ok(identity)
    }

    /// Stores the private transport key in an owner-only file.
    pub fn store(&self, path: impl AsRef<Path>) -> Result<(), IdentityError> {
        let path = path.as_ref();
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(path)
            .map_err(|error| IdentityError::Io(error.to_string()))?;
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| IdentityError::Io(error.to_string()))?;
        file.write_all(&self.private)
            .map_err(|error| IdentityError::Io(error.to_string()))?;
        file.sync_all()
            .map_err(|error| IdentityError::Io(error.to_string()))
    }

    /// Loads an owner-only transport private key.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, IdentityError> {
        let path = path.as_ref();
        #[cfg(unix)]
        if fs::metadata(path)
            .map_err(|error| IdentityError::Io(error.to_string()))?
            .permissions()
            .mode()
            & 0o077
            != 0
        {
            return Err(IdentityError::InsecurePermissions);
        }
        let mut bytes = Vec::new();
        fs::File::open(path)
            .and_then(|mut file| file.read_to_end(&mut bytes))
            .map_err(|error| IdentityError::Io(error.to_string()))?;
        let private = bytes
            .try_into()
            .map_err(|bytes: Vec<u8>| IdentityError::KeyLength {
                expected: TRANSPORT_KEY_BYTES,
                actual: bytes.len(),
            })?;
        Self::from_private_key(private)
    }

    fn from_derived(diffie_hellman: &dyn Dh) -> Result<Self, IdentityError> {
        let private = to_array(diffie_hellman.privkey())?;
        let public = to_array(diffie_hellman.pubkey())?;
        Ok(Self { private, public })
    }

    /// Returns the public key to publish in a descriptor.
    #[must_use]
    pub fn public_key(&self) -> [u8; TRANSPORT_KEY_BYTES] {
        self.public
    }

    /// Returns the private key for the handshake.
    ///
    /// Deliberately `pub(crate)`: only [`crate::session`] may consume it, so no caller can
    /// accidentally serialize a transport secret into a message or log.
    pub(crate) fn private_key(&self) -> &[u8; TRANSPORT_KEY_BYTES] {
        &self.private
    }
}

/// The secp256k1 key that signs descriptors and becomes the suite-1 authorization key.
#[derive(Clone)]
pub struct AuthorizationIdentity {
    signing_key: k256::ecdsa::SigningKey,
}

impl fmt::Debug for AuthorizationIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizationIdentity")
            .field("address", &hex::encode(self.address()))
            .field("signing_key", &"<redacted>")
            .finish()
    }
}

impl AuthorizationIdentity {
    /// Wraps a 32-byte secp256k1 private key.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, IdentityError> {
        let signing_key = k256::ecdsa::SigningKey::from_slice(bytes)
            .map_err(|_| IdentityError::InvalidSigningKey)?;
        Ok(Self { signing_key })
    }

    /// Returns the suite-1 authorization key: the last 20 bytes of keccak256 of the uncompressed
    /// public key without its `0x04` prefix.
    #[must_use]
    pub fn address(&self) -> [u8; AUTHORIZATION_KEY_BYTES] {
        let verifying_key = self.signing_key.verifying_key();
        let point = verifying_key.to_encoded_point(false);
        let digest = keccak256(&point.as_bytes()[1..]);
        let mut address = [0u8; AUTHORIZATION_KEY_BYTES];
        address.copy_from_slice(&digest[12..]);
        address
    }

    /// Signs a 32-byte digest, returning the canonical low-`s` `r || s || v` form suite 1 verifies.
    #[must_use]
    pub fn sign_digest(&self, digest: &[u8; 32]) -> [u8; AUTHORIZATION_SIGNATURE_BYTES] {
        let (signature, recovery_id) = self
            .signing_key
            .sign_prehash_recoverable(digest)
            .expect("signing a fixed-width digest cannot fail");
        let mut bytes = [0u8; AUTHORIZATION_SIGNATURE_BYTES];
        bytes[..64].copy_from_slice(&signature.to_bytes());
        bytes[64] = recovery_id.to_byte();
        bytes
    }
}

fn to_array(bytes: &[u8]) -> Result<[u8; TRANSPORT_KEY_BYTES], IdentityError> {
    bytes.try_into().map_err(|_| IdentityError::KeyLength {
        expected: TRANSPORT_KEY_BYTES,
        actual: bytes.len(),
    })
}

fn keccak256(bytes: &[u8]) -> [u8; 32] {
    use sha3::Digest;
    sha3::Keccak256::digest(bytes).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_round_trip_through_their_private_keys() {
        let identity = TransportIdentity::generate().expect("entropy");
        let rebuilt = TransportIdentity::from_private_key(*identity.private_key())
            .expect("valid private key");
        assert_eq!(identity.public_key(), rebuilt.public_key());
    }

    #[test]
    fn identities_survive_owner_only_storage() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("transport.key");
        let identity = TransportIdentity::generate_and_store(&path).expect("stored identity");
        let loaded = TransportIdentity::load(&path).expect("loaded identity");
        assert_eq!(identity.public_key(), loaded.public_key());
        assert!(TransportIdentity::generate_and_store(&path).is_err());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(path).expect("metadata").permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn transport_identity_debug_hides_the_private_key() {
        let identity = TransportIdentity::generate().expect("entropy");
        let rendered = format!("{identity:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains(&hex::encode(identity.private_key())));
    }

    #[test]
    fn address_is_the_keccak_tail_of_the_public_key() {
        let signing_key = k256::ecdsa::SigningKey::from_slice(&[0x11; 32]).expect("valid key");
        let identity = AuthorizationIdentity::from_bytes(&[0x11; 32]).expect("valid key");
        let point = signing_key.verifying_key().to_encoded_point(false);
        let expected = keccak256(&point.as_bytes()[1..]);
        assert_eq!(identity.address(), expected[12..]);
    }

    #[test]
    fn descriptor_signatures_verify_under_the_suite() {
        let identity = AuthorizationIdentity::from_bytes(&[0x22; 32]).expect("valid key");
        let digest = [0x5a; 32];
        let signature = identity.sign_digest(&digest);
        let suite = erebus_core::suite::suite(1).expect("suite 1");
        suite
            .verify_authorization(&identity.address(), &digest, &signature)
            .expect("suite must verify its own signature");
    }
}
