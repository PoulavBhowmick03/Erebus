//! No RPC or local node: validate restored transactions against pinned M1 evidence.
use erebus_core::commitment::{commit_agreement, deal_nullifier, CommitmentBlinding};
use erebus_core::settlement::PreparedSettlement;
use erebus_core::terms::AgreementTerms;
use erebus_evm::chain::{
    build_signed, Eip1559Fees, NonceError, SignedTransaction, SignerJournal, TransactionKey,
    TransactionParams,
};
use erebus_evm::deployment::EvmDeployment;
use erebus_evm::evidence::SettlementEvidence;

fn fixture() -> (EvmDeployment, PreparedSettlement) {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../core/tests/fixtures/agreement-v1-vectors.json"
    ))
    .unwrap();
    let vector = &vectors["vectors"][0];
    let expected = &vector["expected"];
    let decode = |value: &serde_json::Value| hex::decode(value.as_str().unwrap()).unwrap();
    let evidence = SettlementEvidence {
        terms: decode(&expected["canonicalHex"]),
        blinding: decode(&vector["blindingHex"]).try_into().unwrap(),
        buyer_signature: decode(&expected["buyerSignatureHex"]).try_into().unwrap(),
        seller_signature: decode(&expected["sellerSignatureHex"]).try_into().unwrap(),
    };
    let terms = AgreementTerms::decode(&evidence.terms).unwrap();
    let deployment = EvmDeployment::new(
        terms.domain.namespace.clone(),
        [0x11; 20],
        1,
        "http://127.0.0.1:1",
    )
    .unwrap();
    let prepared = PreparedSettlement {
        operation_ref: [1; 32],
        deal_commitment: commit_agreement(
            &terms,
            &CommitmentBlinding::from_bytes(evidence.blinding),
        )
        .unwrap(),
        deal_nullifier: deal_nullifier(&terms).unwrap(),
        domain: terms.domain,
        mode: terms.settlement_mode,
        required_guarantees: terms.required_guarantees,
        backend_evidence: evidence.encode(),
    };
    (deployment, prepared)
}

fn params() -> TransactionParams {
    TransactionParams {
        nonce: 7,
        fees: Eip1559Fees::new(3_000_000_000, 1_000_000_000).unwrap(),
        gas_limit: 300_000,
    }
}

#[test]
fn restored_bytes_must_match_the_trusted_intent() {
    let (deployment, prepared) = fixture();
    let key = TransactionKey::from_bytes(&[1; 32]).unwrap();
    let signed = build_signed(&deployment, &prepared, &key, params()).unwrap();
    let restored = SignedTransaction::from_raw(signed.raw()).unwrap();
    restored
        .validate_against(&deployment, &prepared, key.address(), params())
        .unwrap();
    assert!(restored
        .validate_against(&deployment, &prepared, [0; 20], params())
        .is_err());
    for changed in [
        TransactionParams {
            nonce: 8,
            ..params()
        },
        TransactionParams {
            gas_limit: 400_000,
            ..params()
        },
        TransactionParams {
            fees: params().fees.replacement_floor().unwrap(),
            ..params()
        },
    ] {
        assert!(restored
            .validate_against(&deployment, &prepared, key.address(), changed)
            .is_err());
    }
    let mut other = deployment.clone();
    other.chain_id += 1;
    assert!(build_signed(&other, &prepared, &key, params()).is_err());
    other = deployment.clone();
    other.settlement_contract = [2; 20];
    assert!(restored
        .validate_against(&other, &prepared, key.address(), params())
        .is_err());
}

#[test]
fn restored_evidence_requires_both_valid_authorizations() {
    let (deployment, prepared) = fixture();
    let key = TransactionKey::from_bytes(&[1; 32]).unwrap();
    for buyer in [true, false] {
        let mut bad = prepared.clone();
        let mut evidence = SettlementEvidence::decode(&bad.backend_evidence).unwrap();
        if buyer {
            evidence.buyer_signature = [0; 65];
        } else {
            evidence.seller_signature = [0; 65];
        }
        bad.backend_evidence = evidence.encode();
        assert!(build_signed(&deployment, &bad, &key, params()).is_err());
    }
    let mut bad = prepared;
    bad.backend_evidence.push(0);
    assert!(build_signed(&deployment, &bad, &key, params()).is_err());
}

