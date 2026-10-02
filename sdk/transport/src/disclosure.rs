//! Verification of one disclosed agreement from stored transcript evidence.
//!
//! This verifies the agreement only. A payment claim also needs independent backend
//! observation; a caller-supplied settlement receipt is not chain evidence.

use erebus_core::auth::{verify_authorization_signature, AuthError, Authorization, Role};
use erebus_core::commitment::{
    commit_agreement, deal_nullifier, CommitmentBlinding, CommitmentError, DealCommitment,
    DealNullifier,
};
use erebus_core::encoding::{EncodingError, Reader, Writer};
use erebus_core::suite;
use erebus_core::terms::AgreementTerms;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use zeroize::Zeroizing;

use crate::identity::{AuthorizationIdentity, DisclosureIdentity};
use crate::limits::MAX_MESSAGES_PER_DEAL;
use crate::message::{Message, MessageError};
use crate::store::{StoreError, TranscriptStore};
use crate::transcript::{Transcript, TranscriptError};

/// The private opening and selected transcript to disclose to one recipient.
/// It must be encrypted before it leaves the issuer's machine.
#[derive(Clone)]
pub struct SelectedAgreement {
    /// Canonical accepted terms, including the committed transcript root.
    pub terms: AgreementTerms,
    /// The secret that opens the agreement commitment.
    pub blinding: CommitmentBlinding,
    /// Buyer's authorization of the accepted commitment.
    pub buyer: Authorization,
    /// Seller's authorization of the accepted commitment.
    pub seller: Authorization,
    /// Transcript hash version, independent of the agreement suite.
    pub transcript_hash_version: u16,
    /// Only messages from this agreement's deal.
    pub messages: Vec<Message>,
}

impl core::fmt::Debug for SelectedAgreement {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("SelectedAgreement { <redacted> }")
    }
}

/// The agreement facts established by replay and both participant signatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedAgreement {
    /// The accepted deal identifier.
    pub deal_id: [u8; 16],
    /// The accepted revision.
    pub revision: u32,
    /// The authorized agreement commitment.
    pub commitment: DealCommitment,
    /// The one-time settlement identity shared by revisions of the deal.
    pub nullifier: DealNullifier,
    /// The transcript root signed as part of the agreement.
    pub transcript_root: [u8; 32],
}

