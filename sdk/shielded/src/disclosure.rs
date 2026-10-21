//! Independent payment checks for an opened suite-2 disclosure.
//!
//! The caller must authenticate the grant issuer separately. This verifies the selected
//! agreement and its finalized pool payment, not an issuer-to-participant relationship.
//! Pool code and verifying keys must belong to a trusted deployment. RPC agreement is not
//! a contract audit or a proof of provider independence.

use erebus_core::{
    deal_state::{DealEvidence, DealReads, RevisionError, SignedRevision},
    ids::AssetId,
    settlement::SettlementContext,
    shielded::{ShieldedDeal, ShieldedMapError},
    suite,
};
use erebus_transport::disclosure::{
    verify_selected_agreement, DisclosureError, SelectedAgreement, VerifiedAgreement,
};
use serde::{Deserialize, Serialize};

use crate::{
    index_store::IndexStore,
    indexer::TreeCheckpoint,
    observation::{
        observe_shielded_deal_agreed, observe_shielded_deal_certified_bounded, ObservationError,
        SettlementLocation,
    },
    rpc::{PoolRpc, PoolRuntimePins},
};

/// New pool blocks each provider may scan in one certified verification before it is pending.
const CERTIFIED_SCAN_BLOCKS: u64 = 1_000;

/// One selected agreement whose payment matches independently observed finalized pool state.
/// This does not establish the identity of the grant issuer or service delivery.
pub struct FinalizedShieldedPayment {
    /// Private selected-deal evidence. Keep this local to the disclosure recipient.
    pub evidence: SelectedAgreement,
    /// Facts established by transcript replay and both participant authorizations.
    pub agreement: VerifiedAgreement,
    /// Final matching payment evidence obtained independently from the paired pool observer.
    pub settlement: DealReads,
}

impl core::fmt::Debug for FinalizedShieldedPayment {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("FinalizedShieldedPayment { <redacted> }")
    }
}

/// Facts a paired finalized observation established about one selected agreement's payment.
///
/// Every field except `observers` was read from both providers or the verified opening, never
/// copied from configuration. `observers` names the two configured RPC endpoints by
/// fingerprint. A certificate records what was observed once; it is not a consensus proof and
/// it assumes the finalized anchor does not revert.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaymentCertificate {
    /// EVM chain id the pool observers were configured for and checked against.
    pub chain_id: u64,
    /// Pool contract that emitted the settlement event.
    pub pool: [u8; 20],
    /// Keccak-256 of the agreement's canonical deployment domain encoding.
    pub domain_digest: [u8; 32],
    /// Pool verifier version read at the finalized anchor.
    pub verifier_version: u32,
    /// Pool ERC-20 asset read at the finalized anchor.
    pub asset: [u8; 20],
    /// Paid agreement commitment from the verified opening.
    pub commitment: [u8; 32],
    /// Deal nullifier from the verified opening.
    pub nullifier: [u8; 32],
    /// Paid amount in base units from the verified opening.
    pub amount: u128,
    /// RPC-finalized anchor number.
    pub finalized_number: u64,
    /// RPC-finalized anchor hash.
    pub finalized_hash: [u8; 32],
    /// RPC-finalized anchor timestamp.
    pub finalized_timestamp: u64,
    /// Canonical location of the settlement event, at or before the finalized anchor.
    pub settlement: SettlementLocation,
    /// Fingerprints of the first and second configured pool RPC endpoints.
    pub observers: [[u8; 32]; 2],
    /// Pool and dependency runtime pins read back from both providers at the finalized anchor.
    /// These are the verified pins, not the configuration they were checked against.
    pub runtime: PoolRuntimePins,
}

impl core::fmt::Debug for PaymentCertificate {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("PaymentCertificate { <redacted> }")
    }
}

impl PaymentCertificate {
    /// Whether this certificate describes exactly this verified agreement and is internally
    /// consistent. It checks binding only; it does not observe the chain again.
    #[must_use]
    pub fn binds(&self, evidence: &SelectedAgreement, agreement: &VerifiedAgreement) -> bool {
        let terms = &evidence.terms;
        let Ok(domain) = terms.domain.encode() else {
            return false;
        };
        let pool_matches = terms
            .domain
            .pool
            .as_ref()
            .is_some_and(|pool| pool.as_bytes() == self.pool.as_slice());
        let chain_matches = terms.domain.namespace.family() == "eip155"
            && terms.domain.namespace.reference().parse::<u64>().ok() == Some(self.chain_id);
        self.commitment == *agreement.commitment.as_bytes()
            && self.nullifier == *agreement.nullifier.as_bytes()
            && self.amount == terms.amount.get()
            && Some(self.asset) == erc20_address(&terms.asset)
            && self.verifier_version == terms.domain.verifier_version
            && self.domain_digest == suite::keccak256(&[&domain])
            && pool_matches
            && chain_matches
            && self.settlement.block_number <= self.finalized_number
            && self.observers[0] != self.observers[1]
    }
}

/// A finalized shielded payment and the certificate of the observation that established it.
pub struct CertifiedShieldedPayment {
    /// The verified payment, exactly as [`verify_shielded_payment`] returns it.
    pub payment: FinalizedShieldedPayment,
    /// Anchors, settlement location, and observer fingerprints of that verification.
    pub certificate: PaymentCertificate,
}

impl core::fmt::Debug for CertifiedShieldedPayment {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("CertifiedShieldedPayment { <redacted> }")
    }
}