#[tokio::test]
async fn coordinator_persists_real_signed_bytes_and_revalidates_after_restart() {
    use erebus_coordinator::{Coordinator, Stage};
    use erebus_core::{
        auth::{Authorization, Role},
        ids::{BaseUnits, SignatureBytes},
        policy::SpendingPolicy,
        settlement::{BackendCapabilities, SettlementContext},
    };
    let (deployment, prepared) = fixture();
    let evidence = SettlementEvidence::decode(&prepared.backend_evidence).unwrap();
    let terms = AgreementTerms::decode(&evidence.terms).unwrap();
    let blinding = CommitmentBlinding::from_bytes(evidence.blinding);
    let context = SettlementContext {
        require_local_proving: true,
        mode: terms.settlement_mode,
        domain: terms.domain.clone(),
        suite_id: terms.suite_id,
        asset: terms.asset.clone(),
        required_guarantees: terms.required_guarantees,
    };
    let capabilities = BackendCapabilities {
        modes: [terms.settlement_mode].into_iter().collect(),
        suites: [terms.suite_id].into_iter().collect(),
        guarantees: terms.required_guarantees,
        local_proving: true,
    };
    let policy = SpendingPolicy {
        per_deal_max: BaseUnits::new(2_000_000),
        allowed_assets: [terms.asset.clone()].into_iter().collect(),
        ..SpendingPolicy::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let open = || {
        Coordinator::open(
            dir.path(),
            terms.buyer_authorization_key.clone(),
            context.clone(),
            &context,
            &capabilities,
            policy.clone(),
        )
        .unwrap()
    };
    let coordinator = open();
    coordinator
        .record_intent([1; 32], &terms, &blinding, 10)
        .unwrap();
    let auth = |role, signature: [u8; 65]| Authorization {
        role,
        suite_id: terms.suite_id,
        commitment: prepared.deal_commitment,
        signature: SignatureBytes::new(signature.to_vec()).unwrap(),
    };
    coordinator
        .authorize_buyer([1; 32], 11, |_, _| {
            Ok::<_, ()>(auth(Role::Buyer, evidence.buyer_signature))
        })
        .unwrap();
    coordinator
        .accept_seller([1; 32], &auth(Role::Seller, evidence.seller_signature))
        .unwrap();
    coordinator
        .prepare([1; 32], 12, |_, _, _, _| Ok::<_, ()>(prepared.clone()))
        .unwrap();
    let key = TransactionKey::from_bytes(&[1; 32]).unwrap();
    use erebus_evm::chain::SigningPlan;
    let nonce_dir = tempfile::tempdir().unwrap();
    let journal =
        SignerJournal::open(nonce_dir.path(), deployment.chain_id, key.address()).unwrap();
    let plan = journal
        .reserve(
            &deployment,
            &prepared,
            params().nonce,
            params().fees,
            params().gas_limit,
        )
        .unwrap();
    assert!(plan
        .sign(
            &deployment,
            &prepared,
            &TransactionKey::from_bytes(&[2; 32]).unwrap()
        )
        .is_err());
    let validate = |p: &PreparedSettlement, plan: &[u8], raw: &[u8]| {
        SigningPlan::decode(plan)?.validate(&deployment, p, raw)
    };
    let raw = coordinator
        .sign_transaction(
            [1; 32],
            13,
            &plan.encode(),
            |p, plan| {
                SigningPlan::decode(plan)?
                    .sign(&deployment, p, &key)
                    .map(|tx| tx.raw().to_vec())
            },
            validate,
        )
        .unwrap();
    drop(coordinator);
    let restarted = open();
    let restored_plan = restarted.transaction_plan([1; 32]).unwrap().unwrap();
    assert_eq!(restored_plan, plan.encode());
    assert_eq!(restarted.diagnostics().unwrap()[0].stage, Stage::Signed);
    assert_eq!(
        restarted
            .sign_transaction(
                [1; 32],
                14,
                &restored_plan,
                |_, _| panic!("must reuse the durable transaction"),
                validate
            )
            .unwrap(),
        raw
    );
    let wrong_nonce = TransactionParams {
        nonce: params().nonce + 1,
        ..params()
    };
    assert!(restarted
        .sign_transaction(
            [1; 32],
            14,
            &SigningPlan::new(key.address(), wrong_nonce)
                .unwrap()
                .encode(),
            |_, _| panic!("must not replace"),
            validate
        )
        .is_err());
    let (chain, responses) = mock_chain().await;
    responses.push_success(&alloy::primitives::U64::from(deployment.chain_id));
    // Simulate a lost send response: the journal must retain an unknown attempt.
    let unknown = chain
        .broadcast_journaled(&restarted, [1; 32], 15)
        .await
        .unwrap();
    assert_eq!(
        unknown.outcome,
        erebus_evm::chain::BroadcastOutcome::Unknown
    );
    assert_eq!(
        restarted.broadcast_history([1; 32]).unwrap()[0].outcome,
        erebus_coordinator::BroadcastOutcome::Unknown
    );
    responses.push_success(&alloy::primitives::U64::from(deployment.chain_id));
    responses.push_success(&alloy::primitives::B256::from(unknown.hash));
    let ack = chain
        .broadcast_journaled(&restarted, [1; 32], 16)
        .await
        .unwrap();
    assert_eq!(ack.hash, unknown.hash);
    let history = open().broadcast_history([1; 32]).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(
        history[0].outcome,
        erebus_coordinator::BroadcastOutcome::Unknown
    );
    assert_eq!(
        history[1].outcome,
        erebus_coordinator::BroadcastOutcome::Submitted
    );
    assert_eq!(
        restarted
            .ledger()
            .unwrap()
            .reserved_total(&terms.asset)
            .unwrap(),
        terms.amount
    );
}

#[test]
fn nonce_claim_is_immutable_across_restart_and_sender_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let (deployment, prepared) = fixture();
    let sender = [7; 20];
    let reserve = |j: &SignerJournal, p: &PreparedSettlement, nonce| {
        j.reserve(&deployment, p, nonce, params().fees, params().gas_limit)
    };
    let journal = SignerJournal::open(dir.path(), deployment.chain_id, sender).unwrap();
    assert_eq!(journal.resume(&deployment, &prepared).unwrap(), None);
    let first = reserve(&journal, &prepared, 7).unwrap();
    drop(journal);
    let journal = SignerJournal::open(dir.path(), deployment.chain_id, sender).unwrap();
    assert_eq!(
        journal.resume(&deployment, &prepared).unwrap(),
        Some(first.clone())
    );
    assert_eq!(reserve(&journal, &prepared, 99).unwrap(), first);
    assert!(matches!(
        journal.reserve(
            &deployment,
            &prepared,
            7,
            params().fees.replacement_floor().unwrap(),
            params().gas_limit
        ),
        Err(NonceError::Conflict)
    ));
    let mut second = prepared.clone();
    second.operation_ref = [2; 32];
    assert!(matches!(
        reserve(&journal, &second, 8),
        Err(NonceError::Busy)
    ));
    let independent = SignerJournal::open(dir.path(), deployment.chain_id, [8; 20]).unwrap();
    assert!(reserve(&independent, &second, 8).is_ok());
    let wrong_chain = SignerJournal::open(dir.path(), deployment.chain_id + 1, sender).unwrap();
    assert!(matches!(
        reserve(&wrong_chain, &second, 8),
        Err(NonceError::Conflict)
    ));
}

