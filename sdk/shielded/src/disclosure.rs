//! Independent payment checks for an opened suite-2 disclosure.
//!
//! The caller must authenticate the grant issuer separately. This verifies the selected
//! agreement and its finalized pool payment, not an issuer-to-participant relationship.
//! Pool code and verifying keys must belong to a trusted deployment. RPC agreement is not
//! a contract audit or a proof of provider independence.

use erebus_core::{
    deal_state::{DealEvidence, DealReads, RevisionError, SignedRevision},
    settlement::SettlementContext,
    shielded::{ShieldedDeal, ShieldedMapError},
};
use erebus_transport::disclosure::{
    verify_selected_agreement, DisclosureError, SelectedAgreement, VerifiedAgreement,
};

use crate::{
    index_store::IndexStore,
    observation::{observe_shielded_deal_agreed, ObservationError},
    rpc::PoolRpc,
};

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
        asset: [u8; 20],
    }

    fn rpc_fixture(claim: &PaymentClaim) -> RpcFixture {
        RpcFixture {
            commitment: *claim.agreement.commitment.as_bytes(),
            nullifier: *claim.agreement.nullifier.as_bytes(),
            paid: true,
            omit_log: false,
            finalized: 4,
            asset: [0xaa; 20],
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
            "eth_blockNumber" => json!("0x4"),
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
                if request["params"][0]["fromBlock"] == "0x3" && state.paid && !state.omit_log {
                    json!([{
                        "address": format!("0x{}", hex::encode([3; 20])),
                        "blockNumber": "0x3",
                        "blockHash": format!("0x{}", hex::encode(block_hash(3))),
                        "transactionHash": format!("0x{}", hex::encode([9; 32])),
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
}