/// An opened agreement or its matching finalized payment could not be verified.
#[derive(Debug, thiserror::Error)]
pub enum ShieldedDisclosureError {
    /// The transcript, commitment opening, or participant authorizations are invalid.
    #[error(transparent)]
    Agreement(#[from] DisclosureError),
    /// The agreement is not the supported fixed-shape suite-2 shielded payment.
    #[error(transparent)]
    Shape(#[from] ShieldedMapError),
    /// The signed revision cannot be derived from the agreement opening.
    #[error(transparent)]
    Revision(#[from] RevisionError),
    /// RPC evidence is unavailable, inconsistent, or its durable history scan is pending.
    #[error(transparent)]
    Observation(#[from] ObservationError),
    /// No finalized pool payment matches this selected agreement.
    #[error("the disclosed agreement has no verified final matching shielded payment")]
    Payment,
}

/// Verifies an opened selected agreement against two configured pool RPCs and separate caches.
///
/// No note wallet, spend key, prover, relayer, or participant process is required. Only public
/// pool data is sent to the RPCs. Signed agreement evidence remains valid after deal expiry.
/// A pending history scan must be retried with the same caches; it is not unpaid evidence.
/// Grant issuer authentication is separate and must precede a full disclosure claim.
pub async fn verify_shielded_payment(
    evidence: SelectedAgreement,
    rpc: &PoolRpc,
    index: &IndexStore,
    peer_rpc: &PoolRpc,
    peer_index: &IndexStore,
) -> Result<FinalizedShieldedPayment, ShieldedDisclosureError> {
    let claim = PaymentClaim::from_evidence(&evidence)?;
    let reads = observe_shielded_deal_agreed(
        rpc,
        index,
        peer_rpc,
        peer_index,
        &claim.context,
        &claim.agreement.nullifier,
        std::slice::from_ref(&claim.revision),
    )
    .await?;
    claim.verify_payment(&reads)?;
    let DealEvidence::Observed(settlement) = reads else {
        return Err(ShieldedDisclosureError::Payment);
    };
    Ok(FinalizedShieldedPayment {
        evidence,
        agreement: claim.agreement,
        settlement,
    })
}

/// [`verify_shielded_payment`] that also returns a [`PaymentCertificate`].
///
/// The payment checks are the same. The pairing is stricter: both providers must also agree on
/// the settlement block, transaction hash, log index, and pool identity. Both providers must
/// also run the pinned pool and dependency code at the finalized anchor: a permissive verifier
/// behind a matching ABI would otherwise accept proofs the reviewed deployment would reject.
/// Scans at most 1000 new blocks per provider; a longer history is reported as pending and
/// resumes from the same caches on retry.
pub async fn verify_shielded_payment_certified(
    evidence: SelectedAgreement,
    rpc: &PoolRpc,
    index: &IndexStore,
    peer_rpc: &PoolRpc,
    peer_index: &IndexStore,
    pins: PoolRuntimePins,
) -> Result<CertifiedShieldedPayment, ShieldedDisclosureError> {
    let claim = PaymentClaim::from_evidence(&evidence)?;
    let observed = observe_shielded_deal_certified_bounded(
        rpc,
        index,
        peer_rpc,
        peer_index,
        &claim.context,
        &claim.agreement.nullifier,
        std::slice::from_ref(&claim.revision),
        CERTIFIED_SCAN_BLOCKS,
    )
    .await?;
    claim.verify_payment(&observed.evidence)?;
    let DealEvidence::Observed(settlement) = observed.evidence else {
        return Err(ShieldedDisclosureError::Payment);
    };
    let Some(location) = observed.settlement else {
        return Err(ShieldedDisclosureError::Payment);
    };
    // The certificate claims a runtime pin, so read the code back from both providers before
    // recording it. A zero pin or a mismatch is an error, never a silent skip.
    rpc.authenticate_runtime_at(observed.finalized.hash, pins)
        .await
        .map_err(ObservationError::from)?;
    peer_rpc
        .authenticate_runtime_at(observed.finalized.hash, pins)
        .await
        .map_err(ObservationError::from)?;
    let certificate = PaymentCertificate {
        chain_id: rpc.chain_id(),
        pool: rpc.pool(),
        domain_digest: suite::keccak256(&[&evidence
            .terms
            .domain
            .encode()
            .map_err(|_| ShieldedDisclosureError::Payment)?]),
        verifier_version: observed.pool.verifier_version,
        asset: observed.pool.asset,
        commitment: *claim.agreement.commitment.as_bytes(),
        nullifier: *claim.agreement.nullifier.as_bytes(),
        amount: claim.revision.amount().get(),
        finalized_number: observed.finalized.number,
        finalized_hash: observed.finalized.hash,
        finalized_timestamp: observed.finalized.timestamp,
        settlement: location,
        observers: [rpc.endpoint_fingerprint(), peer_rpc.endpoint_fingerprint()],
        runtime: pins,
    };
    let payment = FinalizedShieldedPayment {
        evidence,
        agreement: claim.agreement,
        settlement,
    };
    if !certificate.binds(&payment.evidence, &payment.agreement) {
        return Err(ShieldedDisclosureError::Payment);
    }
    Ok(CertifiedShieldedPayment {
        payment,
        certificate,
    })
}

/// Whether both configured providers show this agreement's deal consumed at a checkpoint block.
///
/// Only meaningful after a checkpointed index reported [`ObservationError::HistoryIncomplete`]
/// (the deal is consumed, but no settlement event exists after the checkpoint). The pool sets
/// `consumedDeals` once, so `true` means the deal was settled at or before the checkpoint block,
/// which that index can never see; it is not a payment claim. `false` means the settlement should
/// be after the checkpoint, where the index found no event: the caller must treat that as
/// provider inconsistency, never as nonpayment.
///
/// Both providers must report the checkpoint block canonical and finalized and return the same
/// value, read at exactly the checkpoint block hash; anything else is
/// [`ObservationError::Inconsistent`]. A failed read is an error, never `false`.
pub async fn deal_consumed_at_checkpoint(
    agreement: &VerifiedAgreement,
    rpc: &PoolRpc,
    peer_rpc: &PoolRpc,
    checkpoint: &TreeCheckpoint,
) -> Result<bool, ObservationError> {
    if rpc.shares_endpoint(peer_rpc) {
        return Err(ObservationError::Context);
    }
    let mut agreed = None;
    for source in [rpc, peer_rpc] {
        if source.canonical_block_hash(checkpoint.block_number).await? != checkpoint.block_hash
            || source.finalized_head().await?.number < checkpoint.block_number
        {
            return Err(ObservationError::Inconsistent);
        }
        let consumed = source
            .consumed_deal_at(*agreement.nullifier.as_bytes(), checkpoint.block_hash)
            .await?;
        if agreed.is_some_and(|previous| previous != consumed) {
            return Err(ObservationError::Inconsistent);
        }
        agreed = Some(consumed);
    }
    agreed.ok_or(ObservationError::Inconsistent)
}

fn erc20_address(asset: &AssetId) -> Option<[u8; 20]> {
    if asset.asset_namespace() != "erc20" {
        return None;
    }
    let token = asset.asset_reference().strip_prefix("0x")?;
    if token.len() != 40
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    hex::decode(token).ok()?.try_into().ok()
}

struct PaymentClaim {
    agreement: VerifiedAgreement,
    context: SettlementContext,
    revision: SignedRevision,
}

impl PaymentClaim {
    fn from_evidence(evidence: &SelectedAgreement) -> Result<Self, ShieldedDisclosureError> {
        ShieldedDeal::from_terms(&evidence.terms)?;
        let agreement = verify_selected_agreement(evidence)?;
        let revision = SignedRevision::from_opening(&evidence.terms, &evidence.blinding)?;
        let context = SettlementContext {
            // The auditor observes settlement; it does not generate a spend proof.
            require_local_proving: false,
            mode: evidence.terms.settlement_mode,
            domain: evidence.terms.domain.clone(),
            suite_id: evidence.terms.suite_id,
            asset: evidence.terms.asset.clone(),
            required_guarantees: evidence.terms.required_guarantees,
        };
        Ok(Self {
            agreement,
            context,
            revision,
        })
    }

    fn verify_payment(&self, evidence: &DealEvidence) -> Result<(), ShieldedDisclosureError> {
        let DealEvidence::Observed(reads) = evidence else {
            return Err(ShieldedDisclosureError::Payment);
        };
        let Some(winner) = &reads.winner else {
            return Err(ShieldedDisclosureError::Payment);
        };
        if reads.deal_nullifier != self.agreement.nullifier
            || !reads.consumed_at_final
            || !reads.consumed_at_head
            || !winner.is_final
            || winner.commitment != self.agreement.commitment
            || winner.amount != self.revision.amount()
            || winner.fee != self.revision.fee()
        {
            return Err(ShieldedDisclosureError::Payment);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use erebus_core::{
        auth::{authorization_digest, Authorization, Role},
        commitment::{commit_agreement, CommitmentBlinding, DealCommitment, DealNullifier},
        deal_state::{DealReads, WinningSettlement},
        domain::DeploymentDomain,
        ids::{AddressBytes, AssetId, BaseUnits, ChainNamespace, KeyBytes, SignatureBytes},
        service::ServiceRecord,
        shielded::{note_spend_tag, SHIELDED_GUARANTEES},
        shielded_auth::{derive_key, sign_message},
        terms::{AgreementTerms, FeePolicy, GuaranteeSet, SettlementMode},
    };
    use erebus_transport::{
        hashing::TRANSCRIPT_HASH_VERSION,
        message::{Message, MessageType},
        transcript::Transcript,
    };

    fn signed(mut evidence: SelectedAgreement) -> SelectedAgreement {
        let commitment = commit_agreement(&evidence.terms, &evidence.blinding).unwrap();
        let authorize = |role, seed| {
            let digest =
                authorization_digest(&evidence.terms.domain, role, &commitment, 2).unwrap();
            let (_, signature) = sign_message(&seed, &digest).unwrap();
            Authorization {
                role,
                suite_id: 2,
                commitment,
                signature: SignatureBytes::new(signature.to_vec()).unwrap(),
            }
        };
        evidence.buyer = authorize(Role::Buyer, [1; 32]);
        evidence.seller = authorize(Role::Seller, [2; 32]);
        evidence
    }

    fn fixture() -> SelectedAgreement {
        let deal_id = [7; 16];
        let message = Message::new(
            [9; 32],
            deal_id,
            1,
            Role::Buyer,
            1,
            [0; 32],
            MessageType::Offer,
            b"dataset access for 70".to_vec(),
        )
        .unwrap();
        let mut transcript = Transcript::new(deal_id, TRANSCRIPT_HASH_VERSION).unwrap();
        transcript.append(&message).unwrap();
        let mut secret = [0; 32];
        secret[31] = 3;
        let namespace = ChainNamespace::parse("eip155:31337").unwrap();
        let terms = AgreementTerms {
            protocol_version: 1,
            suite_id: 2,
            domain: DeploymentDomain {
                namespace,
                settlement_contract: Some(AddressBytes::new(vec![3; 20]).unwrap()),
                pool: Some(AddressBytes::new(vec![3; 20]).unwrap()),
                verifier_version: 2,
            },
            deal_id,
            revision: 1,
            transcript_root: transcript.root().unwrap(),
            buyer_authorization_key: KeyBytes::new(derive_key(&[1; 32]).unwrap().to_vec()).unwrap(),
            seller_authorization_key: KeyBytes::new(derive_key(&[2; 32]).unwrap().to_vec())
                .unwrap(),
            payment_recipient: KeyBytes::new(note_spend_tag(&secret).unwrap().to_vec()).unwrap(),
            asset: AssetId::parse("eip155:31337/erc20:0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                .unwrap(),
            amount: BaseUnits::new(70),
            expiry: 200,
            fee_policy: FeePolicy::none(),
            settlement_mode: SettlementMode::Shielded,
            required_guarantees: GuaranteeSet::from_bits(SHIELDED_GUARANTEES).unwrap(),
            settlement_nonce: [6; 32],
            service: ServiceRecord {
                resource: "dataset.access".into(),
                quantity: BaseUnits::new(1),
                unit: "request".into(),
                access_recipient: KeyBytes::new(vec![8; 32]).unwrap(),
                delivery_deadline: 200,
                fulfillment_method: "http-access".into(),
                fulfillment_digest: [0; 32],
            },
        };
        let placeholder = Authorization {
            role: Role::Buyer,
            suite_id: 2,
            commitment: DealCommitment::from_bytes([0; 32]),
            signature: SignatureBytes::new(vec![0; 96]).unwrap(),
        };
        let mut blinding = [0; 32];
        blinding[31] = 5;
        signed(SelectedAgreement {
            terms,
            blinding: CommitmentBlinding::from_bytes(blinding),
            buyer: placeholder.clone(),
            seller: placeholder,
            transcript_hash_version: TRANSCRIPT_HASH_VERSION,
            messages: vec![message],
        })
    }

    // Synthetic reads exercise only the matching predicate. They are not RPC or payment proof.
    fn reads(claim: &PaymentClaim) -> DealReads {
        DealReads {
            deal_nullifier: claim.agreement.nullifier,
            final_anchor_timestamp: 300,
            consumed_at_final: true,
            consumed_at_head: true,
            winner: Some(WinningSettlement {
                commitment: claim.agreement.commitment,
                amount: claim.revision.amount(),
                fee: claim.revision.fee(),
                is_final: true,
            }),
        }
    }

    #[test]
    fn suite_two_disclosure_verifies_transcript_authorizations_and_opening() {
        let evidence = fixture();
        let restored = SelectedAgreement::decode(&evidence.encode().unwrap()).unwrap();
        let claim = PaymentClaim::from_evidence(&restored).unwrap();
        assert_eq!(claim.agreement.commitment, evidence.buyer.commitment);
        assert_eq!(
            claim.agreement.transcript_root,
            evidence.terms.transcript_root
        );
        assert_eq!(claim.revision.amount().get(), 70);
        assert!(!claim.context.require_local_proving);
        // The payment remains auditable after settlement authorization has expired.
        claim
            .verify_payment(&DealEvidence::Observed(reads(&claim)))
            .unwrap();
    }

    #[test]
    fn changed_signed_terms_fail_before_observation() {
        let original = fixture();
        let mut cases = vec![original.clone(); 6];
        cases[0].terms.amount = BaseUnits::new(71);
        cases[1].terms.expiry += 1;
        cases[2].terms.service.resource = "another.dataset".into();
        cases[3].terms.payment_recipient = KeyBytes::new(vec![0; 32]).unwrap();
        cases[4].terms.domain.verifier_version += 1;
        cases[5].blinding = CommitmentBlinding::from_bytes([0; 32]);
        for case in cases {
            assert!(PaymentClaim::from_evidence(&case).is_err());
        }
    }

    #[test]
    fn incomplete_cross_deal_and_forged_consent_are_rejected() {
        let original = fixture();
        let mut missing = original.clone();
        missing.messages.clear();
        assert!(PaymentClaim::from_evidence(&missing).is_err());
        let mut other_deal = original.clone();
        other_deal.messages[0].deal_id[0] ^= 1;
        assert!(PaymentClaim::from_evidence(&other_deal).is_err());
        let mut altered = original.clone();
        altered.messages[0].body.push(0);
        assert!(PaymentClaim::from_evidence(&altered).is_err());
        let mut swapped = original.clone();
        std::mem::swap(&mut swapped.buyer, &mut swapped.seller);
        assert!(PaymentClaim::from_evidence(&swapped).is_err());
        let mut forged = original;
        forged.seller.signature = SignatureBytes::new(vec![0; 96]).unwrap();
        assert!(PaymentClaim::from_evidence(&forged).is_err());
    }

    #[test]
    fn absent_unknown_and_nonfinal_payments_are_not_verified() {
        let claim = PaymentClaim::from_evidence(&fixture()).unwrap();
        assert!(claim.verify_payment(&DealEvidence::Unknown).is_err());
        let mut cases = vec![reads(&claim); 4];
        cases[0].winner = None;
        cases[1].consumed_at_final = false;
        cases[2].consumed_at_head = false;
        cases[3].winner.as_mut().unwrap().is_final = false;
        for case in cases {
            assert!(claim.verify_payment(&DealEvidence::Observed(case)).is_err());
        }
    }

    #[test]
    fn foreign_identity_commitment_amount_or_fee_cannot_match() {
        let claim = PaymentClaim::from_evidence(&fixture()).unwrap();
        let mut cases = vec![reads(&claim); 4];
        cases[0].deal_nullifier = DealNullifier::from_bytes([1; 32]);
        cases[1].winner.as_mut().unwrap().commitment = DealCommitment::from_bytes([1; 32]);
        cases[2].winner.as_mut().unwrap().amount = BaseUnits::new(71);
        cases[3].winner.as_mut().unwrap().fee = BaseUnits::new(1);
        for case in cases {
            assert!(claim.verify_payment(&DealEvidence::Observed(case)).is_err());
        }
    }

    #[test]
    fn a_validly_resigned_different_revision_does_not_match_the_paid_commitment() {
        let original = fixture();
        let paid = PaymentClaim::from_evidence(&original).unwrap();
        let mut changed = original;
        changed.terms.revision += 1;
        changed.terms.amount = BaseUnits::new(71);
        let changed = PaymentClaim::from_evidence(&signed(changed)).unwrap();
        assert_eq!(changed.agreement.nullifier, paid.agreement.nullifier);
        assert_ne!(changed.agreement.commitment, paid.agreement.commitment);
        assert!(changed
            .verify_payment(&DealEvidence::Observed(reads(&paid)))
            .is_err());
    }

    #[tokio::test]
    async fn malformed_disclosure_never_reaches_the_rpc() {
        let mut evidence = fixture();
        evidence.terms.settlement_mode = SettlementMode::PublicBound;
        let directory = tempfile::tempdir().unwrap();
        let domain = crate::index_store::IndexDomain {
            chain_id: 31_337,
            pool: [3; 20],
            first_block: 1,
            first_hash: [1; 32],
        };
        let index = IndexStore::new(directory.path().join("first/index.json"), domain).unwrap();
        let peer_index = IndexStore::new(directory.path().join("peer/index.json"), domain).unwrap();
        let rpc = PoolRpc::new("http://127.0.0.1:1", 31_337, [3; 20]).unwrap();
        let peer_rpc = PoolRpc::new("http://127.0.0.1:2", 31_337, [3; 20]).unwrap();
        assert!(matches!(
            verify_shielded_payment(evidence, &rpc, &index, &peer_rpc, &peer_index).await,
            Err(ShieldedDisclosureError::Shape(_))
        ));
        assert!(std::fs::read_dir(directory.path().join("first"))
            .unwrap()
            .next()
            .is_none());
        assert!(std::fs::read_dir(directory.path().join("peer"))
            .unwrap()
            .next()
            .is_none());
    }

    // This RPC fixture tests observation wiring, not a deployed verifier or funded payment.
    #[derive(Clone)]
    struct RpcFixture {
        commitment: [u8; 32],
        nullifier: [u8; 32],
        paid: bool,
        omit_log: bool,
        finalized: u64,
        head: u64,
        asset: [u8; 20],
        tx_hash: [u8; 32],
        code: Vec<u8>,
    }

    fn rpc_fixture(claim: &PaymentClaim) -> RpcFixture {
        RpcFixture {
            commitment: *claim.agreement.commitment.as_bytes(),
            nullifier: *claim.agreement.nullifier.as_bytes(),
            paid: true,
            omit_log: false,
            finalized: 4,
            head: 4,
            asset: [0xaa; 20],
            tx_hash: [9; 32],
            code: vec![0x60, 0x00],
        }
    }

    /// Pins for the fixture code: the pool and its four dependencies all serve it.
    fn pins_for(code: &[u8]) -> PoolRuntimePins {
        use sha3::{Digest, Keccak256};
        let hash: [u8; 32] = Keccak256::digest(code).into();
        PoolRuntimePins {
            pool: hash,
            poseidon: hash,
            deposit: hash,
            transfer: hash,
            withdraw: hash,
        }
    }

    fn block_hash(number: u64) -> [u8; 32] {
        let mut hash = [0x11; 32];
        hash[24..].copy_from_slice(&number.to_be_bytes());
        hash
    }

    fn rpc_selector(name: &str) -> String {
        use sha3::{Digest, Keccak256};
        format!(
            "0x{}",
            hex::encode(&Keccak256::digest(name.as_bytes())[..4])
        )
    }

    async fn rpc_reply(
        axum::extract::State(state): axum::extract::State<RpcFixture>,
        axum::Json(request): axum::Json<serde_json::Value>,
    ) -> axum::Json<serde_json::Value> {
        use serde_json::json;
        use sha3::{Digest, Keccak256};
        let result = match request["method"].as_str().unwrap() {
            "eth_chainId" => json!("0x7a69"),
            "eth_blockNumber" => json!(format!("0x{:x}", state.head)),
            "eth_getBlockByNumber" => {
                let tag = request["params"][0].as_str().unwrap();
                let number = if tag == "finalized" {
                    state.finalized
                } else {
                    u64::from_str_radix(tag.strip_prefix("0x").unwrap(), 16).unwrap()
                };
                json!({
                    "number": format!("0x{number:x}"),
                    "hash": format!("0x{}", hex::encode(block_hash(number))),
                    "parentHash": format!("0x{}", hex::encode(block_hash(number - 1))),
                    "timestamp": format!("0x{:x}", if number == 4 { 300 } else { 100 + number }),
                })
            }
            "eth_getLogs" => {
                let bound = |key: &str| {
                    let value = request["params"][0][key].as_str().unwrap();
                    u64::from_str_radix(value.strip_prefix("0x").unwrap(), 16).unwrap()
                };
                // Ranged queries are allowed; the payment log lives in block 3.
                if (bound("fromBlock")..=bound("toBlock")).contains(&3)
                    && state.paid
                    && !state.omit_log
                {
                    json!([{
                        "address": format!("0x{}", hex::encode([3; 20])),
                        "blockNumber": "0x3",
                        "blockHash": format!("0x{}", hex::encode(block_hash(3))),
                        "transactionHash": format!("0x{}", hex::encode(state.tx_hash)),
                        "logIndex": "0x0",
                        "topics": [
                            format!("0x{}", hex::encode(Keccak256::digest(b"DealTransferred(uint256,uint256,uint256)"))),
                            format!("0x{}", hex::encode(state.commitment)),
                            format!("0x{}", hex::encode(state.nullifier)),
                            format!("0x{}", hex::encode([7; 32])),
                        ],
                        "data": "0x",
                        "removed": false,
                    }])
                } else {
                    json!([])
                }
            }
            "eth_getCode" => {
                assert_eq!(request["params"][1]["requireCanonical"], true);
                json!(format!("0x{}", hex::encode(&state.code)))
            }
            "eth_call" => {
                let data = request["params"][0]["data"].as_str().unwrap();
                if data == rpc_selector("currentRoot()") {
                    json!(format!(
                        "0x{}",
                        hex::encode(crate::indexer::PoolIndex::new(2).unwrap().root())
                    ))
                } else if data == rpc_selector("nextLeafIndex()") {
                    json!(format!("0x{:064x}", 0))
                } else {
                    assert_eq!(request["params"][1]["requireCanonical"], true);
                    if data == rpc_selector("asset()") {
                        json!(format!("0x{:0>64}", hex::encode(state.asset)))
                    } else if data == rpc_selector("verifierVersion()") {
                        json!(format!("0x{:064x}", 2))
                    } else if [
                        rpc_selector("poseidonTwo()"),
                        rpc_selector("depositVerifier()"),
                        rpc_selector("transferVerifier()"),
                        rpc_selector("withdrawVerifier()"),
                    ]
                    .iter()
                    .any(|selector| data == selector)
                    {
                        json!(format!("0x{:0>64}", hex::encode([0x99; 20])))
                    } else {
                        assert!(data.starts_with(&rpc_selector("consumedDeals(uint256)")));
                        let hash = hex::decode(
                            request["params"][1]["blockHash"]
                                .as_str()
                                .unwrap()
                                .strip_prefix("0x")
                                .unwrap(),
                        )
                        .unwrap();
                        let number = u64::from_be_bytes(hash[24..].try_into().unwrap());
                        json!(format!("0x{:064x}", u8::from(state.paid && number >= 3)))
                    }
                }
            }
            method => panic!("unexpected disclosure RPC method: {method}"),
        };
        axum::Json(json!({"jsonrpc": "2.0", "id": request["id"], "result": result}))
    }

    async fn start_rpc(state: RpcFixture) -> (PoolRpc, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let rpc = PoolRpc::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            31_337,
            [3; 20],
        )
        .unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new()
                    .route("/", axum::routing::post(rpc_reply))
                    .with_state(state),
            )
            .await
            .unwrap();
        });
        (rpc, server)
    }

    fn auditor_index(directory: &std::path::Path, name: &str) -> IndexStore {
        IndexStore::new(
            directory.join(name).join("index.json"),
            crate::index_store::IndexDomain {
                chain_id: 31_337,
                pool: [3; 20],
                first_block: 2,
                first_hash: block_hash(2),
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn rpc_verification_uses_two_fresh_caches_and_needs_no_wallet_or_prover() {
        let evidence = fixture();
        let claim = PaymentClaim::from_evidence(&evidence).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let first_index = auditor_index(directory.path(), "first");
        let peer_index = auditor_index(directory.path(), "peer");
        let (rpc, server) = start_rpc(rpc_fixture(&claim)).await;
        let (peer_rpc, peer_server) = start_rpc(rpc_fixture(&claim)).await;
        let restored = SelectedAgreement::decode(&evidence.encode().unwrap()).unwrap();
        let payment = verify_shielded_payment(restored, &rpc, &first_index, &peer_rpc, &peer_index)
            .await
            .unwrap();
        assert_eq!(payment.agreement, claim.agreement);
        assert!(payment.settlement.consumed_at_final);
        assert_eq!(
            payment.settlement.winner.as_ref().unwrap().commitment,
            payment.agreement.commitment
        );
        assert_eq!(payment.evidence.terms.amount.get(), 70);
        assert_eq!(
            format!("{payment:?}"),
            "FinalizedShieldedPayment { <redacted> }"
        );
        assert_eq!(first_index.load().unwrap().tip().unwrap().number, 4);
        assert_eq!(peer_index.load().unwrap().tip().unwrap().number, 4);
        server.abort();
        peer_server.abort();
    }

    #[tokio::test]
    async fn rpc_verification_rejects_unpaid_nonfinal_inconsistent_and_foreign_deployments() {
        let evidence = fixture();
        let claim = PaymentClaim::from_evidence(&evidence).unwrap();
        let original = rpc_fixture(&claim);
        let mut cases = vec![(original.clone(), original.clone()); 5];
        cases[0].0.paid = false;
        cases[0].1.paid = false;
        cases[1].0.finalized = 2;
        cases[1].1.finalized = 2;
        cases[2].1.commitment[31] ^= 1;
        cases[3].1.omit_log = true;
        cases[4].1.asset = [0xbb; 20];
        for (first, second) in cases {
            let directory = tempfile::tempdir().unwrap();
            let first_index = auditor_index(directory.path(), "first");
            let peer_index = auditor_index(directory.path(), "peer");
            let (rpc, server) = start_rpc(first).await;
            let (peer_rpc, peer_server) = start_rpc(second).await;
            assert!(verify_shielded_payment(
                evidence.clone(),
                &rpc,
                &first_index,
                &peer_rpc,
                &peer_index
            )
            .await
            .is_err());
            server.abort();
            peer_server.abort();
        }
    }

    #[tokio::test]
    async fn cached_finalized_payment_does_not_chase_later_activity() {
        let evidence = fixture();
        let claim = PaymentClaim::from_evidence(&evidence).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let first_index = auditor_index(directory.path(), "first");
        let peer_index = auditor_index(directory.path(), "peer");
        let (rpc, server) = start_rpc(rpc_fixture(&claim)).await;
        let (peer_rpc, peer_server) = start_rpc(rpc_fixture(&claim)).await;
        verify_shielded_payment_certified(
            evidence.clone(),
            &rpc,
            &first_index,
            &peer_rpc,
            &peer_index,
            pins_for(&[0x60, 0x00]),
        )
        .await
        .unwrap();
        server.abort();
        peer_server.abort();

        let mut advanced = rpc_fixture(&claim);
        advanced.head = 2_000;
        advanced.finalized = 2_000;
        let (rpc, server) = start_rpc(advanced.clone()).await;
        let (peer_rpc, peer_server) = start_rpc(advanced.clone()).await;
        let certified = verify_shielded_payment_certified(
            evidence.clone(),
            &rpc,
            &first_index,
            &peer_rpc,
            &peer_index,
            pins_for(&[0x60, 0x00]),
        )
        .await
        .unwrap();
        assert_eq!(certified.certificate.finalized_number, 4);
        assert_eq!(first_index.load().unwrap().tip().unwrap().number, 4);
        assert_eq!(peer_index.load().unwrap().tip().unwrap().number, 4);
        server.abort();
        peer_server.abort();

        let empty = tempfile::tempdir().unwrap();
        let empty_first = auditor_index(empty.path(), "first");
        let empty_peer = auditor_index(empty.path(), "peer");
        let (rpc, server) = start_rpc(advanced.clone()).await;
        let (peer_rpc, peer_server) = start_rpc(advanced.clone()).await;
        assert!(matches!(
            verify_shielded_payment_certified(
                evidence.clone(),
                &rpc,
                &empty_first,
                &peer_rpc,
                &empty_peer,
                pins_for(&[0x60, 0x00]),
            )
            .await,
            Err(ShieldedDisclosureError::Observation(
                ObservationError::Recovery(crate::recovery::RecoveryError::HistoryPending { .. })
            ))
        ));
        server.abort();
        peer_server.abort();

        for fault in 0..3 {
            let mut broken = advanced.clone();
            match fault {
                0 => broken.paid = false,
                1 => broken.omit_log = true,
                _ => broken.code = vec![0x60, 0x01],
            }
            let (rpc, server) = start_rpc(advanced.clone()).await;
            let (peer_rpc, peer_server) = start_rpc(broken).await;
            assert!(verify_shielded_payment_certified(
                evidence.clone(),
                &rpc,
                &first_index,
                &peer_rpc,
                &peer_index,
                pins_for(&[0x60, 0x00]),
            )
            .await
            .is_err());
            server.abort();
            peer_server.abort();
        }
    }

    #[tokio::test]
    async fn certified_verification_returns_the_agreed_anchors_and_settlement_location() {
        let evidence = fixture();
        let claim = PaymentClaim::from_evidence(&evidence).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let first_index = auditor_index(directory.path(), "first");
        let peer_index = auditor_index(directory.path(), "peer");
        let (rpc, server) = start_rpc(rpc_fixture(&claim)).await;
        let (peer_rpc, peer_server) = start_rpc(rpc_fixture(&claim)).await;
        let certified = verify_shielded_payment_certified(
            evidence.clone(),
            &rpc,
            &first_index,
            &peer_rpc,
            &peer_index,
            pins_for(&[0x60, 0x00]),
        )
        .await
        .unwrap();
        let certificate = &certified.certificate;
        assert_eq!(certificate.runtime, pins_for(&[0x60, 0x00]));
        assert_eq!(certificate.chain_id, 31_337);
        assert_eq!(certificate.pool, [3; 20]);
        assert_eq!(certificate.asset, [0xaa; 20]);
        assert_eq!(certificate.verifier_version, 2);
        assert_eq!(certificate.amount, 70);
        assert_eq!(
            certificate.commitment,
            *claim.agreement.commitment.as_bytes()
        );
        assert_eq!(certificate.nullifier, *claim.agreement.nullifier.as_bytes());
        assert_eq!(certificate.finalized_number, 4);
        assert_eq!(certificate.finalized_hash, block_hash(4));
        assert_eq!(certificate.finalized_timestamp, 300);
        assert_eq!(
            certificate.settlement,
            SettlementLocation {
                block_number: 3,
                block_hash: block_hash(3),
                transaction_hash: [9; 32],
                log_index: 0,
            }
        );
        assert_eq!(
            certificate.observers,
            [rpc.endpoint_fingerprint(), peer_rpc.endpoint_fingerprint()]
        );
        assert_ne!(certificate.observers[0], certificate.observers[1]);
        assert!(certificate.binds(&evidence, &claim.agreement));
        assert_eq!(certified.payment.agreement, claim.agreement);
        assert_eq!(
            format!("{certified:?}"),
            "CertifiedShieldedPayment { <redacted> }"
        );
        let encoded = serde_json::to_vec(certificate).unwrap();
        let decoded: PaymentCertificate = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(&decoded, certificate);
        server.abort();
        peer_server.abort();
    }

    #[tokio::test]
    async fn certified_pairing_is_stricter_and_the_legacy_result_is_unchanged() {
        let evidence = fixture();
        let claim = PaymentClaim::from_evidence(&evidence).unwrap();
        let mut peer = rpc_fixture(&claim);
        peer.tx_hash = [5; 32];
        let directory = tempfile::tempdir().unwrap();
        let (rpc, server) = start_rpc(rpc_fixture(&claim)).await;
        let (peer_rpc, peer_server) = start_rpc(peer).await;
        // The legacy predicate compares anchors and deal reads only, as it always has.
        verify_shielded_payment(
            evidence.clone(),
            &rpc,
            &auditor_index(directory.path(), "legacy-first"),
            &peer_rpc,
            &auditor_index(directory.path(), "legacy-peer"),
        )
        .await
        .unwrap();
        // A certificate needs the providers to agree on where the settlement is too.
        assert!(matches!(
            verify_shielded_payment_certified(
                evidence,
                &rpc,
                &auditor_index(directory.path(), "first"),
                &peer_rpc,
                &auditor_index(directory.path(), "peer"),
                pins_for(&[0x60, 0x00]),
            )
            .await,
            Err(ShieldedDisclosureError::Observation(
                ObservationError::Inconsistent
            ))
        ));
        server.abort();
        peer_server.abort();
    }

    #[tokio::test]
    async fn certified_verification_rejects_unpaid_and_nonfinal_payments() {
        let evidence = fixture();
        let claim = PaymentClaim::from_evidence(&evidence).unwrap();
        let original = rpc_fixture(&claim);
        let mut unpaid = original.clone();
        unpaid.paid = false;
        let mut omitted_settlement = original.clone();
        omitted_settlement.omit_log = true;
        let mut nonfinal = original;
        nonfinal.finalized = 2;
        // A consumed deal without its settlement log cannot certify entitlement.
        for state in [unpaid, nonfinal, omitted_settlement] {
            let directory = tempfile::tempdir().unwrap();
            let (rpc, server) = start_rpc(state.clone()).await;
            let (peer_rpc, peer_server) = start_rpc(state).await;
            assert!(verify_shielded_payment_certified(
                evidence.clone(),
                &rpc,
                &auditor_index(directory.path(), "first"),
                &peer_rpc,
                &auditor_index(directory.path(), "peer"),
                pins_for(&[0x60, 0x00]),
            )
            .await
            .is_err());
            server.abort();
            peer_server.abort();
        }
    }

    #[tokio::test]
    async fn certified_verification_rejects_a_runtime_that_differs_from_the_pin() {
        let evidence = fixture();
        let claim = PaymentClaim::from_evidence(&evidence).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let (rpc, server) = start_rpc(rpc_fixture(&claim)).await;
        let (peer_rpc, peer_server) = start_rpc(rpc_fixture(&claim)).await;
        // The providers serve [0x60, 0x00]; a pin for other code must fail closed, and a zero
        // pin is a configuration error rather than a skipped check.
        for wrong in [
            pins_for(&[0x60, 0x01]),
            PoolRuntimePins {
                pool: [0; 32],
                ..pins_for(&[0x60, 0x00])
            },
        ] {
            assert!(matches!(
                verify_shielded_payment_certified(
                    evidence.clone(),
                    &rpc,
                    &auditor_index(directory.path(), "first"),
                    &peer_rpc,
                    &auditor_index(directory.path(), "peer"),
                    wrong,
                )
                .await,
                Err(ShieldedDisclosureError::Observation(ObservationError::Rpc(
                    _
                )))
            ));
        }
        server.abort();
        peer_server.abort();
    }

    #[test]
    fn a_certificate_binds_only_its_own_agreement() {
        let evidence = fixture();
        let claim = PaymentClaim::from_evidence(&evidence).unwrap();
        let certificate = PaymentCertificate {
            chain_id: 31_337,
            pool: [3; 20],
            domain_digest: suite::keccak256(&[&evidence.terms.domain.encode().unwrap()]),
            verifier_version: 2,
            asset: [0xaa; 20],
            commitment: *claim.agreement.commitment.as_bytes(),
            nullifier: *claim.agreement.nullifier.as_bytes(),
            amount: 70,
            finalized_number: 4,
            finalized_hash: block_hash(4),
            finalized_timestamp: 300,
            settlement: SettlementLocation {
                block_number: 3,
                block_hash: block_hash(3),
                transaction_hash: [9; 32],
                log_index: 0,
            },
            observers: [[1; 32], [2; 32]],
            runtime: pins_for(&[0x60, 0x00]),
        };
        assert!(certificate.binds(&evidence, &claim.agreement));
        assert_eq!(
            format!("{certificate:?}"),
            "PaymentCertificate { <redacted> }"
        );
        let mut cases = vec![certificate.clone(); 10];
        cases[0].chain_id += 1;
        cases[1].pool[0] ^= 1;
        cases[2].domain_digest[0] ^= 1;
        cases[3].verifier_version += 1;
        cases[4].asset[0] ^= 1;
        cases[5].commitment[0] ^= 1;
        cases[6].nullifier[0] ^= 1;
        cases[7].amount += 1;
        cases[8].settlement.block_number = 5;
        cases[9].observers = [[1; 32], [1; 32]];
        for case in cases {
            assert!(!case.binds(&evidence, &claim.agreement));
        }
    }
}