#[test]
fn generic_pool_call_claim_survives_restart_and_fences_target_and_calldata() {
    let dir = tempfile::tempdir().unwrap();
    let (deployment, prepared) = fixture();
    let sender = [7; 20];
    let target = deployment.settlement_contract;
    let calldata = [0xab, 0xcd, 0xef, 0x12];
    let journal = SignerJournal::open(dir.path(), deployment.chain_id, sender).unwrap();
    let plan = journal
        .reserve_call(
            &deployment,
            &prepared,
            target,
            &calldata,
            7,
            params().fees,
            params().gas_limit,
        )
        .unwrap();
    drop(journal);
    let journal = SignerJournal::open(dir.path(), deployment.chain_id, sender).unwrap();
    assert_eq!(
        journal
            .resume_call(&deployment, &prepared, target, &calldata)
            .unwrap(),
        Some(plan)
    );
    assert!(matches!(
        journal.resume_call(&deployment, &prepared, [0x56; 20], &calldata),
        Err(NonceError::Conflict)
    ));
    assert!(matches!(
        journal.resume_call(&deployment, &prepared, target, &[0xab, 0xcd, 0xef, 0x13]),
        Err(NonceError::Conflict)
    ));
    let mut other = prepared.clone();
    other.operation_ref = [2; 32];
    assert!(matches!(
        journal.reserve(&deployment, &other, 8, params().fees, params().gas_limit),
        Err(NonceError::Busy)
    ));
}

