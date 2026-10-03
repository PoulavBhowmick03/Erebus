//! Encrypted local note state for the experimental shielded EVM backend.
//!
//! The operator supplies the encryption key. This module never sends note openings to a
//! relay or indexer, and it does not infer chain finality from an RPC response.

use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
    XChaCha20Poly1305,
};
use erebus_journal::{Boundary, FaultHook, NoFaults, Step};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use erebus_core::{
    shielded::{note_commitment, note_nullifier, note_spend_tag, ShieldedDeal},
    terms::AgreementTerms,
};

use crate::{ChangeNote, InputNote};

const MAGIC: &[u8; 8] = b"ERBWL001";
const NONCE_BYTES: usize = 24;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Errors never include plaintext wallet contents or secret note fields.
#[derive(Debug, thiserror::Error)]
pub enum WalletError {
    /// Local filesystem operation failed.
    #[error("wallet filesystem operation failed")]
    Io(#[from] std::io::Error),
    /// A path is a symlink or has unsafe local permissions.
    #[error("wallet path is not a private regular file or directory")]
    UnsafePath,
    /// File size, version, or encoding is invalid.
    #[error("wallet file is malformed or unsupported")]
    InvalidFile,
    /// Wrong key, wrong deployment, or tampered ciphertext.
    #[error("wallet authentication failed")]
    Authentication,
    /// A note opening or state transition is invalid.
    #[error("invalid wallet note or transition: {0}")]
    Note(&'static str),
}

/// Chain instance and pool address bound into wallet authenticated encryption.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct WalletDomain {
    /// EVM chain ID.
    pub chain_id: u64,
    /// Deployed shielded pool address.
    pub pool: [u8; 20],
}

impl WalletDomain {
    fn aad(self) -> [u8; 36] {
        let mut aad = [0u8; 36];
        aad[..8].copy_from_slice(MAGIC);
        aad[8..16].copy_from_slice(&self.chain_id.to_be_bytes());
        aad[16..].copy_from_slice(&self.pool);
        aad
    }

    fn validate(self) -> Result<(), WalletError> {
        if self.chain_id == 0 || self.pool == [0; 20] {
            return Err(WalletError::Note("empty deployment domain"));
        }
        Ok(())
    }
}

/// Inclusion evidence from a public `NoteInserted` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteInclusion {
    /// Leaf index in the pool tree.
    pub index: u32,
    /// Block containing the event.
    pub block_number: u64,
    /// Hash of that block, used during reorg reconciliation.
    pub block_hash: [u8; 32],
    /// Root emitted for this insertion.
    pub root: [u8; 32],
}

/// Confirmed note consumption evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteConsumption {
    /// Block containing the successful spend.
    pub block_number: u64,
    /// Hash of that block.
    pub block_hash: [u8; 32],
    /// Transaction that consumed the note.
    pub tx_hash: [u8; 32],
}

/// A locally held note opening and its separately reconciled chain evidence.
#[derive(Clone, Serialize, Deserialize)]
pub struct OwnedNote {
    asset: [u8; 20],
    amount: u128,
    owner_x: [u8; 32],
    owner_y: [u8; 32],
    spend_secret: [u8; 32],
    salt: [u8; 32],
    commitment: [u8; 32],
    nullifier: [u8; 32],
    inclusion: Option<NoteInclusion>,
    consumption: Option<NoteConsumption>,
    reserved_operation: Option<[u8; 32]>,
}

impl Drop for OwnedNote {
    fn drop(&mut self) {
        self.spend_secret.zeroize();
        self.salt.zeroize();
    }
}

