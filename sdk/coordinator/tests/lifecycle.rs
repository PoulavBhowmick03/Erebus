//! Durable intent and policy tests. Backend facts are synthetic, not chain verification.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Barrier,
};

use erebus_coordinator::{BroadcastOutcome, Coordinator, Error, Stage};
use erebus_core::{
    auth::{Authorization, Role},
    commitment::{commit_agreement, deal_nullifier, CommitmentBlinding},
    deal_state::{DealEvidence, DealReads, DealState, WinningSettlement},
    ids::{BaseUnits, SignatureBytes},
    policy::{AggregateLimit, ReservationState, SpendingPolicy},
    settlement::{BackendCapabilities, PreparedSettlement, SettlementContext},
    terms::AgreementTerms,
};
use erebus_journal::{Boundary, FaultHook};

fn fixture(
    index: usize,
) -> (
    AgreementTerms,
    CommitmentBlinding,
    Authorization,
    Authorization,
) {
    let all: serde_json::Value = serde_json::from_str(include_str!(
        "../../core/tests/fixtures/agreement-v1-vectors.json"
    ))
    .unwrap();
    let v = &all["vectors"][index];
    let bytes = |value: &serde_json::Value| hex::decode(value.as_str().unwrap()).unwrap();
    let terms = AgreementTerms::decode(&bytes(&v["expected"]["canonicalHex"])).unwrap();
    let blinding = CommitmentBlinding::from_bytes(bytes(&v["blindingHex"]).try_into().unwrap());
    let commitment = commit_agreement(&terms, &blinding).unwrap();
    let auth = |role, key: &str| Authorization {
        role,
        suite_id: terms.suite_id,
        commitment,
        signature: SignatureBytes::new(bytes(&v["expected"][key])).unwrap(),
    };
    let buyer = auth(Role::Buyer, "buyerSignatureHex");
    let seller = auth(Role::Seller, "sellerSignatureHex");
    (terms, blinding, buyer, seller)
}

fn config(
    terms: &AgreementTerms,
    limit: u128,
) -> (SettlementContext, BackendCapabilities, SpendingPolicy) {
    let context = SettlementContext {
        require_local_proving: true,
        mode: terms.settlement_mode,
        domain: terms.domain.clone(),
        suite_id: terms.suite_id,
        asset: terms.asset.clone(),
        required_guarantees: terms.required_guarantees,
    };
    let capabilities = BackendCapabilities {
        suites: [terms.suite_id].into_iter().collect(),
        modes: [terms.settlement_mode].into_iter().collect(),
        guarantees: terms.required_guarantees,
        local_proving: true,
    };
    let policy = SpendingPolicy {
        per_deal_max: BaseUnits::new(limit),
        allowed_assets: [terms.asset.clone()].into_iter().collect(),
        per_asset_limits: [(
            terms.asset.clone(),
            AggregateLimit {
                max: BaseUnits::new(limit),
                window_seconds: 100,
            },
        )]
        .into_iter()
        .collect(),
        ..SpendingPolicy::default()
    };
    (context, capabilities, policy)
}

fn shielded_fixture() -> (
    AgreementTerms,
    CommitmentBlinding,
    Authorization,
    Authorization,
) {
    use erebus_core::{
        ids::{AddressBytes, AssetId, KeyBytes},
        terms::{GuaranteeSet, SettlementMode},
    };
    let (mut terms, _, _, _) = fixture(0);
    let v: serde_json::Value = serde_json::from_str(include_str!(
        "../../core/tests/fixtures/agreement-suite2-vector.json"
    ))
    .unwrap();
    let t = &v["terms"];
    let bytes = |v: &serde_json::Value| hex::decode(v.as_str().unwrap()).unwrap();
    terms.suite_id = 2;
    terms.settlement_mode = SettlementMode::Shielded;
    terms.required_guarantees = GuaranteeSet::from_bits(7).unwrap();
    terms.domain.pool = Some(AddressBytes::new(bytes(&t["domain"]["poolHex"])).unwrap());
    terms.domain.settlement_contract = terms.domain.pool.clone();
    terms.domain.verifier_version = 2;
    terms.deal_id = bytes(&t["dealIdHex"]).try_into().unwrap();
    terms.transcript_root = bytes(&t["transcriptRootHex"]).try_into().unwrap();
    terms.settlement_nonce = bytes(&t["settlementNonceHex"]).try_into().unwrap();
    terms.buyer_authorization_key = KeyBytes::new(bytes(&t["buyerAuthorizationKeyHex"])).unwrap();
    terms.seller_authorization_key = KeyBytes::new(bytes(&t["sellerAuthorizationKeyHex"])).unwrap();
    terms.payment_recipient = KeyBytes::new(bytes(&t["paymentRecipientHex"])).unwrap();
    terms.asset = AssetId::parse(t["asset"].as_str().unwrap()).unwrap();
    terms.amount = BaseUnits::new(70);
    terms.expiry = t["expiry"].as_u64().unwrap();
    let blinding = CommitmentBlinding::from_bytes(bytes(&v["blindingHex"]).try_into().unwrap());
    let commitment = commit_agreement(&terms, &blinding).unwrap();
    assert_eq!(
        hex::encode(commitment.as_bytes()),
        v["expected"]["commitmentHex"].as_str().unwrap()
    );
    let auth = |role, key: &str| Authorization {
        role,
        suite_id: 2,
        commitment,
        signature: SignatureBytes::new(bytes(&v["expected"][key])).unwrap(),
    };
    let buyer = auth(Role::Buyer, "buyerSignatureHex");
    let seller = auth(Role::Seller, "sellerSignatureHex");
    (terms, blinding, buyer, seller)
}

#[test]
fn shielded_agreements_use_the_same_durable_authorization_boundary_without_downgrade() {
    use erebus_core::terms::SettlementMode;
    let dir = tempfile::tempdir().unwrap();
    let (terms, blinding, buyer, seller) = shielded_fixture();
    let commitment = commit_agreement(&terms, &blinding).unwrap();
    let coordinator = open(dir.path(), &terms, 100);
    coordinator
        .record_intent([1; 32], &terms, &blinding, 10)
        .unwrap();
    coordinator
        .authorize_buyer([1; 32], 11, |_, _| Ok::<_, ()>(buyer.clone()))
        .unwrap();
    coordinator.accept_seller([1; 32], &seller).unwrap();
    let restarted = open(dir.path(), &terms, 100);
    assert_eq!(restarted.diagnostics().unwrap()[0].stage, Stage::Authorized);
    let (recorded_context, revisions) = restarted.recorded_deal([1; 32]).unwrap();
    assert_eq!(recorded_context, config(&terms, 100).0);
    assert_eq!(revisions.len(), 1);
    assert_eq!(revisions[0].amount(), terms.amount);
    assert_eq!(revisions[0].commitment(), commitment);
    assert_eq!(
        restarted
            .ledger()
            .unwrap()
            .reserved_total(&terms.asset)
            .unwrap()
            .get(),
        70
    );

    let (context, mut capabilities, policy) = config(&terms, 100);
    capabilities.modes = [SettlementMode::PublicBound].into_iter().collect();
    assert!(matches!(
        Coordinator::open(
            dir.path(),
            terms.buyer_authorization_key.clone(),
            context.clone(),
            &context,
            &capabilities,
            policy
        ),
        Err(Error::Context)
    ));
}