fn other_deployment() -> (EvmDeployment, PreparedSettlement) {
    use erebus_core::{
        auth::{authorization_digest, Role},
        ids::AddressBytes,
    };
    let (mut deployment, mut prepared) = fixture();
    deployment.settlement_contract = [0x22; 20];
    prepared.operation_ref = [2; 32];
    let mut evidence = SettlementEvidence::decode(&prepared.backend_evidence).unwrap();
    let mut terms = AgreementTerms::decode(&evidence.terms).unwrap();
    terms.domain.settlement_contract = Some(AddressBytes::new(vec![0x22; 20]).unwrap());
    evidence.terms = terms.encode().unwrap();
    prepared.domain = terms.domain.clone();
    prepared.deal_commitment =
        commit_agreement(&terms, &CommitmentBlinding::from_bytes(evidence.blinding)).unwrap();
    prepared.deal_nullifier = deal_nullifier(&terms).unwrap();
    for (role, key, output) in [
        (Role::Buyer, [1u8; 32], &mut evidence.buyer_signature),
        (Role::Seller, [2u8; 32], &mut evidence.seller_signature),
    ] {
        let digest = authorization_digest(
            &terms.domain,
            role,
            &prepared.deal_commitment,
            terms.suite_id,
        )
        .unwrap();
        let (sig, recovery) = k256::ecdsa::SigningKey::from_slice(&key)
            .unwrap()
            .sign_prehash_recoverable(&digest)
            .unwrap();
        output[..64].copy_from_slice(&sig.to_bytes());
        output[64] = recovery.to_byte();
    }
    prepared.backend_evidence = evidence.encode();
    (deployment, prepared)
}

#[test]
fn concurrent_deployments_share_one_sender_slot() {
    use std::sync::{Arc, Barrier};
    let dir = tempfile::tempdir().unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = [fixture(), other_deployment()]
        .into_iter()
        .map(|(deployment, prepared)| {
            let journal = SignerJournal::open(dir.path(), deployment.chain_id, [7; 20]).unwrap();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                journal.reserve(&deployment, &prepared, 7, params().fees, params().gas_limit)
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        1,
        "{results:?}"
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(NonceError::Busy)))
            .count(),
        1,
        "{results:?}"
    );
}

#[test]
fn missing_signer_snapshot_never_reopens_an_empty_slot() {
    let dir = tempfile::tempdir().unwrap();
    let (deployment, prepared) = fixture();
    let journal = SignerJournal::open(dir.path(), deployment.chain_id, [7; 20]).unwrap();
    journal
        .reserve(&deployment, &prepared, 7, params().fees, params().gas_limit)
        .unwrap();
    let path = dir.path().join(format!(
        "eip155-{}-{}.json",
        deployment.chain_id,
        hex::encode([7; 20])
    ));
    std::fs::remove_file(path).unwrap();
    assert!(matches!(
        SignerJournal::open(dir.path(), deployment.chain_id, [7; 20]),
        Err(NonceError::Storage)
    ));
}