impl OwnedNote {
    /// Reconstructs a seller-owned payment note from accepted private terms.
    ///
    /// The seller must already hold the spend secret whose tag it authorized in the deal.
    /// This does not mark the note included; the public index must observe its commitment.
    pub fn expected_payment(
        terms: &AgreementTerms,
        spend_secret: [u8; 32],
    ) -> Result<Self, WalletError> {
        let deal =
            ShieldedDeal::from_terms(terms).map_err(|_| WalletError::Note("agreement shape"))?;
        let tag =
            note_spend_tag(&spend_secret).map_err(|_| WalletError::Note("seller spend secret"))?;
        if terms.payment_recipient.as_bytes() != tag {
            return Err(WalletError::Note("seller spend tag mismatch"));
        }
        let asset_ref = terms.asset.asset_reference();
        let asset: [u8; 20] = hex::decode(
            asset_ref
                .strip_prefix("0x")
                .ok_or(WalletError::Note("asset"))?,
        )
        .map_err(|_| WalletError::Note("asset"))?
        .try_into()
        .map_err(|_| WalletError::Note("asset"))?;
        let owner: [u8; 64] = terms
            .seller_authorization_key
            .as_bytes()
            .try_into()
            .map_err(|_| WalletError::Note("seller key"))?;
        let note = Self::new(
            asset,
            terms.amount.get(),
            owner,
            spend_secret,
            deal.payment_salt()
                .map_err(|_| WalletError::Note("payment salt"))?,
        )?;
        if note.commitment
            != deal
                .payment_note_commitment()
                .map_err(|_| WalletError::Note("payment commitment"))?
        {
            return Err(WalletError::Note("payment commitment mismatch"));
        }
        Ok(note)
    }

    /// Validates a local opening and computes its commitment and spend nullifier.
    pub fn new(
        asset: [u8; 20],
        amount: u128,
        owner: [u8; 64],
        spend_secret: [u8; 32],
        salt: [u8; 32],
    ) -> Result<Self, WalletError> {
        if amount == 0 {
            return Err(WalletError::Note("zero-value owned note"));
        }
        let tag = note_spend_tag(&spend_secret).map_err(|_| WalletError::Note("spend secret"))?;
        let commitment = note_commitment(&asset, amount, &owner, &tag, &salt)
            .map_err(|_| WalletError::Note("note opening"))?;
        let nullifier = note_nullifier(&spend_secret, &commitment)
            .map_err(|_| WalletError::Note("note nullifier"))?;
        let mut owner_x = [0u8; 32];
        let mut owner_y = [0u8; 32];
        owner_x.copy_from_slice(&owner[..32]);
        owner_y.copy_from_slice(&owner[32..]);
        Ok(Self {
            asset,
            amount,
            owner_x,
            owner_y,
            spend_secret,
            salt,
            commitment,
            nullifier,
            inclusion: None,
            consumption: None,
            reserved_operation: None,
        })
    }

    /// Commitment used to match a public insertion event.
    pub fn commitment(&self) -> [u8; 32] {
        self.commitment
    }

    /// One-time note spend identity.
    pub fn nullifier(&self) -> [u8; 32] {
        self.nullifier
    }

    /// Note value in token base units.
    pub fn amount(&self) -> u128 {
        self.amount
    }

    /// Asset address committed by this note.
    pub fn asset(&self) -> [u8; 20] {
        self.asset
    }

    /// Authorization key that owns this note, as two canonical field elements.
    pub fn owner(&self) -> [u8; 64] {
        let mut owner = [0u8; 64];
        owner[..32].copy_from_slice(&self.owner_x);
        owner[32..].copy_from_slice(&self.owner_y);
        owner
    }

    /// Current insertion evidence, if confirmed by the caller's indexer.
    pub fn inclusion(&self) -> Option<NoteInclusion> {
        self.inclusion
    }

    /// Returns the spend opening only to the local caller.
    pub fn input_note(&self) -> InputNote {
        InputNote {
            amount: self.amount,
            spend_secret: self.spend_secret,
            salt: self.salt,
        }
    }

    fn validate(&self) -> Result<(), WalletError> {
        let mut owner = [0u8; 64];
        owner[..32].copy_from_slice(&self.owner_x);
        owner[32..].copy_from_slice(&self.owner_y);
        let reconstructed =
            Self::new(self.asset, self.amount, owner, self.spend_secret, self.salt)?;
        if reconstructed.commitment != self.commitment || reconstructed.nullifier != self.nullifier
        {
            return Err(WalletError::Note("stored opening mismatch"));
        }
        if self.consumption.is_some() && self.inclusion.is_none() {
            return Err(WalletError::Note("consumed note without inclusion"));
        }
        Ok(())
    }
}