fn open(root: &std::path::Path, terms: &AgreementTerms, limit: u128) -> Coordinator {
    let (context, capabilities, policy) = config(terms, limit);
    Coordinator::open(
        root,
        terms.buyer_authorization_key.clone(),
        context.clone(),
        &context,
        &capabilities,
        policy,
    )
    .unwrap()
}

fn evidence(terms: &AgreementTerms, blinding: &CommitmentBlinding) -> DealEvidence {
    DealEvidence::Observed(DealReads {
        deal_nullifier: deal_nullifier(terms).unwrap(),
        final_anchor_timestamp: terms.expiry + 1,
        consumed_at_final: true,
        consumed_at_head: true,
        winner: Some(WinningSettlement {
            commitment: commit_agreement(terms, blinding).unwrap(),
            amount: terms.amount,
            fee: terms.fee_policy.fee,
            is_final: true,
        }),
    })
}

#[test]
fn intent_and_authorizations_survive_restart_without_double_reservation_or_signing() {
    let dir = tempfile::tempdir().unwrap();
    let (terms, blinding, buyer, seller) = fixture(0);
    let coordinator = open(dir.path(), &terms, 2_000_000);
    coordinator
        .record_intent([1; 32], &terms, &blinding, 10)
        .unwrap();
    coordinator.accept_seller([1; 32], &seller).unwrap();
    coordinator
        .authorize_buyer([1; 32], 11, |_, _| Ok::<_, ()>(buyer.clone()))
        .unwrap();
    assert_eq!(
        coordinator.diagnostics().unwrap()[0].stage,
        Stage::Authorized
    );
    drop(coordinator);
    // Tightening the policy cannot discard earlier uncertain reservations.
    let restarted = open(dir.path(), &terms, 1);
    restarted
        .record_intent([1; 32], &terms, &blinding, terms.expiry + 1)
        .unwrap();
    restarted
        .authorize_buyer::<()>([1; 32], 12, |_, _| panic!("already signed"))
        .unwrap();
    assert_eq!(
        restarted
            .ledger()
            .unwrap()
            .reserved_total(&terms.asset)
            .unwrap(),
        terms.amount
    );
    let mut changed = terms.clone();
    changed.amount = BaseUnits::new(2);
    assert!(matches!(
        restarted.record_intent([1; 32], &changed, &blinding, 13),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        restarted.record_intent([2; 32], &terms, &blinding, 13),
        Err(Error::Policy)
    ));
}

#[test]
fn unknown_consumption_holds_and_verified_winner_reconciles_all_revisions_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let (first, first_blinding, _, _) = fixture(0);
    let (second, second_blinding, _, _) = fixture(1);
    let coordinator = open(dir.path(), &first, 3_000_000);
    coordinator
        .record_intent([1; 32], &first, &first_blinding, 10)
        .unwrap();
    coordinator
        .record_intent([2; 32], &second, &second_blinding, 11)
        .unwrap();
    let amount = first.amount.get() + second.amount.get();
    let unresolved = DealEvidence::Observed(DealReads {
        deal_nullifier: deal_nullifier(&first).unwrap(),
        final_anchor_timestamp: first.expiry + 1,
        consumed_at_final: false,
        consumed_at_head: true,
        winner: None,
    });
    let assessment = coordinator.reconcile([1; 32], &unresolved, 12).unwrap();
    assert_eq!(assessment.state, DealState::ConsumedUnresolved);
    assert_eq!(
        coordinator
            .ledger()
            .unwrap()
            .reserved_total(&first.asset)
            .unwrap()
            .get(),
        amount
    );
    coordinator
        .reconcile([1; 32], &DealEvidence::Unknown, 13)
        .unwrap();
    coordinator
        .reconcile([1; 32], &evidence(&second, &second_blinding), 14)
        .unwrap();
    let ledger = coordinator.ledger().unwrap();
    assert_eq!(ledger.reservations()[0].state, ReservationState::Released);
    assert_eq!(ledger.reservations()[1].state, ReservationState::Committed);
    coordinator
        .reconcile([1; 32], &evidence(&second, &second_blinding), 15)
        .unwrap();
    assert_eq!(
        coordinator.ledger().unwrap().reservations(),
        ledger.reservations()
    );
}

#[test]
fn concurrent_callers_cannot_overspend_the_shared_budget() {
    let dir = tempfile::tempdir().unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let mut tasks = Vec::new();
    for index in 0..2 {
        let path = dir.path().to_path_buf();
        let barrier = barrier.clone();
        tasks.push(std::thread::spawn(move || {
            let (terms, blinding, _, _) = fixture(index);
            let coordinator = open(&path, &terms, 1_500_000);
            barrier.wait();
            coordinator.record_intent([index as u8 + 1; 32], &terms, &blinding, 10)
        }));
    }
    let outcomes: Vec<_> = tasks.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| matches!(r, Err(Error::Policy)))
            .count(),
        1
    );
}

#[test]
fn mismatched_context_wrong_roles_and_local_expiry_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let (terms, blinding, buyer, seller) = fixture(0);
    let (context, capabilities, policy) = config(&terms, 2_000_000);
    let mut peer = context.clone();
    peer.require_local_proving = false;
    assert!(matches!(
        Coordinator::open(
            dir.path(),
            terms.buyer_authorization_key.clone(),
            context,
            &peer,
            &capabilities,
            policy
        ),
        Err(Error::Context)
    ));
    let coordinator = open(dir.path(), &terms, 2_000_000);
    coordinator
        .record_intent([1; 32], &terms, &blinding, 10)
        .unwrap();
    assert!(matches!(
        coordinator.accept_seller([1; 32], &buyer),
        Err(Error::Agreement)
    ));
    assert!(matches!(
        coordinator.authorize_buyer([1; 32], 11, |_, _| Ok::<_, ()>(seller)),
        Err(Error::Agreement)
    ));
    assert_eq!(
        coordinator.diagnostics().unwrap()[0].stage,
        Stage::AuthorizationUnknown
    );
    assert!(matches!(
        coordinator.authorize_buyer::<()>([1; 32], terms.expiry, |_, _| panic!("expired")),
        Err(Error::Expired)
    ));
    assert_eq!(
        coordinator
            .ledger()
            .unwrap()
            .reserved_total(&terms.asset)
            .unwrap(),
        terms.amount
    );
}