#[test]
fn nonce_write_faults_recover_without_reassignment() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    struct Fail {
        count: AtomicUsize,
        at: usize,
    }
    impl erebus_journal::FaultHook for Fail {
        fn after(&self, _: erebus_journal::Boundary<'_>) -> std::io::Result<()> {
            if self.count.fetch_add(1, Ordering::SeqCst) + 1 == self.at {
                Err(std::io::Error::other("injected failure"))
            } else {
                Ok(())
            }
        }
    }
    let mut boundaries = 0;
    for fail in 0..=16 {
        if fail > 0 && fail > boundaries {
            break;
        }
        let dir = tempfile::tempdir().unwrap();
        let (deployment, prepared) = fixture();
        SignerJournal::open(dir.path(), deployment.chain_id, [7; 20]).unwrap();
        let hook = Arc::new(Fail {
            count: AtomicUsize::new(0),
            at: if fail == 0 { usize::MAX } else { fail },
        });
        let journal =
            SignerJournal::open_with_faults(dir.path(), deployment.chain_id, [7; 20], hook.clone())
                .unwrap();
        let result = journal.reserve(&deployment, &prepared, 7, params().fees, params().gas_limit);
        if fail == 0 {
            result.unwrap();
            boundaries = hook.count.load(Ordering::SeqCst);
            assert!((1..=16).contains(&boundaries));
        } else {
            assert!(matches!(result, Err(NonceError::Storage)));
        }
        let restarted = SignerJournal::open(dir.path(), deployment.chain_id, [7; 20]).unwrap();
        let plan = restarted
            .reserve(&deployment, &prepared, 7, params().fees, params().gas_limit)
            .unwrap();
        assert_eq!(plan.params().nonce, 7);
        let mut next = prepared;
        next.operation_ref = [2; 32];
        assert!(matches!(
            restarted.reserve(&deployment, &next, 8, params().fees, params().gas_limit),
            Err(NonceError::Busy)
        ));
    }
}

async fn mock_chain() -> (
    erebus_evm::chain::EvmChain,
    alloy::transports::mock::Asserter,
) {
    use alloy::providers::{Provider, ProviderBuilder};
    let (deployment, _) = fixture();
    let responses = alloy::transports::mock::Asserter::new();
    responses.push_success(&alloy::primitives::U64::from(deployment.chain_id));
    let provider = ProviderBuilder::default()
        .connect_mocked_client(responses.clone())
        .erased();
    let chain = erebus_evm::chain::EvmChain::from_provider(
        deployment,
        provider,
        std::time::Duration::from_secs(1),
    )
    .await
    .unwrap();
    (chain, responses)
}

#[tokio::test]
async fn rpc_nonce_allocation_checks_chain_and_resumes_without_rpc() {
    use alloy::primitives::U64;
    let (deployment, prepared) = fixture();
    let (chain, responses) = mock_chain().await;
    let dir = tempfile::tempdir().unwrap();
    let journal = SignerJournal::open(dir.path(), deployment.chain_id, [7; 20]).unwrap();
    for value in [deployment.chain_id, 7, 7, deployment.chain_id] {
        responses.push_success(&U64::from(value));
    }
    let plan = chain
        .reserve_nonce(&journal, &prepared, params().fees, params().gas_limit)
        .await
        .unwrap();
    assert_eq!(plan.params().nonce, 7);
    assert!(responses.read_q().is_empty());
    // No responses are queued: restart recovery must not query a fresh nonce.
    assert_eq!(
        chain
            .reserve_nonce(&journal, &prepared, params().fees, params().gas_limit)
            .await
            .unwrap(),
        plan
    );
    let mut next = prepared;
    next.operation_ref = [2; 32];
    assert!(matches!(
        chain
            .reserve_nonce(&journal, &next, params().fees, params().gas_limit)
            .await,
        Err(NonceError::Busy)
    ));
}