/// One deployment's encrypted note inventory. Never serialize this outside the store.
#[derive(Clone, Serialize, Deserialize)]
pub struct WalletSnapshot {
    version: u32,
    notes: Vec<OwnedNote>,
    #[serde(default)]
    transfers: Vec<TransferReservation>,
    #[serde(default)]
    choices: Vec<TransferChoice>,
}

/// Private input and change opening retained across proof failures, including zero change.
/// Only the encrypted wallet store may serialize this record.
#[derive(Clone, Serialize, Deserialize)]
pub struct TransferChoice {
    operation: [u8; 32],
    agreement: [u8; 32],
    input: [u8; 32],
    spend_secret: [u8; 32],
    salt: [u8; 32],
}

impl TransferChoice {
    /// The input reserved for this operation.
    pub fn input(&self) -> [u8; 32] {
        self.input
    }

    /// Copies the retained change opening into the prover request.
    pub fn change(&self) -> ChangeNote {
        ChangeNote {
            spend_secret: self.spend_secret,
            salt: self.salt,
        }
    }
}

impl Drop for TransferChoice {
    fn drop(&mut self) {
        self.spend_secret.zeroize();
        self.salt.zeroize();
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TransferReservation {
    operation: [u8; 32],
    input: [u8; 32],
    binding: [u8; 32],
    change: Option<[u8; 32]>,
    released: bool,
}

impl Default for WalletSnapshot {
    fn default() -> Self {
        Self {
            version: 1,
            notes: Vec::new(),
            transfers: Vec::new(),
            choices: Vec::new(),
        }
    }
}

impl WalletSnapshot {
    /// Selects and reserves an input once, retaining both change fields before proving.
    /// Call through `WalletStore::update`. Existing legacy reservations need their original
    /// opening; this method never invents a replacement for them.
    pub fn choose_transfer(
        &mut self,
        terms: &AgreementTerms,
        blinding: &erebus_core::commitment::CommitmentBlinding,
        operation: [u8; 32],
    ) -> Result<TransferChoice, WalletError> {
        use ark_ff::{BigInteger, PrimeField};
        use ark_std::{rand::rngs::OsRng, UniformRand};
        use erebus_core::commitment::{commit_agreement, deal_nullifier};
        use erebus_core::suite::keccak256;

        ShieldedDeal::from_terms(terms).map_err(|_| WalletError::Note("agreement shape"))?;
        let commitment = commit_agreement(terms, blinding)
            .map_err(|_| WalletError::Note("agreement commitment"))?;
        let nullifier =
            deal_nullifier(terms).map_err(|_| WalletError::Note("agreement nullifier"))?;
        let agreement = keccak256(&[
            b"erebus/shielded/input-choice/v1",
            commitment.as_bytes(),
            nullifier.as_bytes(),
        ]);
        if operation == [0; 32] {
            return Err(WalletError::Note("zero operation identity"));
        }
        if let Some(choice) = self
            .choices
            .iter()
            .find(|choice| choice.operation == operation)
        {
            if choice.agreement != agreement
                || self.spendable_for(&choice.input, operation).is_none()
            {
                return Err(WalletError::Note("operation binding mismatch"));
            }
            return Ok(choice.clone());
        }
        if self.transfers.iter().any(|r| r.operation == operation)
            || self
                .notes
                .iter()
                .any(|n| n.reserved_operation == Some(operation))
        {
            return Err(WalletError::Note(
                "retained input requires its original change opening",
            ));
        }
        let asset = erebus_evm::deployment::parse_lowercase_address(terms.asset.asset_reference())
            .ok_or(WalletError::Note("agreement asset"))?;
        let input = self
            .notes
            .iter()
            .filter(|note| {
                note.asset == asset
                    && note.owner() == terms.buyer_authorization_key.as_bytes()
                    && note.amount >= terms.amount.get()
                    && note.inclusion.is_some()
                    && note.consumption.is_none()
                    && note.reserved_operation.is_none()
            })
            .min_by_key(|note| (note.amount, note.commitment))
            .ok_or(WalletError::Note("no funded buyer note covers the payment"))?
            .commitment;
        let random_field = || {
            let bytes = ark_bn254::Fr::rand(&mut OsRng).into_bigint().to_bytes_be();
            let mut field = [0; 32];
            field[32 - bytes.len()..].copy_from_slice(&bytes);
            field
        };
        let choice = TransferChoice {
            operation,
            agreement,
            input,
            spend_secret: random_field(),
            salt: random_field(),
        };
        self.reserve(&input, operation)?;
        self.choices.push(choice.clone());
        Ok(choice)
    }
    /// Finds an explicit input available to this operation, including its own reservation.
    pub fn spendable_for(&self, commitment: &[u8; 32], operation: [u8; 32]) -> Option<&OwnedNote> {
        if operation == [0; 32] {
            return None;
        }
        self.notes.iter().find(|note| {
            &note.commitment == commitment
                && note.inclusion.is_some()
                && note.consumption.is_none()
                && note
                    .reserved_operation
                    .is_none_or(|owner| owner == operation)
        })
    }

    /// Binds an operation to its input, agreement binding, and change opening.
    /// Call through `WalletStore::update` to make the transition durable before proving.
    pub fn reserve_transfer(
        &mut self,
        operation: [u8; 32],
        input: [u8; 32],
        binding: [u8; 32],
        change: Option<OwnedNote>,
    ) -> Result<(), WalletError> {
        if binding == [0; 32]
            || self.spendable_for(&input, operation).is_none()
            || self
                .notes
                .iter()
                .any(|note| note.reserved_operation == Some(operation) && note.commitment != input)
        {
            return Err(WalletError::Note("transfer input unavailable"));
        }
        let reservation = TransferReservation {
            operation,
            input,
            binding,
            change: change.as_ref().map(OwnedNote::commitment),
            released: false,
        };
        if let Some(existing) = self.transfers.iter().find(|r| r.operation == operation) {
            return if existing == &reservation {
                Ok(())
            } else {
                Err(WalletError::Note("operation binding mismatch"))
            };
        }
        if let Some(change) = change {
            self.add(change)?;
        }
        self.reserve(&input, operation)?;
        self.transfers.push(reservation);
        Ok(())
    }

    /// Read-only note inventory. Each note's secret remains private to this process.
    pub fn notes(&self) -> &[OwnedNote] {
        &self.notes
    }

    /// Adds an expected note, rejecting a duplicate commitment or nullifier.
    pub fn add(&mut self, note: OwnedNote) -> Result<(), WalletError> {
        note.validate()?;
        if self.notes.iter().any(|existing| {
            existing.commitment == note.commitment || existing.nullifier == note.nullifier
        }) {
            return Err(WalletError::Note("duplicate note"));
        }
        self.notes.push(note);
        Ok(())
    }

    /// Selects the smallest confirmed, unspent, unreserved note that covers `amount`.
    pub fn select(&self, asset: &[u8; 20], amount: u128) -> Option<&OwnedNote> {
        self.notes
            .iter()
            .filter(|note| {
                &note.asset == asset
                    && note.amount >= amount
                    && note.inclusion.is_some()
                    && note.consumption.is_none()
                    && note.reserved_operation.is_none()
            })
            .min_by_key(|note| (note.amount, note.commitment))
    }

    /// Finds one specific confirmed note that has not been spent or reserved.
    pub fn spendable(&self, commitment: &[u8; 32]) -> Option<&OwnedNote> {
        self.notes.iter().find(|note| {
            &note.commitment == commitment
                && note.inclusion.is_some()
                && note.consumption.is_none()
                && note.reserved_operation.is_none()
        })
    }

    /// Reserves one confirmed note for a durable operation before proof construction.
    pub fn reserve(
        &mut self,
        commitment: &[u8; 32],
        operation: [u8; 32],
    ) -> Result<(), WalletError> {
        if operation == [0; 32] {
            return Err(WalletError::Note("zero operation identity"));
        }
        if self.notes.iter().any(|note| {
            note.reserved_operation == Some(operation) && &note.commitment != commitment
        }) || self
            .transfers
            .iter()
            .any(|r| r.operation == operation && (r.released || &r.input != commitment))
        {
            return Err(WalletError::Note("operation binding mismatch"));
        }
        let note = self
            .notes
            .iter_mut()
            .find(|note| &note.commitment == commitment)
            .ok_or(WalletError::Note("unknown note"))?;
        if note.inclusion.is_none() || note.consumption.is_some() {
            return Err(WalletError::Note("note is not spendable"));
        }
        match note.reserved_operation {
            None => note.reserved_operation = Some(operation),
            Some(existing) if existing == operation => {}
            Some(_) => return Err(WalletError::Note("note reserved by another operation")),
        }
        Ok(())
    }

    /// Releases a reservation only when the caller has reconciled the operation as absent.
    pub fn release(
        &mut self,
        commitment: &[u8; 32],
        operation: [u8; 32],
    ) -> Result<(), WalletError> {
        let note = self
            .notes
            .iter_mut()
            .find(|note| &note.commitment == commitment)
            .ok_or(WalletError::Note("unknown note"))?;
        if note.reserved_operation != Some(operation) || note.consumption.is_some() {
            return Err(WalletError::Note("reservation mismatch"));
        }
        note.reserved_operation = None;
        if let Some(reservation) = self.transfers.iter_mut().find(|r| r.operation == operation) {
            reservation.released = true;
        }
        Ok(())
    }

    /// Applies one verified public insertion event to a precomputed local commitment.
    pub fn observe_insertion(
        &mut self,
        commitment: &[u8; 32],
        inclusion: NoteInclusion,
    ) -> Result<bool, WalletError> {
        let Some(note) = self
            .notes
            .iter_mut()
            .find(|note| &note.commitment == commitment)
        else {
            return Ok(false);
        };
        if inclusion.index >= 1 << 20 || inclusion.block_hash == [0; 32] {
            return Err(WalletError::Note("invalid insertion evidence"));
        }
        match note.inclusion {
            None => note.inclusion = Some(inclusion),
            Some(existing) if existing == inclusion => {}
            Some(_) => return Err(WalletError::Note("conflicting insertion")),
        }
        Ok(true)
    }

    /// Marks a note spent after an observed successful onchain nullifier event.
    pub fn observe_consumption(
        &mut self,
        nullifier: &[u8; 32],
        spend: NoteConsumption,
    ) -> Result<bool, WalletError> {
        let Some(note) = self
            .notes
            .iter_mut()
            .find(|note| &note.nullifier == nullifier)
        else {
            return Ok(false);
        };
        let included = note
            .inclusion
            .ok_or(WalletError::Note("spend before inclusion"))?;
        if spend.block_number < included.block_number
            || spend.block_hash == [0; 32]
            || spend.tx_hash == [0; 32]
        {
            return Err(WalletError::Note("invalid spend evidence"));
        }
        match note.consumption {
            None => note.consumption = Some(spend),
            Some(existing) if existing == spend => {}
            Some(_) => return Err(WalletError::Note("conflicting spend")),
        }
        Ok(true)
    }

    /// Drops chain-derived observations from a reorg height, retaining local openings and reservations.
    pub fn rewind_from(&mut self, height: u64) {
        for note in &mut self.notes {
            if note
                .inclusion
                .is_some_and(|inclusion| inclusion.block_number >= height)
            {
                note.inclusion = None;
                note.consumption = None;
            } else if note
                .consumption
                .is_some_and(|spend| spend.block_number >= height)
            {
                note.consumption = None;
            }
        }
    }

    fn validate(&self) -> Result<(), WalletError> {
        if self.version != 1 {
            return Err(WalletError::InvalidFile);
        }
        let mut commitments = HashSet::new();
        let mut nullifiers = HashSet::new();
        for note in &self.notes {
            note.validate()?;
            if !commitments.insert(note.commitment) || !nullifiers.insert(note.nullifier) {
                return Err(WalletError::Note("duplicate stored note"));
            }
        }
        let mut operations = HashSet::new();
        for transfer in &self.transfers {
            if transfer.operation == [0; 32]
                || transfer.binding == [0; 32]
                || !operations.insert(transfer.operation)
                || !self.notes.iter().any(|note| {
                    note.commitment == transfer.input
                        && (transfer.released
                            || note.reserved_operation == Some(transfer.operation))
                })
                || transfer.change.is_some_and(|change| {
                    change == transfer.input || !commitments.contains(&change)
                })
            {
                return Err(WalletError::Note("invalid transfer reservation"));
            }
        }
        let mut operations = HashSet::new();
        for choice in &self.choices {
            if choice.operation == [0; 32]
                || choice.agreement == [0; 32]
                || !operations.insert(choice.operation)
                || !commitments.contains(&choice.input)
            {
                return Err(WalletError::Note("invalid retained transfer choice"));
            }
        }
        Ok(())
    }
}

/// Encrypted, locked, atomic file store. The caller retains the 32-byte wallet key.
pub struct WalletStore {
    path: PathBuf,
    lock_path: PathBuf,
    domain: WalletDomain,
    key: Zeroizing<[u8; 32]>,
    faults: Arc<dyn FaultHook>,
}

impl WalletStore {
    /// Chain and pool bound into this wallet's authenticated encryption.
    pub fn domain(&self) -> WalletDomain {
        self.domain
    }

    /// Opens a store under a private directory without creating a plaintext wallet file.
    pub fn new(
        path: impl Into<PathBuf>,
        domain: WalletDomain,
        key: [u8; 32],
    ) -> Result<Self, WalletError> {
        Self::with_faults(path, domain, key, Arc::new(NoFaults))
    }

    /// Opens the same encrypted store with a hook after each durable filesystem step.
    pub fn with_faults(
        path: impl Into<PathBuf>,
        domain: WalletDomain,
        key: [u8; 32],
        faults: Arc<dyn FaultHook>,
    ) -> Result<Self, WalletError> {
        domain.validate()?;
        let path = path.into();
        let parent = path.parent().ok_or(WalletError::UnsafePath)?;
        create_private_directory(parent)?;
        private_directory(parent)?;
        safe_file(&path)?;
        let lock_path = path.with_extension("lock");
        safe_file(&lock_path)?;
        Ok(Self {
            path,
            lock_path,
            domain,
            key: Zeroizing::new(key),
            faults,
        })
    }

    /// Loads and authenticates the latest snapshot. An absent file is an empty wallet.
    pub fn snapshot(&self) -> Result<WalletSnapshot, WalletError> {
        let lock = self.lock()?;
        let result = self.read_locked();
        drop(lock);
        result
    }

    /// Applies one local state transition and persists it before returning success.
    pub fn update<T>(
        &self,
        change: impl FnOnce(&mut WalletSnapshot) -> Result<T, WalletError>,
    ) -> Result<T, WalletError> {
        let lock = self.lock()?;
        let mut snapshot = self.read_locked()?;
        let result = change(&mut snapshot)?;
        snapshot.validate()?;
        self.write_locked(&snapshot)?;
        drop(lock);
        Ok(result)
    }

    fn lock(&self) -> Result<File, WalletError> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        private_create(&mut options);
        let lock = options.open(&self.lock_path)?;
        private_file(&self.lock_path)?;
        FileExt::lock_exclusive(&lock)?;
        Ok(lock)
    }

    fn read_locked(&self) -> Result<WalletSnapshot, WalletError> {
        safe_file(&self.path)?;
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(WalletSnapshot::default())
            }
            Err(error) => return Err(error.into()),
        };
        if file.metadata()?.len() > MAX_FILE_BYTES {
            return Err(WalletError::InvalidFile);
        }
        let mut encrypted = Vec::new();
        file.take(MAX_FILE_BYTES + 1).read_to_end(&mut encrypted)?;
        if encrypted.len() as u64 > MAX_FILE_BYTES {
            return Err(WalletError::InvalidFile);
        }
        if encrypted.len() < MAGIC.len() + NONCE_BYTES + 16 || &encrypted[..MAGIC.len()] != MAGIC {
            return Err(WalletError::InvalidFile);
        }
        let cipher = XChaCha20Poly1305::new_from_slice(self.key.as_ref())
            .map_err(|_| WalletError::Authentication)?;
        let nonce = (&encrypted[MAGIC.len()..MAGIC.len() + NONCE_BYTES]).into();
        let plaintext = cipher
            .decrypt(
                nonce,
                Payload {
                    msg: &encrypted[MAGIC.len() + NONCE_BYTES..],
                    aad: &self.domain.aad(),
                },
            )
            .map_err(|_| WalletError::Authentication)?;
        let plaintext = Zeroizing::new(plaintext);
        let snapshot: WalletSnapshot =
            serde_json::from_slice(&plaintext).map_err(|_| WalletError::InvalidFile)?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    fn write_locked(&self, snapshot: &WalletSnapshot) -> Result<(), WalletError> {
        let plaintext =
            Zeroizing::new(serde_json::to_vec(snapshot).map_err(|_| WalletError::InvalidFile)?);
        let cipher = XChaCha20Poly1305::new_from_slice(self.key.as_ref())
            .map_err(|_| WalletError::Authentication)?;
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &plaintext,
                    aad: &self.domain.aad(),
                },
            )
            .map_err(|_| WalletError::Authentication)?;
        if ciphertext.len() as u64 > MAX_FILE_BYTES - (MAGIC.len() + NONCE_BYTES) as u64 {
            return Err(WalletError::InvalidFile);
        }
        let temp = self
            .path
            .with_extension(format!("tmp-{}", hex::encode(&nonce[..8])));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        private_create(&mut options);
        let mut file = options.open(&temp)?;
        let result = (|| {
            file.write_all(MAGIC)?;
            file.write_all(&nonce)?;
            file.write_all(&ciphertext)?;
            file.sync_all()?;
            self.faults.after(Boundary {
                step: Step::FileSynced,
                path: &temp,
            })?;
            fs::rename(&temp, &self.path)?;
            self.faults.after(Boundary {
                step: Step::Renamed,
                path: &self.path,
            })?;
            File::open(self.path.parent().ok_or(WalletError::UnsafePath)?)?.sync_all()?;
            self.faults.after(Boundary {
                step: Step::DirectorySynced,
                path: self.path.parent().ok_or(WalletError::UnsafePath)?,
            })?;
            Ok::<(), WalletError>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}

fn safe_file(path: &Path) -> Result<(), WalletError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.file_type().is_file() => Err(WalletError::UnsafePath),
        Ok(_) => private_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> Result<(), WalletError> {
    use std::os::unix::fs::DirBuilderExt;
    if !path.exists() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> Result<(), WalletError> {
    fs::create_dir_all(path)?;
    Ok(())
}

#[cfg(unix)]
fn private_file(path: &Path) -> Result<(), WalletError> {
    use std::os::unix::fs::PermissionsExt;
    if fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
        return Err(WalletError::UnsafePath);
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_file(_path: &Path) -> Result<(), WalletError> {
    Ok(())
}

#[cfg(unix)]
fn private_directory(path: &Path) -> Result<(), WalletError> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_dir() || meta.permissions().mode() & 0o077 != 0 {
        return Err(WalletError::UnsafePath);
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_directory(path: &Path) -> Result<(), WalletError> {
    if !fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(WalletError::UnsafePath);
    }
    Ok(())
}

#[cfg(unix)]
fn private_create(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn private_create(_options: &mut OpenOptions) {}