#[test]
fn prepare_runs_after_both_authorizations_and_persists_only_bound_backend_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let (terms, blinding, buyer, seller) = fixture(0);
    let coordinator = open(dir.path(), &terms, 2_000_000);
    coordinator
        .record_intent([1; 32], &terms, &blinding, 10)
        .unwrap();
    assert!(matches!(
        coordinator.prepare::<()>([1; 32], 11, |_, _, _, _| panic!("unsigned")),
        Err(Error::Stage)
    ));
    coordinator.accept_seller([1; 32], &seller).unwrap();
    coordinator
        .authorize_buyer([1; 32], 11, |_, _| Ok::<_, ()>(buyer))
        .unwrap();
    let prepared = PreparedSettlement {
        operation_ref: [1; 32],
        deal_commitment: commit_agreement(&terms, &blinding).unwrap(),
        deal_nullifier: deal_nullifier(&terms).unwrap(),
        domain: terms.domain.clone(),
        mode: terms.settlement_mode,
        required_guarantees: terms.required_guarantees,
        backend_evidence: vec![1, 2, 3],
    };
    let mut wrong = prepared.clone();
    wrong.operation_ref = [2; 32];
    assert!(matches!(
        coordinator.prepare([1; 32], 12, |_, _, _, _| Ok::<_, ()>(wrong)),
        Err(Error::Context)
    ));
    coordinator
        .prepare([1; 32], 12, |_, _, _, _| Ok::<_, ()>(prepared.clone()))
        .unwrap();
    drop(coordinator);
    let restarted = open(dir.path(), &terms, 2_000_000);
    assert_eq!(restarted.prepared_settlement([1; 32]).unwrap(), prepared);
    assert_eq!(
        restarted
            .prepare::<()>([1; 32], 13, |_, _, _, _| panic!("already prepared"))
            .unwrap(),
        prepared
    );
    assert_eq!(restarted.diagnostics().unwrap()[0].stage, Stage::Prepared);
}

struct FailAt {
    count: AtomicUsize,
    fail: usize,
}

// Evidence and signed bytes are synthetic here. This sweep verifies storage/retry semantics,
// not pool verification or payment execution (covered by the funded chain runners).
fn matrix_phase(
    coordinator: &Coordinator,
    phase: usize,
    terms: &AgreementTerms,
    blinding: &CommitmentBlinding,
    buyer: &Authorization,
    seller: &Authorization,
) -> Result<(), Error> {
    let operation = [1; 32];
    match phase {
        0 => coordinator
            .record_intent(operation, terms, blinding, 10)
            .map(|_| ()),
        1 => coordinator
            .authorize_buyer(operation, 11, |_, _| Ok::<_, ()>(buyer.clone()))
            .map(|_| ()),
        2 => coordinator.accept_seller(operation, seller),
        3 => coordinator
            .prepare(operation, 12, |_, _, _, _| {
                Ok::<_, ()>(PreparedSettlement {
                    operation_ref: operation,
                    deal_commitment: commit_agreement(terms, blinding).unwrap(),
                    deal_nullifier: deal_nullifier(terms).unwrap(),
                    domain: terms.domain.clone(),
                    mode: terms.settlement_mode,
                    required_guarantees: terms.required_guarantees,
                    backend_evidence: vec![1, 2, 3],
                })
            })
            .map(|_| ()),
        4 => coordinator
            .sign_transaction(
                operation,
                13,
                b"matrix-plan",
                |_, _| Ok::<_, ()>(vec![42; 100]),
                |_, _, raw| {
                    assert_eq!(raw, [42; 100]);
                    Ok(())
                },
            )
            .map(|_| ()),
        5 => coordinator
            .begin_broadcast_attempt(operation, 14)
            .map(|_| ()),
        6 => {
            let attempt = coordinator.begin_broadcast_attempt(operation, 14)?;
            coordinator.finish_broadcast_attempt(
                operation,
                &attempt.token,
                BroadcastOutcome::Submitted,
            )
        }
        7 => coordinator
            .reconcile(operation, &evidence(terms, blinding), 15)
            .map(|_| ()),
        _ => unreachable!(),
    }
}

#[test]
#[ignore = "exhaustive cryptographic storage sweep; run in release mode"]
fn every_discovered_lifecycle_write_is_restartable_in_both_settlement_modes() {
    for (terms, blinding, buyer, seller) in [fixture(0), shielded_fixture()] {
        for phase in 0..8 {
            let mut boundary_count = 0;
            let mut fail = 0;
            loop {
                let dir = tempfile::tempdir().unwrap();
                let initial = open(dir.path(), &terms, 2_000_000);
                for prior in 0..phase {
                    matrix_phase(&initial, prior, &terms, &blinding, &buyer, &seller).unwrap();
                }
                drop(initial);
                let (context, capabilities, policy) = config(&terms, 2_000_000);
                let hook = Arc::new(FailAt {
                    count: AtomicUsize::new(0),
                    fail: if fail == 0 { usize::MAX } else { fail },
                });
                let coordinator = Coordinator::open_with_faults(
                    dir.path(),
                    terms.buyer_authorization_key.clone(),
                    context.clone(),
                    &context,
                    &capabilities,
                    policy,
                    hook.clone(),
                )
                .unwrap();
                let result = matrix_phase(&coordinator, phase, &terms, &blinding, &buyer, &seller);
                if fail == 0 {
                    result.unwrap();
                    boundary_count = hook.count.load(Ordering::SeqCst);
                    assert!(
                        boundary_count > 0,
                        "phase {phase} must persist a transition"
                    );
                } else {
                    assert!(
                        matches!(result, Err(Error::Storage)),
                        "suite {}, phase {phase}, boundary {fail}",
                        terms.suite_id
                    );
                }
                drop(coordinator);
                let restarted = open(dir.path(), &terms, 2_000_000);
                // A failed rename/sync may have persisted the transition. Retry must accept
                // that exact intent rather than duplicate its reservation or change its bytes.
                matrix_phase(&restarted, phase, &terms, &blinding, &buyer, &seller).unwrap();
                for remaining in phase + 1..8 {
                    matrix_phase(&restarted, remaining, &terms, &blinding, &buyer, &seller)
                        .unwrap();
                }
                assert_eq!(restarted.diagnostics().unwrap()[0].stage, Stage::Finalized);
                let ledger = restarted.ledger().unwrap();
                assert_eq!(ledger.reservations().len(), 1);
                assert_eq!(ledger.reservations()[0].state, ReservationState::Committed);
                assert_eq!(
                    restarted
                        .signed_transaction([1; 32])
                        .unwrap()
                        .unwrap()
                        .raw(),
                    [42; 100]
                );
                fail += 1;
                if fail > boundary_count {
                    break;
                }
            }
        }
    }
}