#[tokio::test]
async fn rpc_nonce_disagreement_or_chain_change_never_creates_a_claim() {
    use alloy::primitives::U64;
    let (deployment, prepared) = fixture();
    for (pending, last_chain) in [(8, deployment.chain_id), (6, deployment.chain_id), (7, 1)] {
        let (chain, responses) = mock_chain().await;
        let dir = tempfile::tempdir().unwrap();
        let journal = SignerJournal::open(dir.path(), deployment.chain_id, [7; 20]).unwrap();
        for value in [deployment.chain_id, 7, pending, last_chain] {
            responses.push_success(&U64::from(value));
        }
        assert!(chain
            .reserve_nonce(&journal, &prepared, params().fees, params().gas_limit)
            .await
            .is_err());
        assert!(journal.resume(&deployment, &prepared).unwrap().is_none());
    }
    let (chain, responses) = mock_chain().await;
    let dir = tempfile::tempdir().unwrap();
    let journal = SignerJournal::open(dir.path(), deployment.chain_id, [7; 20]).unwrap();
    responses.push_failure_msg("nonce RPC contains a secret");
    let error = chain
        .reserve_nonce(&journal, &prepared, params().fees, params().gas_limit)
        .await
        .unwrap_err();
    assert!(!format!("{error:?}").contains("secret"));
    assert!(journal.resume(&deployment, &prepared).unwrap().is_none());
}

#[tokio::test]
async fn broadcast_never_turns_rpc_errors_into_nonpayment() {
    use alloy::primitives::{B256, U64};
    use erebus_evm::chain::{BroadcastOutcome, SigningPlan};
    let (deployment, prepared) = fixture();
    let key = TransactionKey::from_bytes(&[1; 32]).unwrap();
    let plan = SigningPlan::new(key.address(), params()).unwrap();
    let tx = plan.sign(&deployment, &prepared, &key).unwrap();
    for case in 0..5 {
        let (chain, responses) = mock_chain().await;
        responses.push_success(&U64::from(deployment.chain_id));
        match case {
            0 => responses.push_success(&B256::from(tx.hash())),
            1 => responses.push_success(&B256::ZERO),
            2 => responses.push_failure_msg("raw calldata secret"),
            3 => responses.push_success(&"malformed hash"),
            _ => {} // Empty queue produces a transport error.
        }
        let result = chain.broadcast(&prepared, &plan, tx.raw()).await.unwrap();
        assert_eq!(result.hash, tx.hash());
        match case {
            0 => assert_eq!(result.outcome, BroadcastOutcome::Acknowledged),
            2 => assert!(matches!(result.outcome, BroadcastOutcome::Rejected { .. })),
            _ => assert_eq!(result.outcome, BroadcastOutcome::Unknown),
        }
        assert!(!format!("{result:?}").contains("secret"));
    }
}

#[tokio::test]
async fn broadcast_validates_locally_and_checks_chain_before_sending() {
    use alloy::primitives::{B256, U64};
    use erebus_evm::chain::SigningPlan;
    let (deployment, prepared) = fixture();
    let key = TransactionKey::from_bytes(&[1; 32]).unwrap();
    let plan = SigningPlan::new(key.address(), params()).unwrap();
    let tx = plan.sign(&deployment, &prepared, &key).unwrap();
    let (chain, responses) = mock_chain().await;
    responses.push_success(&U64::from(1)); // Wrong live chain.
    responses.push_success(&B256::from(tx.hash()));
    assert!(chain.broadcast(&prepared, &plan, &[0]).await.is_err());
    assert_eq!(responses.read_q().len(), 2); // Invalid bytes reached no RPC.
    assert!(chain.broadcast(&prepared, &plan, tx.raw()).await.is_err());
    assert_eq!(responses.read_q().len(), 1); // No send after wrong-chain check.
    assert!(!format!("{chain:?}").contains(&deployment.rpc_url));
}
