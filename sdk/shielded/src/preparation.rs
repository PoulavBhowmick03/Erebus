//! Local preparation of an agreement-bound private transfer.
//!
//! Prepared evidence contains only contract calldata. Wallet reservations, change-note
//! persistence, submission, and receipt reconciliation remain the caller's responsibility.

use ark_ff::{BigInteger, PrimeField};
use erebus_core::{
    commitment::{commit_agreement, deal_nullifier, CommitmentError},
    settlement::{check_capabilities, BackendCapabilities, PreparedSettlement, SelectionError, SettlementContext},
    shielded::SHIELDED_GUARANTEES,
    suite::{keccak256, SHIELDED_POSEIDON_EDDSA_SUITE_ID},
    terms::{GuaranteeSet, SettlementMode, TermsError},
};

use crate::{build_transfer_witness_from_wallet, LocalProof, ProverError, ProvingArtifacts, WalletTransferRequest};

const TRANSFER_SIGNATURE: &[u8] = b"transferPrivate(uint256[2],uint256[2][2],uint256[2],uint256[11])";
const TRANSFER_CALL_BYTES: usize = 4 + 19 * 32;

/// A local agreement or prepared transfer failed validation.
#[derive(Debug, thiserror::Error)]
pub enum PreparationError {
    /// Terms cannot be represented by the selected suite.
    #[error(transparent)]
    Terms(#[from] TermsError),
    /// Commitment construction failed.
    #[error(transparent)]
    Commitment(#[from] CommitmentError),
    /// The requested deployment or guarantees are unsupported.
    #[error(transparent)]
    Selection(#[from] SelectionError),
    /// Local witness construction or proving failed.
    #[error(transparent)]
    Prover(#[from] ProverError),
    /// The deal or prepared metadata differs from the configured session.
    #[error("shielded transfer differs from configured settlement context")]
    Context,
    /// Public calldata does not match the prepared identity or has an invalid shape.
    #[error("shielded transfer calldata does not match its prepared identity")]
    Evidence,
}

/// Guarantees of the fixed M5 transfer statement. This is not a release-readiness claim.
pub fn capabilities() -> BackendCapabilities {
    BackendCapabilities {
        suites: [SHIELDED_POSEIDON_EDDSA_SUITE_ID].into_iter().collect(),
        modes: [SettlementMode::Shielded].into_iter().collect(),
        guarantees: GuaranteeSet::from_bits(SHIELDED_GUARANTEES).expect("fixed guarantee bits"),
        local_proving: true,
    }
}

fn word<F: PrimeField>(field: F) -> [u8; 32] {
    let bytes = field.into_bigint().to_bytes_be();
    let mut word = [0; 32];
    word[32 - bytes.len()..].copy_from_slice(&bytes);
    word
}

fn transfer_calldata(proof: &LocalProof) -> Result<Vec<u8>, PreparationError> {
    if proof.public_inputs.len() != 11 {
        return Err(PreparationError::Evidence);
    }
    let mut data = Vec::with_capacity(TRANSFER_CALL_BYTES);
    data.extend_from_slice(&keccak256(&[TRANSFER_SIGNATURE])[..4]);
    for field in [proof.proof.a.x, proof.proof.a.y, proof.proof.b.x.c1, proof.proof.b.x.c0,
        proof.proof.b.y.c1, proof.proof.b.y.c0, proof.proof.c.x, proof.proof.c.y] {
        data.extend_from_slice(&word(field));
    }
    for field in &proof.public_inputs {
        data.extend_from_slice(&word(*field));
    }
    Ok(data)
}

/// Checks configured context and public calldata binding, without verifying the proof.
///
/// The pool verifier must still check the proof and current chain state. This function
/// neither submits a transaction nor establishes payment success or finality.
pub fn validate_prepared(
    context: &SettlementContext,
    prepared: &PreparedSettlement,
) -> Result<(), PreparationError> {
    check_capabilities(context, &capabilities())?;
    if prepared.operation_ref == [0; 32]
        || prepared.domain != context.domain
        || prepared.mode != context.mode
        || prepared.required_guarantees != context.required_guarantees
        || context.required_guarantees.bits() != SHIELDED_GUARANTEES
    {
        return Err(PreparationError::Context);
    }
    let data = &prepared.backend_evidence;
    if data.len() != TRANSFER_CALL_BYTES || data[..4] != keccak256(&[TRANSFER_SIGNATURE])[..4] {
        return Err(PreparationError::Evidence);
    }
    let signals = &data[4 + 8 * 32..];
    let mut chain = [0; 32];
    chain[24..].copy_from_slice(&context.domain.namespace.reference().parse::<u64>()
        .map_err(|_| PreparationError::Context)?.to_be_bytes());
    let mut pool = [0; 32];
    pool[12..].copy_from_slice(context.domain.pool.as_ref()
        .ok_or(PreparationError::Context)?.as_bytes());
    let mut version = [0; 32];
    version[28..].copy_from_slice(&context.domain.verifier_version.to_be_bytes());
    let asset_ref = context.asset.asset_reference();
    if context.asset.asset_namespace() != "erc20" || asset_ref.len() != 42
        || !asset_ref.starts_with("0x")
        || !asset_ref[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(PreparationError::Context);
    }
    let mut asset = [0; 32];
    asset[12..].copy_from_slice(&hex::decode(&asset_ref[2..]).map_err(|_| PreparationError::Context)?);
    for (index, expected) in [chain, pool, version, asset, *prepared.deal_commitment.as_bytes(), *prepared.deal_nullifier.as_bytes()].iter().enumerate() {
        if &signals[index * 32..(index + 1) * 32] != expected {
            return Err(PreparationError::Evidence);
        }
    }
    Ok(())
}

/// Validates an accepted deal, constructs its witness from the wallet, and proves locally.
///
/// The returned evidence is the pool's ABI calldata: proof plus eleven public inputs.
/// It contains no note opening, blinding, plaintext terms, or authorizations. Before
/// submitting it, the caller must durably reserve the input and save the change opening.
pub fn prepare_transfer(
    context: &SettlementContext,
    artifacts: &ProvingArtifacts,
    request: &WalletTransferRequest<'_>,
    operation_ref: [u8; 32],
) -> Result<PreparedSettlement, PreparationError> {
    check_capabilities(context, &capabilities())?;
    request.terms.validate()?;
    if operation_ref == [0; 32] || request.terms.domain != context.domain
        || request.terms.asset != context.asset || request.terms.suite_id != context.suite_id
        || request.terms.settlement_mode != context.mode
        || request.terms.required_guarantees != context.required_guarantees {
        return Err(PreparationError::Context);
    }
    let commitment = commit_agreement(request.terms, request.blinding)?;
    let nullifier = deal_nullifier(request.terms)?;
    let witness = build_transfer_witness_from_wallet(request)?;
    let expected: Vec<_> = witness.public_inputs.iter().map(String::as_str).collect();
    let proof = artifacts.prove(&witness.input, &expected)?;
    let prepared = PreparedSettlement {
        operation_ref,
        deal_commitment: commitment,
        deal_nullifier: nullifier,
        domain: context.domain.clone(),
        mode: context.mode,
        required_guarantees: context.required_guarantees,
        backend_evidence: transfer_calldata(&proof)?,
    };
    validate_prepared(context, &prepared)?;
    Ok(prepared)
}