fn prepared_coordinator(root: &std::path::Path) -> (Coordinator, AgreementTerms) {
    let (terms, blinding, buyer, seller) = fixture(0);
    let coordinator = open(root, &terms, 2_000_000);
    coordinator
        .record_intent([1; 32], &terms, &blinding, 10)
        .unwrap();
    coordinator
        .authorize_buyer([1; 32], 11, |_, _| Ok::<_, ()>(buyer))
        .unwrap();
    coordinator.accept_seller([1; 32], &seller).unwrap();
    coordinator
        .prepare([1; 32], 12, |_, _, _, _| {
            Ok::<_, ()>(PreparedSettlement {
                operation_ref: [1; 32],
                deal_commitment: commit_agreement(&terms, &blinding).unwrap(),
                deal_nullifier: deal_nullifier(&terms).unwrap(),
                domain: terms.domain.clone(),
                mode: terms.settlement_mode,
                required_guarantees: terms.required_guarantees,
                backend_evidence: vec![1, 2, 3],
            })
        })
        .unwrap();
    (coordinator, terms)
}

fn signed_coordinator(root: &std::path::Path) -> (Coordinator, AgreementTerms) {
    let (coordinator, terms) = prepared_coordinator(root);
    coordinator
        .sign_transaction(
            [1; 32],
            13,
            b"private-plan",
            |_, _| Ok::<_, ()>(b"private-signed-transaction".to_vec()),
            |_, _, _| Ok(()),
        )
        .unwrap();
    (coordinator, terms)
}

#[test]
fn initial_broadcast_fence_is_atomic_and_never_retries_unknown_history() {
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, terms) = signed_coordinator(dir.path());
    let barrier = Arc::new(Barrier::new(8));
    let wins = std::thread::scope(|scope| {
        let mut threads = Vec::new();
        for _ in 0..8 {
            let barrier = barrier.clone();
            let coordinator = &coordinator;
            threads.push(scope.spawn(move || {
                barrier.wait();
                match coordinator.begin_initial_broadcast_attempt([1; 32], 14) {
                    Ok(_) => true,
                    Err(Error::Conflict) => false,
                    other => panic!("unexpected fence outcome: {other:?}"),
                }
            }));
        }
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(|won| *won)
            .count()
    });
    assert_eq!(wins, 1);
    let history = coordinator.broadcast_history([1; 32]).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].outcome, BroadcastOutcome::Unknown);
    let restarted = open(dir.path(), &terms, 2_000_000);
    assert!(matches!(
        restarted.begin_initial_broadcast_attempt([1; 32], 15),
        Err(Error::Conflict)
    ));
    // The owner-controlled retry API still appends an explicit attempt without erasing the first.
    restarted.begin_broadcast_attempt([1; 32], 15).unwrap();
    assert_eq!(restarted.broadcast_history([1; 32]).unwrap().len(), 2);

    let replaced_dir = tempfile::tempdir().unwrap();
    let (replaced, _) = signed_coordinator(replaced_dir.path());
    replaced
        .sign_replacement(
            [1; 32],
            14,
            b"replacement-plan",
            |_, _| Ok::<_, ()>(b"replacement-transaction".to_vec()),
            |_, _, _| Ok(()),
        )
        .unwrap();
    assert!(matches!(
        replaced.begin_initial_broadcast_attempt([1; 32], 15),
        Err(Error::Conflict)
    ));
    assert!(replaced.broadcast_history([1; 32]).unwrap().is_empty());
}

#[test]
fn broadcast_restart_preserves_unknown_attempts_and_scopes_late_responses() {
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, terms) = signed_coordinator(dir.path());
    let first = coordinator.begin_broadcast_attempt([1; 32], 14).unwrap();
    assert_eq!(first.transaction.raw(), b"private-signed-transaction");
    assert_eq!(first.transaction.plan(), b"private-plan");
    assert_eq!(first.transaction.prepared().operation_ref, [1; 32]);
    assert_eq!(format!("{first:?}"), "BroadcastAttempt { token: AttemptToken { [REDACTED] }, transaction: SignedTransaction { [REDACTED] } }");
    drop(coordinator);
    let restarted = open(dir.path(), &terms, 2_000_000);
    assert_eq!(
        restarted.diagnostics().unwrap()[0].stage,
        Stage::BroadcastUnknown
    );
    let second = restarted.begin_broadcast_attempt([1; 32], 15).unwrap();
    restarted
        .finish_broadcast_attempt([1; 32], &second.token, BroadcastOutcome::Submitted)
        .unwrap();
    assert_eq!(restarted.diagnostics().unwrap()[0].unknown_attempts, 1);
    assert_eq!(
        restarted.diagnostics().unwrap()[0].stage,
        Stage::BroadcastUnknown
    );
    assert!(matches!(
        restarted.finish_broadcast_attempt([2; 32], &first.token, BroadcastOutcome::Rejected),
        Err(Error::Conflict)
    ));
    let other_dir = tempfile::tempdir().unwrap();
    let (other, _) = signed_coordinator(other_dir.path());
    other.begin_broadcast_attempt([1; 32], 14).unwrap();
    assert!(matches!(
        other.finish_broadcast_attempt([1; 32], &first.token, BroadcastOutcome::Rejected),
        Err(Error::Conflict)
    ));
    restarted
        .finish_broadcast_attempt([1; 32], &first.token, BroadcastOutcome::Rejected)
        .unwrap();
    restarted
        .finish_broadcast_attempt([1; 32], &second.token, BroadcastOutcome::Unknown)
        .unwrap();
    assert!(matches!(
        restarted.finish_broadcast_attempt([1; 32], &second.token, BroadcastOutcome::Rejected),
        Err(Error::Conflict)
    ));
    let history = restarted.broadcast_history([1; 32]).unwrap();
    assert_eq!(
        history.iter().map(|a| a.outcome).collect::<Vec<_>>(),
        [BroadcastOutcome::Rejected, BroadcastOutcome::Submitted]
    );
    assert_eq!(restarted.diagnostics().unwrap()[0].stage, Stage::Submitted);
    assert_eq!(
        restarted
            .ledger()
            .unwrap()
            .reserved_total(&terms.asset)
            .unwrap(),
        terms.amount
    );
    assert!(matches!(
        restarted.begin_broadcast_attempt([1; 32], terms.expiry),
        Err(Error::Expired)
    ));
    assert_eq!(
        restarted
            .signed_transaction([1; 32])
            .unwrap()
            .unwrap()
            .raw(),
        first.transaction.raw()
    );
}

