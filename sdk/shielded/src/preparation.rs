//! Local preparation of an agreement-bound private transfer.
//!
//! Prepared evidence contains only contract calldata. Input reservations and change
//! openings are durable before proving. Submission and reconciliation are caller-owned.

use ark_ff::{BigInteger, PrimeField};
use erebus_coordinator::{Coordinator, Error as CoordinatorError};
use erebus_core::{
    commitment::{commit_agreement, deal_nullifier, CommitmentError},
    settlement::{
        check_capabilities, BackendCapabilities, PreparedSettlement, SelectionError,
        SettlementContext,
    },
    shielded::SHIELDED_GUARANTEES,
    suite::{keccak256, SHIELDED_POSEIDON_EDDSA_SUITE_ID},
    terms::{GuaranteeSet, SettlementMode, TermsError},
};

use crate::{
    build_transfer_witness_for_operation,
    wallet::{WalletError, WalletStore},
    LocalProof, ProverError, ProvingArtifacts, TransferWitness, WalletTransferRequest,
};

const TRANSFER_SIGNATURE: &[u8] =
    b"transferPrivate(uint256[2],uint256[2][2],uint256[2],uint256[11])";
const TRANSFER_CALL_BYTES: usize = 4 + 19 * 32;

/// A local agreement or prepared transfer failed validation.
#[derive(Debug, thiserror::Error)]
pub enum PreparationError {
    /// Durable wallet transition failed; no proof is produced.
    #[error(transparent)]
    Wallet(#[from] WalletError),
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
    for field in [
        proof.proof.a.x,
        proof.proof.a.y,
        proof.proof.b.x.c1,
        proof.proof.b.x.c0,
        proof.proof.b.y.c1,
        proof.proof.b.y.c0,
        proof.proof.c.x,
        proof.proof.c.y,
    ] {
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
    chain[24..].copy_from_slice(
        &context
            .domain
            .namespace
            .reference()
            .parse::<u64>()
            .map_err(|_| PreparationError::Context)?
            .to_be_bytes(),
    );
    let mut pool = [0; 32];
    pool[12..].copy_from_slice(
        context
            .domain
            .pool
            .as_ref()
            .ok_or(PreparationError::Context)?
            .as_bytes(),
    );
    let mut version = [0; 32];
    version[28..].copy_from_slice(&context.domain.verifier_version.to_be_bytes());
    let asset_ref = context.asset.asset_reference();
    if context.asset.asset_namespace() != "erc20"
        || asset_ref.len() != 42
        || !asset_ref.starts_with("0x")
        || !asset_ref[2..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(PreparationError::Context);
    }
    let mut asset = [0; 32];
    asset[12..]
        .copy_from_slice(&hex::decode(&asset_ref[2..]).map_err(|_| PreparationError::Context)?);
    for (index, expected) in [
        chain,
        pool,
        version,
        asset,
        *prepared.deal_commitment.as_bytes(),
        *prepared.deal_nullifier.as_bytes(),
    ]
    .iter()
    .enumerate()
    {
        if &signals[index * 32..(index + 1) * 32] != expected {
            return Err(PreparationError::Evidence);
        }
    }
    Ok(())
}

/// Validates a transfer and persists its reservation and change before returning a witness.
/// The store's locked snapshot is authoritative; `request.wallet` is not used.
/// Retries must retain the same operation, input, agreement, and change opening.
pub fn prepare_transfer_witness(
    context: &SettlementContext,
    store: &WalletStore,
    request: &WalletTransferRequest<'_>,
    input: [u8; 32],
    operation_ref: [u8; 32],
) -> Result<TransferWitness, PreparationError> {
    check_capabilities(context, &capabilities())?;
    request.terms.validate()?;
    if operation_ref == [0; 32]
        || request.terms.domain != context.domain
        || request.terms.asset != context.asset
        || request.terms.suite_id != context.suite_id
        || request.terms.settlement_mode != context.mode
        || request.terms.required_guarantees != context.required_guarantees
        || context.domain.namespace.reference().parse::<u64>().ok() != Some(store.domain().chain_id)
        || context.domain.pool.as_ref().map(|pool| pool.as_bytes())
            != Some(store.domain().pool.as_slice())
    {
        return Err(PreparationError::Context);
    }
    let commitment = commit_agreement(request.terms, request.blinding)?;
    let nullifier = deal_nullifier(request.terms)?;
    let binding = keccak256(&[
        b"erebus/shielded/reservation/v1",
        commitment.as_bytes(),
        nullifier.as_bytes(),
    ]);
    Ok(store.update(|wallet| {
        let current = WalletTransferRequest { wallet, ..*request };
        let witness = build_transfer_witness_for_operation(&current, &input, operation_ref)
            .map_err(|_| WalletError::Note("transfer witness validation"))?;
        let amount = wallet
            .spendable_for(&input, operation_ref)
            .ok_or(WalletError::Note("transfer input unavailable"))?
            .amount();
        let change = request
            .change
            .owned_note(request.terms, amount)
            .map_err(|_| WalletError::Note("change opening"))?;
        // Bind zero-value change too: it is public proof data even without a wallet note.
        let binding = keccak256(&[&binding, &request.change.spend_secret, &request.change.salt]);
        wallet.reserve_transfer(operation_ref, input, binding, change)?;
        Ok(witness)
    })?)
}

/// Persists the input reservation and change opening, then proves locally.
/// Proof failures retain the reservation for retry or explicit absence reconciliation.
/// Returned calldata can be submitted without a further wallet write.
pub fn prepare_transfer(
    context: &SettlementContext,
    artifacts: &ProvingArtifacts,
    store: &WalletStore,
    request: &WalletTransferRequest<'_>,
    input: [u8; 32],
    operation_ref: [u8; 32],
) -> Result<PreparedSettlement, PreparationError> {
    let witness = prepare_transfer_witness(context, store, request, input, operation_ref)?;
    let commitment = commit_agreement(request.terms, request.blinding)?;
    let nullifier = deal_nullifier(request.terms)?;
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

/// Proves a shielded transfer only after the coordinator has persisted its agreement,
/// authorizations, and spending reservation. The wallet reserves its input and change before
/// proving. A failed proof leaves both reservations held for recovery.
///
/// The accepted terms, opening, and authorizations come from the coordinator's durable record;
/// caller-supplied copies in `request` cannot replace them. This method does not broadcast or
/// establish settlement finality.
///
/// This synchronous prover performs blocking work. Async callers must use a blocking worker
/// or `tokio::task::block_in_place` on a multi-thread runtime, not call it directly in a task.
#[allow(clippy::too_many_arguments)]
pub fn prepare_coordinated_transfer(
    coordinator: &Coordinator,
    context: &SettlementContext,
    artifacts: &ProvingArtifacts,
    store: &WalletStore,
    request: &WalletTransferRequest<'_>,
    input: [u8; 32],
    operation_ref: [u8; 32],
    now: u64,
) -> Result<PreparedSettlement, CoordinatorError> {
    coordinator.prepare(operation_ref, now, |terms, blinding, buyer, seller| {
        let accepted = WalletTransferRequest {
            terms,
            blinding,
            buyer,
            seller,
            now,
            ..*request
        };
        prepare_transfer(context, artifacts, store, &accepted, input, operation_ref)
    })
}