/// A selected agreement was incomplete or did not match its transcript or signatures.
#[derive(Debug, thiserror::Error)]
pub enum DisclosureError {
    /// The stored messages cannot form one deal transcript.
    #[error(transparent)]
    Transcript(#[from] TranscriptError),
    /// The messages do not reproduce the root authorized in the terms.
    #[error("disclosed transcript does not match the accepted agreement")]
    TranscriptRoot,
    /// An authorization was supplied in the wrong role.
    #[error("disclosed authorizations must contain buyer and seller roles")]
    Roles,
    /// The agreement commitment or nullifier cannot be derived.
    #[error(transparent)]
    Commitment(#[from] CommitmentError),
    /// An authorization does not verify against the opened agreement.
    #[error(transparent)]
    Authorization(#[from] AuthError),
    /// Evidence encoding is malformed or exceeds a bound.
    #[error(transparent)]
    Encoding(#[from] EncodingError),
    /// A canonical agreement could not be decoded.
    #[error(transparent)]
    Terms(#[from] erebus_core::terms::TermsError),
    /// An individual transcript message could not be decoded.
    #[error(transparent)]
    Message(#[from] MessageError),
    /// The selected evidence exceeds the disclosure package bound.
    #[error("selected disclosure exceeds the size bound")]
    TooLarge,
    /// The package names an unsupported format version.
    #[error("unsupported disclosure evidence version")]
    UnsupportedVersion,
    /// The selected transcript could not be read from participant storage.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// No messages remain for the selected deal in participant storage.
    #[error("selected deal transcript is unavailable")]
    TranscriptUnavailable,
}

/// Maximum plaintext bytes in one selected-deal package, including 4,096 messages.
pub const MAX_DISCLOSURE_BYTES: usize = 48 * 1024 * 1024;

impl SelectedAgreement {
    /// Reconstructs export evidence from one participant's stored transcript.
    /// Supply the opening and authorizations from trusted durable agreement state.
    /// Missing data is not payment evidence; this never queries or submits to a chain.
    pub fn from_store(
        terms: AgreementTerms,
        blinding: CommitmentBlinding,
        buyer: Authorization,
        seller: Authorization,
        transcript_hash_version: u16,
        store: &dyn TranscriptStore,
        namespace: &str,
    ) -> Result<Self, DisclosureError> {
        let messages = store.messages(namespace, terms.deal_id)?;
        if messages.is_empty() {
            return Err(DisclosureError::TranscriptUnavailable);
        }
        let evidence = Self {
            terms,
            blinding,
            buyer,
            seller,
            transcript_hash_version,
            messages,
        };
        verify_selected_agreement(&evidence)?;
        Ok(evidence)
    }

    /// Encodes only this deal's evidence for encrypted export.
    pub fn encode(&self) -> Result<Vec<u8>, DisclosureError> {
        if self.messages.len() > MAX_MESSAGES_PER_DEAL {
            return Err(DisclosureError::TooLarge);
        }
        let mut writer = Writer::new();
        writer.u16(1);
        writer.u16(self.transcript_hash_version);
        writer.bytes_bounded(&self.terms.encode()?, u16::MAX as usize);
        writer.fixed(self.blinding.as_bytes());
        writer.bytes(&self.buyer.encode()?);
        writer.bytes(&self.seller.encode()?);
        writer.u32(self.messages.len() as u32);
        for message in &self.messages {
            writer.bytes_bounded(&message.encode(), u16::MAX as usize);
        }
        let bytes = writer.finish();
        if bytes.len() > MAX_DISCLOSURE_BYTES {
            return Err(DisclosureError::TooLarge);
        }
        Ok(bytes)
    }

    /// Decodes a complete selected-deal package without accepting trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, DisclosureError> {
        if bytes.len() > MAX_DISCLOSURE_BYTES {
            return Err(DisclosureError::TooLarge);
        }
        let mut reader = Reader::new(bytes);
        if reader.u16("disclosure_version")? != 1 {
            return Err(DisclosureError::UnsupportedVersion);
        }
        let transcript_hash_version = reader.u16("transcript_hash_version")?;
        let terms = AgreementTerms::decode(reader.bytes("terms", 1, u16::MAX as usize)?)?;
        let blinding = CommitmentBlinding::from_bytes(reader.fixed("blinding")?);
        let buyer = Authorization::decode(reader.bytes("buyer_authorization", 1, 1024)?)?;
        let seller = Authorization::decode(reader.bytes("seller_authorization", 1, 1024)?)?;
        let count = reader.u32("message_count")? as usize;
        if count > MAX_MESSAGES_PER_DEAL {
            return Err(DisclosureError::TooLarge);
        }
        let mut messages = Vec::with_capacity(count);
        for _ in 0..count {
            messages.push(Message::decode(reader.bytes(
                "message",
                1,
                u16::MAX as usize,
            )?)?);
        }
        reader.finish()?;
        Ok(Self {
            terms,
            blinding,
            buyer,
            seller,
            transcript_hash_version,
            messages,
        })
    }
}

/// A recipient-bound, issuer-signed encrypted grant for one deal.
#[derive(Clone)]
pub struct DisclosureGrant {
    /// Suite-1 address (20 bytes) or suite-2 participant key (64 bytes).
    /// The recipient must obtain the expected identity independently.
    pub issuer: Vec<u8>,
    /// Intended disclosure recipient's X25519 public key.
    pub recipient: [u8; 32],
    /// Deal selected by this grant.
    pub deal_id: [u8; 16],
    /// Unix time after which a recipient must reject the grant.
    pub expires_at: u64,
    handshake: Vec<u8>,
    frames: Vec<Vec<u8>>,
    version: u16,
    signature: Vec<u8>,
}

impl core::fmt::Debug for DisclosureGrant {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("DisclosureGrant { <redacted> }")
    }
}

/// Grant creation or opening failed.
#[derive(Debug, thiserror::Error)]
pub enum GrantError {
    /// The disclosure evidence failed verification or canonical decoding.
    #[error(transparent)]
    Evidence(#[from] DisclosureError),
    /// Grant structure or issuer signature is invalid.
    #[error("invalid disclosure grant")]
    Invalid,
    /// Recipient key does not match the signed grant.
    #[error("grant belongs to a different recipient")]
    WrongRecipient,
    /// The grant is past its acceptance deadline.
    #[error("grant has expired")]
    Expired,
    /// Encryption or decryption failed.
    #[error("disclosure encryption failed")]
    Cryptography,
    /// The encoded grant could not be parsed.
    #[error(transparent)]
    Encoding(#[from] EncodingError),
    /// A grant backup could not be read or written.
    #[error("grant backup I/O failed")]
    Backup,
}

const GRANT_DOMAIN: &[u8] = b"EREBUS_DEAL_DISCLOSURE_V1";
const SHIELDED_GRANT_DOMAIN: &[u8] = b"EREBUS_DEAL_DISCLOSURE_V2_SUITE2";
const NOISE_N: &str = "Noise_N_25519_ChaChaPoly_BLAKE2s";
const CHUNK_BYTES: usize = 60_000;
const MAX_FRAME_BYTES: usize = 65_535;
/// Maximum encoded grant bytes, including encryption and signature framing.
pub const MAX_GRANT_BYTES: usize = MAX_DISCLOSURE_BYTES + 1024 * 1024;

impl DisclosureGrant {
    /// Encodes the recipient-bound grant for transfer or local backup.
    pub fn encode(&self) -> Result<Vec<u8>, GrantError> {
        if !matches!(
            (self.version, self.issuer.len(), self.signature.len()),
            (1, 20, 65) | (2, 64, 96)
        ) {
            return Err(GrantError::Invalid);
        }
        let mut writer = Writer::new();
        writer.u16(self.version);
        writer.fixed(&self.issuer);
        writer.fixed(&self.recipient);
        writer.fixed(&self.deal_id);
        writer.u64(self.expires_at);
        writer.bytes(&self.handshake);
        writer.u32(self.frames.len() as u32);
        for frame in &self.frames {
            writer.bytes_bounded(frame, MAX_FRAME_BYTES);
        }
        writer.fixed(&self.signature);
        let bytes = writer.finish();
        if bytes.len() > MAX_GRANT_BYTES {
            return Err(GrantError::Invalid);
        }
        Ok(bytes)
    }

    /// Decodes an entire grant. Opening also checks the signature and recipient.
    pub fn decode(bytes: &[u8]) -> Result<Self, GrantError> {
        if bytes.len() > MAX_GRANT_BYTES {
            return Err(GrantError::Invalid);
        }
        let mut reader = Reader::new(bytes);
        let version = reader.u16("grant_version")?;
        let issuer = match version {
            1 => reader.fixed::<20>("issuer")?.to_vec(),
            2 => reader.fixed::<64>("issuer")?.to_vec(),
            _ => return Err(GrantError::Invalid),
        };
        let recipient = reader.fixed("recipient")?;
        let deal_id = reader.fixed("deal_id")?;
        let expires_at = reader.u64("expires_at")?;
        let handshake = reader.bytes("handshake", 1, MAX_FRAME_BYTES)?.to_vec();
        let count = reader.u32("frame_count")? as usize;
        if count == 0 || count > MAX_DISCLOSURE_BYTES / CHUNK_BYTES + 1 {
            return Err(GrantError::Invalid);
        }
        let mut frames = Vec::with_capacity(count);
        for _ in 0..count {
            frames.push(reader.bytes("frame", 16, MAX_FRAME_BYTES)?.to_vec());
        }
        let signature = match version {
            1 => reader.fixed::<65>("signature")?.to_vec(),
            2 => reader.fixed::<96>("signature")?.to_vec(),
            _ => return Err(GrantError::Invalid),
        };
        reader.finish()?;
        Ok(Self {
            issuer,
            recipient,
            deal_id,
            expires_at,
            handshake,
            frames,
            signature,
            version,
        })
    }

    /// Writes one owner-only backup without replacing an existing file.
    /// The file may be partial after an I/O failure; `load_backup` rejects it.
    pub fn write_backup(&self, path: impl AsRef<Path>) -> Result<(), GrantError> {
        let path = path.as_ref();
        let bytes = self.encode()?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(path).map_err(|_| GrantError::Backup)?;
        file.write_all(&bytes).map_err(|_| GrantError::Backup)?;
        file.sync_all().map_err(|_| GrantError::Backup)?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::File::open(parent)
            .and_then(|dir| dir.sync_all())
            .map_err(|_| GrantError::Backup)?;
        Ok(())
    }

    /// Loads a grant from an owner-only backup file.
    pub fn load_backup(path: impl AsRef<Path>) -> Result<Self, GrantError> {
        let path = path.as_ref();
        #[cfg(unix)]
        if fs::metadata(path)
            .map_err(|_| GrantError::Backup)?
            .permissions()
            .mode()
            & 0o077
            != 0
        {
            return Err(GrantError::Backup);
        }
        let mut bytes = Vec::new();
        fs::File::open(path)
            .and_then(|file| {
                file.take((MAX_GRANT_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
            })
            .map_err(|_| GrantError::Backup)?;
        Self::decode(&bytes)
    }

    /// Encrypts verified evidence for a recipient who need not be online.
    pub fn seal(
        evidence: &SelectedAgreement,
        issuer: &AuthorizationIdentity,
        recipient: [u8; 32],
        expires_at: u64,
        now: u64,
    ) -> Result<Self, GrantError> {
        if evidence.terms.suite_id != 1 {
            return Err(GrantError::Invalid);
        }
        let mut grant = Self::encrypt(
            evidence,
            issuer.address().to_vec(),
            1,
            recipient,
            expires_at,
            now,
        )?;
        grant.signature = issuer.sign_digest(&grant.signature_digest()).to_vec();
        Ok(grant)
    }

    /// Signs a version-2 grant directly with a suite-2 agreement participant seed.
    /// This performs local signing only; no witness, circuit, or prover is involved.
    pub fn seal_shielded(
        evidence: &SelectedAgreement,
        issuer_seed: &[u8; 32],
        recipient: [u8; 32],
        expires_at: u64,
        now: u64,
    ) -> Result<Self, GrantError> {
        use erebus_core::{
            shielded::disclosure_message,
            shielded_auth::{derive_key, sign_message},
        };
        if evidence.terms.suite_id != 2
            || evidence.terms.settlement_mode != erebus_core::terms::SettlementMode::Shielded
        {
            return Err(GrantError::Invalid);
        }
        let issuer = derive_key(issuer_seed).map_err(|_| GrantError::Invalid)?;
        if evidence.terms.buyer_authorization_key.as_bytes() != issuer
            && evidence.terms.seller_authorization_key.as_bytes() != issuer
        {
            return Err(GrantError::Invalid);
        }
        let mut grant = Self::encrypt(evidence, issuer.to_vec(), 2, recipient, expires_at, now)?;
        let message =
            disclosure_message(&grant.signature_digest()).map_err(|_| GrantError::Invalid)?;
        let (key, signature) =
            sign_message(issuer_seed, &message).map_err(|_| GrantError::Invalid)?;
        if key != issuer {
            return Err(GrantError::Invalid);
        }
        grant.signature = signature.to_vec();
        Ok(grant)
    }

    fn encrypt(
        evidence: &SelectedAgreement,
        issuer: Vec<u8>,
        version: u16,
        recipient: [u8; 32],
        expires_at: u64,
        now: u64,
    ) -> Result<Self, GrantError> {
        if now >= expires_at || recipient == [0; 32] {
            return Err(GrantError::Invalid);
        }
        verify_selected_agreement(evidence)?;
        let plaintext = Zeroizing::new(evidence.encode()?);
        let mut grant = Self {
            issuer,
            recipient,
            deal_id: evidence.terms.deal_id,
            expires_at,
            handshake: Vec::new(),
            frames: Vec::new(),
            signature: Vec::new(),
            version,
        };
        let prologue = grant.header_digest();
        let params = NOISE_N.parse().map_err(|_| GrantError::Cryptography)?;
        let mut handshake = snow::Builder::new(params)
            .prologue(&prologue)
            .remote_public_key(&recipient)
            .build_initiator()
            .map_err(|_| GrantError::Cryptography)?;
        let mut buffer = vec![0u8; MAX_FRAME_BYTES];
        let written = handshake
            .write_message(&[], &mut buffer)
            .map_err(|_| GrantError::Cryptography)?;
        grant.handshake = buffer[..written].to_vec();
        let mut channel = handshake
            .into_transport_mode()
            .map_err(|_| GrantError::Cryptography)?;
        for chunk in plaintext.chunks(CHUNK_BYTES) {
            let written = channel
                .write_message(chunk, &mut buffer)
                .map_err(|_| GrantError::Cryptography)?;
            grant.frames.push(buffer[..written].to_vec());
        }
        Ok(grant)
    }

    /// Opens a grant and independently verifies the selected agreement.
    /// The caller supplies the independently expected issuer address or suite-2 key.
    pub fn open(
        &self,
        recipient: &DisclosureIdentity,
        expected_issuer: impl AsRef<[u8]>,
        now: u64,
    ) -> Result<(SelectedAgreement, VerifiedAgreement), GrantError> {
        if recipient.public_key() != self.recipient {
            return Err(GrantError::WrongRecipient);
        }
        if now >= self.expires_at {
            return Err(GrantError::Expired);
        }
        if expected_issuer.as_ref() != self.issuer
            || self.handshake.is_empty()
            || self.handshake.len() > MAX_FRAME_BYTES
            || self.frames.is_empty()
            || self.frames.len() > MAX_DISCLOSURE_BYTES / CHUNK_BYTES + 1
            || self
                .frames
                .iter()
                .any(|frame| frame.len() < 16 || frame.len() > MAX_FRAME_BYTES)
        {
            return Err(GrantError::Invalid);
        }
        match self.version {
            1 => suite::suite(1)
                .map_err(|_| GrantError::Invalid)?
                .verify_authorization(&self.issuer, &self.signature_digest(), &self.signature)
                .map_err(|_| GrantError::Invalid)?,
            2 => {
                let message = erebus_core::shielded::disclosure_message(&self.signature_digest())
                    .map_err(|_| GrantError::Invalid)?;
                erebus_core::shielded_auth::verify_message(&self.issuer, &message, &self.signature)
                    .map_err(|_| GrantError::Invalid)?;
            }
            _ => return Err(GrantError::Invalid),
        }
        let prologue = self.header_digest();
        let params = NOISE_N.parse().map_err(|_| GrantError::Cryptography)?;
        let mut handshake = snow::Builder::new(params)
            .prologue(&prologue)
            .local_private_key(recipient.private_key())
            .build_responder()
            .map_err(|_| GrantError::Cryptography)?;
        let mut buffer = vec![0u8; MAX_FRAME_BYTES];
        handshake
            .read_message(&self.handshake, &mut buffer)
            .map_err(|_| GrantError::Cryptography)?;
        let mut channel = handshake
            .into_transport_mode()
            .map_err(|_| GrantError::Cryptography)?;
        let mut plaintext = Zeroizing::new(Vec::new());
        for frame in &self.frames {
            let read = channel
                .read_message(frame, &mut buffer)
                .map_err(|_| GrantError::Cryptography)?;
            if plaintext
                .len()
                .checked_add(read)
                .is_none_or(|length| length > MAX_DISCLOSURE_BYTES)
            {
                return Err(GrantError::Invalid);
            }
            plaintext.extend_from_slice(&buffer[..read]);
        }
        let evidence = SelectedAgreement::decode(&plaintext)?;
        if evidence.terms.deal_id != self.deal_id {
            return Err(GrantError::Invalid);
        }
        let verified = verify_selected_agreement(&evidence)?;
        if (self.version == 1 && evidence.terms.suite_id != 1)
            || (self.version == 2
                && (evidence.terms.suite_id != 2
                    || evidence.terms.settlement_mode
                        != erebus_core::terms::SettlementMode::Shielded
                    || (evidence.terms.buyer_authorization_key.as_bytes() != self.issuer
                        && evidence.terms.seller_authorization_key.as_bytes() != self.issuer)))
        {
            return Err(GrantError::Invalid);
        }
        Ok((evidence, verified))
    }

    fn header_digest(&self) -> [u8; 32] {
        suite::keccak256(&[
            self.domain(),
            &self.issuer,
            &self.recipient,
            &self.deal_id,
            &self.expires_at.to_be_bytes(),
        ])
    }

    fn signature_digest(&self) -> [u8; 32] {
        let mut writer = Writer::new();
        writer.fixed(&self.header_digest());
        writer.bytes(&self.handshake);
        writer.u32(self.frames.len() as u32);
        for frame in &self.frames {
            writer.bytes_bounded(frame, MAX_FRAME_BYTES);
        }
        suite::keccak256(&[self.domain(), &writer.finish()])
    }

    fn domain(&self) -> &'static [u8] {
        if self.version == 2 {
            SHIELDED_GRANT_DOMAIN
        } else {
            GRANT_DOMAIN
        }
    }
}

/// Replays the selected transcript and verifies the accepted terms and both signatures.
/// Expiry does not erase a signature that was valid when the deal was accepted.
pub fn verify_selected_agreement(
    evidence: &SelectedAgreement,
) -> Result<VerifiedAgreement, DisclosureError> {
    if evidence.buyer.role != Role::Buyer || evidence.seller.role != Role::Seller {
        return Err(DisclosureError::Roles);
    }
    let transcript = Transcript::replay(
        evidence.terms.deal_id,
        evidence.transcript_hash_version,
        &evidence.messages,
    )?;
    if transcript.root()? != evidence.terms.transcript_root {
        return Err(DisclosureError::TranscriptRoot);
    }
    let commitment = commit_agreement(&evidence.terms, &evidence.blinding)?;
    verify_authorization_signature(
        &evidence.terms,
        &commitment,
        &evidence.blinding,
        &evidence.buyer,
    )?;
    verify_authorization_signature(
        &evidence.terms,
        &commitment,
        &evidence.blinding,
        &evidence.seller,
    )?;
    let nullifier = deal_nullifier(&evidence.terms)?;
    Ok(VerifiedAgreement {
        deal_id: evidence.terms.deal_id,
        revision: evidence.terms.revision,
        commitment,
        nullifier,
        transcript_root: evidence.terms.transcript_root,
    })
}