#[test]
fn broadcast_payload_holds_no_lock_and_concurrent_attempts_append() {
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, terms) = signed_coordinator(dir.path());
    let payload = coordinator.begin_broadcast_attempt([1; 32], 14).unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let root = dir.path().to_owned();
    let worker = std::thread::spawn(move || {
        let reopened = open(&root, &terms, 2_000_000);
        let attempt = reopened.begin_broadcast_attempt([1; 32], 15).unwrap();
        reopened
            .finish_broadcast_attempt([1; 32], &attempt.token, BroadcastOutcome::Rejected)
            .unwrap();
        send.send(()).unwrap();
    });
    receive
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    worker.join().unwrap();
    coordinator
        .finish_broadcast_attempt([1; 32], &payload.token, BroadcastOutcome::Submitted)
        .unwrap();
    assert_eq!(coordinator.broadcast_history([1; 32]).unwrap().len(), 2);
}

#[test]
fn replacements_keep_initial_bytes_fence_plans_and_link_broadcasts() {
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, terms) = signed_coordinator(dir.path());
    let first = coordinator.begin_broadcast_attempt([1; 32], 14).unwrap();
    assert!(matches!(
        coordinator.sign_replacement(
            [1; 32],
            15,
            b"bump",
            |previous, plan| {
                assert_eq!(previous.raw(), b"private-signed-transaction");
                assert_eq!(plan, b"bump");
                Err::<Vec<u8>, ()>(())
            },
            |_, _, _| Ok(())
        ),
        Err(Error::Backend)
    ));
    drop(coordinator);
    let restarted = open(dir.path(), &terms, 2_000_000);
    assert_eq!(
        restarted.replacement_plans([1; 32]).unwrap(),
        [b"bump".to_vec()]
    );
    assert!(restarted
        .signed_transaction_at([1; 32], 1)
        .unwrap()
        .is_none());
    assert!(matches!(
        restarted.begin_broadcast_attempt([1; 32], 16),
        Err(Error::Stage)
    ));
    assert!(matches!(
        restarted.sign_replacement(
            [1; 32],
            16,
            b"different",
            |_, _| Ok::<_, ()>(vec![9]),
            |_, _, _| Ok(())
        ),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        restarted.sign_replacement(
            [1; 32],
            16,
            b"bump",
            |_, _| Ok::<_, ()>(vec![9]),
            |_, _, _| Err(())
        ),
        Err(Error::Backend)
    ));
    assert_eq!(
        restarted
            .sign_replacement(
                [1; 32],
                16,
                b"bump",
                |_, _| Ok::<_, ()>(vec![10]),
                |previous, plan, raw| {
                    assert_eq!(previous.index(), 0);
                    assert_eq!(plan, b"bump");
                    assert_eq!(raw, [10]);
                    Ok(())
                }
            )
            .unwrap(),
        1
    );
    restarted
        .sign_replacement(
            [1; 32],
            17,
            b"bump",
            |_, _| panic!("must not resign"),
            |_, _, _| Ok::<_, ()>(()),
        )
        .unwrap();
    let second = restarted.begin_broadcast_attempt([1; 32], 17).unwrap();
    assert_eq!(second.transaction.index(), 1);
    assert_eq!(second.transaction.raw(), [10]);
    restarted
        .finish_broadcast_attempt([1; 32], &second.token, BroadcastOutcome::Rejected)
        .unwrap();
    assert_eq!(
        restarted
            .broadcast_history([1; 32])
            .unwrap()
            .iter()
            .map(|a| a.transaction_index)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    assert_eq!(restarted.diagnostics().unwrap()[0].unknown_attempts, 1);
    assert_eq!(
        restarted
            .signed_transaction([1; 32])
            .unwrap()
            .unwrap()
            .raw(),
        first.transaction.raw()
    );
    assert_eq!(
        restarted.transaction_plan([1; 32]).unwrap().unwrap(),
        b"private-plan"
    );
    assert_eq!(
        restarted
            .ledger()
            .unwrap()
            .reserved_total(&terms.asset)
            .unwrap(),
        terms.amount
    );
    for index in 2..=16 {
        restarted
            .sign_replacement(
                [1; 32],
                18,
                &[index],
                |previous, _| {
                    assert_eq!(previous.index(), usize::from(index) - 1);
                    Ok::<_, ()>(vec![index])
                },
                |_, _, _| Ok(()),
            )
            .unwrap();
    }
    assert!(matches!(
        restarted.sign_replacement(
            [1; 32],
            18,
            &[17],
            |_, _| Ok::<_, ()>(vec![17]),
            |_, _, _| Ok(())
        ),
        Err(Error::Stage)
    ));
}

#[test]
fn broadcast_recovery_and_replacement_fault_boundaries() {
    // Begin, finish, raw recovery, and replacement each fail after every store boundary.
    for operation in 0..4 {
        let mut boundary_count = 0;
        for fail in 0..=8 {
            if fail > 0 && fail > boundary_count {
                break;
            }
            let dir = tempfile::tempdir().unwrap();
            let (coordinator, terms) = signed_coordinator(dir.path());
            let first = coordinator.begin_broadcast_attempt([1; 32], 14).unwrap();
            drop(coordinator);
            let (context, capabilities, policy) = config(&terms, 2_000_000);
            let hook = Arc::new(FailAt {
                count: AtomicUsize::new(0),
                fail: if fail == 0 { usize::MAX } else { fail },
            });
            let coordinator = Coordinator::open_with_faults(
                dir.path(),
                terms.buyer_authorization_key.clone(),
                context.clone(),
                &context,
                &capabilities,
                policy,
                hook.clone(),
            )
            .unwrap();
            let result = match operation {
                0 => coordinator.begin_broadcast_attempt([1; 32], 15).map(|_| ()),
                1 => coordinator.finish_broadcast_attempt(
                    [1; 32],
                    &first.token,
                    BroadcastOutcome::Submitted,
                ),
                2 => coordinator.signed_transaction([1; 32]).map(|_| ()),
                _ => coordinator
                    .sign_replacement(
                        [1; 32],
                        15,
                        b"bump",
                        |_, _| Ok::<_, ()>(vec![10]),
                        |_, _, _| Ok(()),
                    )
                    .map(|_| ()),
            };
            if fail == 0 {
                result.unwrap();
                boundary_count = hook.count.load(Ordering::SeqCst);
                assert_eq!(boundary_count, if operation == 3 { 8 } else { 4 });
            } else {
                assert!(matches!(result, Err(Error::Storage)));
            }
            drop(coordinator);
            let restarted = open(dir.path(), &terms, 2_000_000);
            let history = restarted.broadcast_history([1; 32]).unwrap();
            assert!(!history.is_empty());
            if operation == 0 {
                assert_eq!(history.len(), if fail == 1 { 1 } else { 2 });
            }
            if operation == 1 {
                assert_eq!(
                    history[0].outcome,
                    if fail == 1 {
                        BroadcastOutcome::Unknown
                    } else {
                        BroadcastOutcome::Submitted
                    }
                );
            }
            if operation != 1 {
                assert_eq!(history[0].outcome, BroadcastOutcome::Unknown);
            }
            if operation == 0 {
                let count = history.len();
                restarted.begin_broadcast_attempt([1; 32], 16).unwrap();
                assert_eq!(
                    restarted.broadcast_history([1; 32]).unwrap().len(),
                    count + 1
                );
                assert!(restarted
                    .broadcast_history([1; 32])
                    .unwrap()
                    .iter()
                    .all(|a| a.outcome == BroadcastOutcome::Unknown));
            } else if operation == 1 {
                restarted
                    .finish_broadcast_attempt([1; 32], &first.token, BroadcastOutcome::Submitted)
                    .unwrap();
                assert_eq!(
                    restarted.broadcast_history([1; 32]).unwrap()[0].outcome,
                    BroadcastOutcome::Submitted
                );
            } else if operation == 3 {
                let signed = restarted
                    .signed_transaction_at([1; 32], 1)
                    .ok()
                    .flatten()
                    .is_some();
                restarted
                    .sign_replacement(
                        [1; 32],
                        16,
                        b"bump",
                        |_, _| {
                            assert!(!signed);
                            Ok::<_, ()>(vec![10])
                        },
                        |_, _, _| Ok(()),
                    )
                    .unwrap();
                assert_eq!(
                    restarted
                        .signed_transaction_at([1; 32], 1)
                        .unwrap()
                        .unwrap()
                        .raw(),
                    [10]
                );
            }
            assert_eq!(
                restarted
                    .signed_transaction([1; 32])
                    .unwrap()
                    .unwrap()
                    .raw(),
                b"private-signed-transaction"
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
    }
}

#[test]
fn malformed_and_full_histories_fail_closed_and_old_snapshots_remain_readable() {
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, terms) = signed_coordinator(dir.path());
    let path = dir.path().join("coordinator.json");
    let original: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let mut legacy = original.clone();
    legacy["intents"][0]
        .as_object_mut()
        .unwrap()
        .remove("broadcast_attempts");
    legacy["intents"][0]
        .as_object_mut()
        .unwrap()
        .remove("replacements");
    std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert!(coordinator.broadcast_history([1; 32]).unwrap().is_empty());
    let mut full = original.clone();
    full["intents"][0]["broadcast_attempts"] = (1_u64..=1024)
        .map(|index| {
            let mut token = [0_u8; 32];
            token[..8].copy_from_slice(&index.to_le_bytes());
            serde_json::json!({"token": token, "outcome": "Unknown", "transaction_index": 0})
        })
        .collect::<Vec<_>>()
        .into();
    std::fs::write(&path, serde_json::to_vec(&full).unwrap()).unwrap();
    assert!(matches!(
        coordinator.begin_broadcast_attempt([1; 32], 14),
        Err(Error::Stage)
    ));
    assert_eq!(coordinator.broadcast_history([1; 32]).unwrap().len(), 1024);
    assert_eq!(
        coordinator
            .ledger()
            .unwrap()
            .reserved_total(&terms.asset)
            .unwrap(),
        terms.amount
    );
    for corruption in 0..5 {
        let mut invalid = full.clone();
        match corruption {
            0 => {
                invalid["intents"][0]["broadcast_attempts"][1] =
                    invalid["intents"][0]["broadcast_attempts"][0].clone()
            }
            1 => invalid["intents"][0]["broadcast_attempts"][0]["transaction_index"] = 1.into(),
            2 => invalid["intents"][0]["signed_transaction"] = serde_json::Value::Null,
            3 => {
                let token = [7_u8; 32].to_vec();
                invalid["intents"][0]["broadcast_attempts"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"token": token, "outcome": "Unknown", "transaction_index": 0}));
            }
            _ => {
                invalid["intents"][0]["replacements"] =
                    serde_json::json!([{"plan": [1], "raw": null}, {"plan": [2], "raw": [3]}])
            }
        }
        std::fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(matches!(coordinator.diagnostics(), Err(Error::Storage)));
    }
}

#[test]
fn late_broadcast_outcome_does_not_change_reconciled_accounting() {
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, terms) = signed_coordinator(dir.path());
    let (_, blinding, _, _) = fixture(0);
    let attempt = coordinator.begin_broadcast_attempt([1; 32], 14).unwrap();
    coordinator
        .reconcile([1; 32], &evidence(&terms, &blinding), 20)
        .unwrap();
    assert_eq!(
        coordinator.diagnostics().unwrap()[0].stage,
        Stage::Finalized
    );
    coordinator
        .finish_broadcast_attempt([1; 32], &attempt.token, BroadcastOutcome::Rejected)
        .unwrap();
    assert_eq!(
        coordinator.diagnostics().unwrap()[0].stage,
        Stage::Finalized
    );
    assert_eq!(
        coordinator.ledger().unwrap().reservations()[0].state,
        ReservationState::Committed
    );
    assert!(matches!(
        coordinator.begin_broadcast_attempt([1; 32], 21),
        Err(Error::Stage)
    ));
    assert_eq!(
        coordinator
            .signed_transaction([1; 32])
            .unwrap()
            .unwrap()
            .raw(),
        attempt.transaction.raw()
    );
}

#[test]
fn signed_bytes_survive_restart_and_are_revalidated_without_resigning() {
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, terms) = prepared_coordinator(dir.path());
    let raw = vec![42; 100]; // Synthetic backend bytes, not a valid EVM transaction.
    assert!(matches!(
        coordinator.sign_transaction(
            [1; 32],
            13,
            b"plan",
            |_, _| Ok::<_, ()>(raw.clone()),
            |_, _, _| Err(())
        ),
        Err(Error::Backend)
    ));
    assert_eq!(coordinator.diagnostics().unwrap()[0].stage, Stage::Prepared);
    assert_eq!(
        coordinator.transaction_plan([1; 32]).unwrap(),
        Some(b"plan".to_vec())
    );
    assert!(matches!(
        coordinator.sign_transaction::<()>(
            [1; 32],
            13,
            b"different plan",
            |_, _| panic!("plan conflict must precede signing"),
            |_, _, _| panic!("plan conflict must precede validation")
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(
        coordinator
            .sign_transaction(
                [1; 32],
                13,
                b"plan",
                |_, _| Ok::<_, ()>(raw.clone()),
                |p, plan, bytes| {
                    assert_eq!(plan, b"plan");
                    assert_eq!(p.backend_evidence, [1, 2, 3]);
                    assert_eq!(bytes, raw);
                    Ok(())
                }
            )
            .unwrap(),
        raw
    );
    drop(coordinator);
    let restarted = open(dir.path(), &terms, 2_000_000);
    assert_eq!(restarted.diagnostics().unwrap()[0].stage, Stage::Signed);
    assert!(matches!(
        restarted.sign_transaction::<()>(
            [1; 32],
            14,
            b"plan",
            |_, _| panic!("must not sign again"),
            |_, _, _| Err(())
        ),
        Err(Error::Backend)
    ));
    assert_eq!(
        restarted
            .sign_transaction::<()>(
                [1; 32],
                14,
                b"plan",
                |_, _| panic!("must not sign again"),
                |_, _, bytes| {
                    assert_eq!(bytes, raw);
                    Ok(())
                }
            )
            .unwrap(),
        raw
    );
    assert!(matches!(
        restarted.sign_transaction::<()>(
            [1; 32],
            terms.expiry,
            b"plan",
            |_, _| panic!("expired"),
            |_, _, _| panic!("expired")
        ),
        Err(Error::Expired)
    ));
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
fn signed_transaction_storage_faults_never_return_unpersisted_bytes() {
    let mut boundary_count = 0;
    // The first pass counts the boundaries; each subsequent pass fails at one of them.
    for fail in 0..=16 {
        if fail > 0 && fail > boundary_count {
            break;
        }
        let dir = tempfile::tempdir().unwrap();
        let (_, terms) = prepared_coordinator(dir.path());
        let (context, capabilities, policy) = config(&terms, 2_000_000);
        let hook = Arc::new(FailAt {
            count: AtomicUsize::new(0),
            fail: if fail == 0 { usize::MAX } else { fail },
        });
        let coordinator = Coordinator::open_with_faults(
            dir.path(),
            terms.buyer_authorization_key.clone(),
            context.clone(),
            &context,
            &capabilities,
            policy,
            hook.clone(),
        )
        .unwrap();
        let result = coordinator.sign_transaction(
            [1; 32],
            13,
            b"plan",
            |_, _| Ok::<_, ()>(vec![42; 100]),
            |_, _, _| Ok(()),
        );
        if fail == 0 {
            assert!(result.is_ok());
            boundary_count = hook.count.load(Ordering::SeqCst);
            assert!((1..=16).contains(&boundary_count));
        } else {
            assert!(matches!(result, Err(Error::Storage)));
        }
        drop(coordinator);
        let restarted = open(dir.path(), &terms, 2_000_000);
        let already_signed = restarted.diagnostics().unwrap()[0].stage == Stage::Signed;
        assert_eq!(
            restarted
                .sign_transaction(
                    [1; 32],
                    14,
                    b"plan",
                    |_, _| {
                        assert!(!already_signed);
                        Ok::<_, ()>(vec![42; 100])
                    },
                    |_, _, raw| {
                        assert_eq!(raw, [42; 100]);
                        Ok(())
                    }
                )
                .unwrap(),
            [42; 100]
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
}

#[test]
fn transaction_signing_requires_preparation_and_bounded_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (terms, blinding, _, _) = fixture(0);
    let coordinator = open(dir.path(), &terms, 2_000_000);
    coordinator
        .record_intent([1; 32], &terms, &blinding, 10)
        .unwrap();
    assert!(matches!(
        coordinator.sign_transaction::<()>(
            [1; 32],
            11,
            b"plan",
            |_, _| panic!("not prepared"),
            |_, _, _| panic!("not prepared")
        ),
        Err(Error::Stage)
    ));
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, _) = prepared_coordinator(dir.path());
    for plan in [vec![], vec![1; 1025]] {
        assert!(matches!(
            coordinator.sign_transaction::<()>(
                [1; 32],
                13,
                &plan,
                |_, _| panic!("invalid plan"),
                |_, _, _| panic!("invalid plan")
            ),
            Err(Error::Agreement)
        ));
    }
    assert_eq!(coordinator.transaction_plan([1; 32]).unwrap(), None);
    for raw in [vec![], vec![1; 128 * 1024 + 1]] {
        assert!(matches!(
            coordinator.sign_transaction(
                [1; 32],
                13,
                b"plan",
                |_, _| Ok::<_, ()>(raw),
                |_, _, _| panic!("invalid size")
            ),
            Err(Error::Agreement)
        ));
    }
    assert_eq!(coordinator.diagnostics().unwrap()[0].stage, Stage::Prepared);
}

#[test]
fn concurrent_signing_with_different_plans_cannot_replace_the_first_plan() {
    let dir = tempfile::tempdir().unwrap();
    let (_, terms) = prepared_coordinator(dir.path());
    let barrier = Arc::new(Barrier::new(2));
    let calls = Arc::new(AtomicUsize::new(0));
    let handles: Vec<_> = [1u8, 2]
        .into_iter()
        .map(|n| {
            let coordinator = open(dir.path(), &terms, 2_000_000);
            let barrier = barrier.clone();
            let calls = calls.clone();
            std::thread::spawn(move || {
                barrier.wait();
                coordinator.sign_transaction(
                    [1; 32],
                    13,
                    &[n],
                    |_, plan| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, ()>(plan.to_vec())
                    },
                    |_, plan, raw| {
                        assert_eq!(plan, raw);
                        Ok(())
                    },
                )
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(Error::Conflict)))
            .count(),
        1
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
impl FaultHook for FailAt {
    fn after(&self, _: Boundary<'_>) -> std::io::Result<()> {
        let count = self.count.fetch_add(1, Ordering::SeqCst) + 1;
        if count == self.fail {
            Err(std::io::Error::other("injected crash"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn faults_at_every_authorization_boundary_preserve_the_budget_and_signature_fence() {
    // Discover the boundary count from a normal run, rather than hard-coding it.
    let (terms, blinding, buyer, _) = fixture(0);
    let dir = tempfile::tempdir().unwrap();
    let initial = open(dir.path(), &terms, 2_000_000);
    initial
        .record_intent([1; 32], &terms, &blinding, 10)
        .unwrap();
    let (context, capabilities, policy) = config(&terms, 2_000_000);
    let hook = Arc::new(FailAt {
        count: AtomicUsize::new(0),
        fail: usize::MAX,
    });
    let coordinator = Coordinator::open_with_faults(
        dir.path(),
        terms.buyer_authorization_key.clone(),
        context.clone(),
        &context,
        &capabilities,
        policy,
        hook.clone(),
    )
    .unwrap();
    coordinator
        .authorize_buyer([1; 32], 11, |_, _| Ok::<_, ()>(buyer.clone()))
        .unwrap();
    let boundaries = hook.count.load(Ordering::SeqCst);
    assert!(boundaries > 0);
    for fail in 1..=boundaries {
        let dir = tempfile::tempdir().unwrap();
        open(dir.path(), &terms, 2_000_000)
            .record_intent([1; 32], &terms, &blinding, 10)
            .unwrap();
        let (context, capabilities, policy) = config(&terms, 2_000_000);
        let hook = Arc::new(FailAt {
            count: AtomicUsize::new(0),
            fail,
        });
        let coordinator = Coordinator::open_with_faults(
            dir.path(),
            terms.buyer_authorization_key.clone(),
            context.clone(),
            &context,
            &capabilities,
            policy,
            hook,
        )
        .unwrap();
        let signed = AtomicUsize::new(0);
        assert!(coordinator
            .authorize_buyer([1; 32], 11, |_, _| {
                signed.fetch_add(1, Ordering::SeqCst);
                Ok::<_, ()>(buyer.clone())
            })
            .is_err());
        drop(coordinator);
        let restarted = open(dir.path(), &terms, 2_000_000);
        assert_eq!(
            restarted
                .ledger()
                .unwrap()
                .reserved_total(&terms.asset)
                .unwrap(),
            terms.amount
        );
        if signed.load(Ordering::SeqCst) > 0 {
            assert_ne!(restarted.diagnostics().unwrap()[0].stage, Stage::Reserved);
        }
        restarted
            .authorize_buyer([1; 32], 12, |_, _| Ok::<_, ()>(buyer.clone()))
            .unwrap();
        assert_eq!(restarted.ledger().unwrap().reservations().len(), 1);
    }
}

#[test]
fn snapshot_corruption_and_foreign_buyer_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let (terms, blinding, _, _) = fixture(0);
    let coordinator = open(dir.path(), &terms, 2_000_000);
    coordinator
        .record_intent([1; 32], &terms, &blinding, 10)
        .unwrap();
    let (context, capabilities, policy) = config(&terms, 2_000_000);
    assert!(matches!(
        Coordinator::open(
            dir.path(),
            terms.seller_authorization_key.clone(),
            context.clone(),
            &context,
            &capabilities,
            policy
        ),
        Err(Error::Storage)
    ));
    std::fs::write(dir.path().join("coordinator.json"), b"{broken").unwrap();
    assert!(matches!(coordinator.ledger(), Err(Error::Storage)));
    assert!(matches!(coordinator.diagnostics(), Err(Error::Storage)));
}

#[test]
fn a_missing_snapshot_is_not_an_empty_budget() {
    let dir = tempfile::tempdir().unwrap();
    let (terms, blinding, _, _) = fixture(0);
    let coordinator = open(dir.path(), &terms, 2_000_000);
    coordinator
        .record_intent([1; 32], &terms, &blinding, 10)
        .unwrap();
    std::fs::remove_file(dir.path().join("coordinator.json")).unwrap();
    assert!(matches!(coordinator.ledger(), Err(Error::Storage)));
    let (context, capabilities, policy) = config(&terms, 2_000_000);
    assert!(matches!(
        Coordinator::open(
            dir.path(),
            terms.buyer_authorization_key.clone(),
            context.clone(),
            &context,
            &capabilities,
            policy
        ),
        Err(Error::Storage)
    ));
    assert!(!dir.path().join("coordinator.json").exists());
}

#[test]
fn disclosure_opening_is_readable_from_durable_state_without_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, terms) = signed_coordinator(dir.path());
    drop(coordinator);
    let (expected_terms, expected_blinding, expected_buyer, expected_seller) = fixture(0);
    assert_eq!(terms, expected_terms);

    let (terms, blinding, buyer, seller) =
        erebus_coordinator::read_disclosure_opening(dir.path(), [1; 32]).unwrap();
    assert_eq!(terms, expected_terms);
    assert_eq!(blinding, expected_blinding);
    assert_eq!(buyer, expected_buyer);
    assert_eq!(seller, expected_seller);
    let commitment = commit_agreement(&terms, &blinding).unwrap();
    erebus_core::auth::verify_authorization_signature(&terms, &commitment, &blinding, &buyer)
        .unwrap();
    erebus_core::auth::verify_authorization_signature(&terms, &commitment, &blinding, &seller)
        .unwrap();
}

#[test]
fn disclosure_opening_fails_closed_without_initialized_state() {
    let dir = tempfile::tempdir().unwrap();
    let absent = dir.path().join("absent");
    assert!(matches!(
        erebus_coordinator::read_disclosure_opening(&absent, [1; 32]),
        Err(Error::Storage)
    ));
    assert!(
        !absent.exists(),
        "recovery must not create a missing state root"
    );
    assert!(matches!(
        erebus_coordinator::read_disclosure_opening(dir.path(), [1; 32]),
        Err(Error::Storage)
    ));
    let (coordinator, _) = signed_coordinator(dir.path());
    drop(coordinator);
    assert!(matches!(
        erebus_coordinator::read_disclosure_opening(dir.path(), [9; 32]),
        Err(Error::Conflict)
    ));
}

#[test]
fn disclosure_opening_rejects_changed_or_oversized_markers() {
    for case in ["prefix", "buyer", "truncated", "oversized", "directory"] {
        let dir = tempfile::tempdir().unwrap();
        let (coordinator, _) = signed_coordinator(dir.path());
        drop(coordinator);
        let path = dir.path().join("coordinator.0.tx");
        let mut marker = std::fs::read(&path).unwrap();
        match case {
            "prefix" => marker[0] ^= 1,
            "buyer" => *marker.last_mut().unwrap() ^= 1,
            "truncated" => marker.truncate(5),
            "oversized" => marker.resize(1024, 0),
            "directory" => {
                std::fs::remove_file(&path).unwrap();
                std::fs::create_dir(&path).unwrap();
            }
            _ => unreachable!(),
        }
        if case != "directory" {
            std::fs::write(&path, marker).unwrap();
        }
        assert!(
            erebus_coordinator::read_disclosure_opening(dir.path(), [1; 32]).is_err(),
            "accepted invalid {case} marker"
        );
    }
}

#[cfg(unix)]
#[test]
fn disclosure_opening_rejects_a_symlink_marker() {
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, _) = signed_coordinator(dir.path());
    drop(coordinator);
    let path = dir.path().join("coordinator.0.tx");
    let target = dir.path().join("marker.backup");
    std::fs::rename(&path, &target).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(matches!(
        erebus_coordinator::read_disclosure_opening(dir.path(), [1; 32]),
        Err(Error::Storage)
    ));
}

#[test]
fn disclosure_opening_rejects_corrupted_stored_authorizations() {
    for field in ["buyer_auth", "seller_auth"] {
        let dir = tempfile::tempdir().unwrap();
        let (coordinator, _) = signed_coordinator(dir.path());
        drop(coordinator);
        let path = dir.path().join("coordinator.json");
        let original = std::fs::read(&path).unwrap();
        let mut state: serde_json::Value = serde_json::from_slice(&original).unwrap();
        let bytes = state["intents"][0][field].as_array_mut().unwrap();
        let last = bytes.last_mut().unwrap();
        *last = serde_json::json!(last.as_u64().unwrap() ^ 1);
        let corrupt = serde_json::to_vec(&state).unwrap();
        std::fs::write(&path, &corrupt).unwrap();
        assert!(matches!(
            erebus_coordinator::read_disclosure_opening(dir.path(), [1; 32]),
            Err(Error::Storage)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), corrupt);
    }
}
