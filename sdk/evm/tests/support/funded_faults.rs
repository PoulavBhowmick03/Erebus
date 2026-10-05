//! Real-chain operation-write sweep, distinct from synthetic coordinator storage tests.

use super::*;
use erebus_journal::{Boundary, FaultHook, NoFaults, Step};

type Failure = Box<dyn std::error::Error + Send + Sync>;

struct Sweep {
    at: usize,
    steps: Mutex<Vec<Step>>,
}

impl FaultHook for Sweep {
    fn after(&self, boundary: Boundary<'_>) -> std::io::Result<()> {
        let mut steps = self.steps.lock().unwrap();
        steps.push(boundary.step);
        if steps.len() == self.at {
            Err(std::io::Error::other("funded public-bound crash"))
        } else {
            Ok(())
        }
    }
}

fn stores(
    fixture: &Fixture,
    root: &std::path::Path,
    hook: Arc<dyn FaultHook>,
) -> Result<(Coordinator, SignerJournal), Failure> {
    let context = context_for(fixture);
    let coordinator = Coordinator::open_with_faults(
        root.join("coordinator"),
        KeyBytes::new(BUYER_KEY_ADDRESS.to_vec())?,
        context.clone(),
        &context,
        &capabilities_for(fixture),
        policy_for(fixture),
        hook.clone(),
    )?;
    let signer = TransactionKey::from_bytes(&RELAYER_KEY)?.address();
    let journal = SignerJournal::open_with_faults(root.join("signer"), 31_337, signer, hook)?;
    Ok((coordinator, journal))
}

async fn drive(
    fixture: &Fixture,
    root: &std::path::Path,
    hook: Arc<dyn FaultHook>,
) -> Result<(), Failure> {
    let (coordinator, journal) = stores(fixture, root, hook)?;
    let deployment = deployment(fixture);
    let chain = EvmChain::connect(deployment.clone(), Duration::from_secs(5)).await?;
    let mut peer_deployment = deployment.clone();
    peer_deployment.rpc_url = fixture.verification_proxy.url.clone();
    let peer = EvmChain::connect(peer_deployment, Duration::from_secs(5)).await?;
    let prepared = prepared(fixture);
    let operation = prepared.operation_ref;
    let key = TransactionKey::from_bytes(&RELAYER_KEY)?;
    let time = now();
    let evidence = chain
        .finalized_deal_evidence_agreed(
            &peer,
            &prepared.deal_nullifier,
            ObservationLimits::default(),
        )
        .await?;
    if coordinator.prepared_settlement(operation).is_ok() {
        let assessment = coordinator.reconcile(operation, &evidence, time)?;
        if matches!(
            assessment.state,
            erebus_core::deal_state::DealState::PaidFinalized { .. }
        ) {
            journal.release(
                &chain
                    .verified_finalized_nonce_agreed(&peer, key.address())
                    .await?,
            )?;
            return Ok(());
        }
    }
    coordinator.record_intent(operation, &fixture.terms, &fixture.blinding, time)?;
    let commitment = commit_agreement(&fixture.terms, &fixture.blinding)?;
    let buyer = authorization(Role::Buyer, &fixture.terms, &commitment, &fixture.buyer_key);
    let seller = authorization(
        Role::Seller,
        &fixture.terms,
        &commitment,
        &fixture.seller_key,
    );
    coordinator.authorize_buyer(operation, time, |_, _| Ok::<_, Infallible>(buyer))?;
    coordinator.accept_seller(operation, &seller)?;
    coordinator.prepare(operation, time, |terms, blinding, buyer, seller| {
        EvmSettlementBackend::connect(deployment.clone(), &RELAYER_KEY)?
            .prepare(terms, blinding, buyer, seller, operation)
    })?;
    let plan = chain
        .reserve_nonce(
            &journal,
            &prepared,
            Eip1559Fees::new(2_000_000_000, 1_000_000_000)?,
            1_000_000,
        )
        .await?;
    coordinator.sign_transaction(
        operation,
        time,
        &plan.encode(),
        |prepared, bytes| {
            SigningPlan::decode(bytes)?
                .sign(&deployment, prepared, &key)
                .map(|tx| tx.raw().to_vec())
        },
        |prepared, bytes, raw| SigningPlan::decode(bytes)?.validate(&deployment, prepared, raw),
    )?;
    let replacement = SigningPlan::new(
        key.address(),
        erebus_evm::chain::TransactionParams {
            fees: Eip1559Fees::new(3_000_000_000, 1_500_000_000)?,
            ..plan.params()
        },
    )?;
    coordinator.sign_replacement(
        operation,
        time,
        &replacement.encode(),
        |previous, bytes| {
            SigningPlan::decode(bytes)?
                .sign(&deployment, previous.prepared(), &key)
                .map(|tx| tx.raw().to_vec())
        },
        |previous, bytes, raw| {
            SigningPlan::decode(previous.plan())?.validate(
                &deployment,
                previous.prepared(),
                previous.raw(),
            )?;
            SigningPlan::decode(bytes)?.validate(&deployment, previous.prepared(), raw)?;
            erebus_evm::chain::SignedTransaction::from_raw(raw)?.validate_replacement(
                &erebus_evm::chain::SignedTransaction::from_raw(previous.raw())?,
            )
        },
    )?;
    chain
        .broadcast_journaled(&coordinator, operation, time)
        .await?;
    mine_blocks(&read_only_provider(&fixture.rpc_url), 3).await;
    let evidence = chain
        .finalized_deal_evidence_agreed(
            &peer,
            &prepared.deal_nullifier,
            ObservationLimits::default(),
        )
        .await?;
    let assessment = coordinator.reconcile(operation, &evidence, time)?;
    assert!(matches!(
        assessment.state,
        erebus_core::deal_state::DealState::PaidFinalized { .. }
    ));
    journal.release(
        &chain
            .verified_finalized_nonce_agreed(&peer, key.address())
            .await?,
    )?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Anvil; exhaustive funded persistence sweep"]
async fn every_public_bound_operation_write_recovers_one_actual_payment() {
    let fixture = fixture().await;
    let reader = read_only_provider(&fixture.rpc_url);
    let signer = TransactionKey::from_bytes(&RELAYER_KEY).unwrap().address();
    let buyer_before = token_balance(&reader, fixture.token, &BUYER_KEY_ADDRESS).await;
    let mut boundaries = 0;
    let mut fail = 0;
    loop {
        let snapshot: serde_json::Value =
            reader.client().request("evm_snapshot", ()).await.unwrap();
        let block = reader
            .get_block_by_number(alloy::eips::BlockNumberOrTag::Latest)
            .await
            .unwrap()
            .unwrap();
        let _: serde_json::Value = reader
            .client()
            .request(
                "evm_setNextBlockTimestamp",
                (block.header.timestamp + fail as u64 + 1,),
            )
            .await
            .unwrap();
        mine_blocks(&reader, 1).await;
        let nonce_before = reader
            .get_transaction_count(Address::from(signer))
            .await
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        // Empty-store setup is outside the operation sweep, covered by onboarding tests.
        stores(&fixture, root.path(), Arc::new(NoFaults)).unwrap();
        let hook = Arc::new(Sweep {
            at: if fail == 0 { usize::MAX } else { fail },
            steps: Mutex::new(Vec::new()),
        });
        let result = drive(&fixture, root.path(), hook.clone()).await;
        if fail == 0 {
            result.unwrap();
            boundaries = hook.steps.lock().unwrap().len();
            assert!(boundaries > 0);
        } else {
            assert!(
                result.is_err(),
                "boundary {fail} must interrupt the operation"
            );
            assert_eq!(hook.steps.lock().unwrap().len(), fail);
            mine_blocks(&reader, 3).await;
            drive(&fixture, root.path(), Arc::new(NoFaults))
                .await
                .unwrap();
        }
        let (coordinator, journal) = stores(&fixture, root.path(), Arc::new(NoFaults)).unwrap();
        let prepared = prepared(&fixture);
        assert_eq!(
            coordinator.diagnostics().unwrap()[0].stage,
            Stage::Finalized
        );
        assert_eq!(coordinator.ledger().unwrap().reservations().len(), 1);
        assert_eq!(
            coordinator.ledger().unwrap().reservations()[0].state,
            ReservationState::Committed
        );
        assert_eq!(coordinator.diagnostics().unwrap()[0].replacements, 1);
        assert_eq!(
            token_balance(&reader, fixture.token, &RECIPIENT).await,
            AMOUNT
        );
        assert_eq!(
            token_balance(&reader, fixture.token, &FEE_RECIPIENT).await,
            FEE
        );
        assert_eq!(
            token_balance(&reader, fixture.token, &BUYER_KEY_ADDRESS).await,
            buyer_before - AMOUNT - FEE
        );
        assert_eq!(
            reader
                .get_transaction_count(Address::from(signer))
                .await
                .unwrap(),
            nonce_before + 1
        );
        let logs: serde_json::Value = reader.client().request("eth_getLogs", (serde_json::json!({
            "address": fixture.settlement,
            "fromBlock": "0x0", "toBlock": "latest",
            "topics": [format!("0x{}", hex::encode(alloy::primitives::keccak256(b"DealSettled(bytes32,bytes32,address,address,address,uint256,uint256)"))),
                format!("0x{}", hex::encode(prepared.deal_commitment.as_bytes())),
                format!("0x{}", hex::encode(prepared.deal_nullifier.as_bytes()))]
        }),)).await.unwrap();
        assert_eq!(
            logs.as_array().unwrap().len(),
            1,
            "one actual settlement event"
        );
        assert!(journal
            .resume(&deployment(&fixture), &prepared)
            .unwrap()
            .is_none());
        let reverted: bool = reader
            .client()
            .request("evm_revert", (snapshot,))
            .await
            .unwrap();
        assert!(reverted);
        println!("M6 public-bound funded boundary {fail}/{boundaries} recovered");
        fail += 1;
        if fail > boundaries {
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Anvil"]
async fn finalized_expiry_releases_spending_but_never_an_unconsumed_signer_nonce() {
    let fixture = fixture().await;
    let reader = read_only_provider(&fixture.rpc_url);
    let root = tempfile::tempdir().unwrap();
    let (coordinator, journal, prepared, _) = coordinated_setup(&fixture, root.path()).await;
    coordinator
        .reconcile(
            prepared.operation_ref,
            &erebus_core::deal_state::DealEvidence::Unknown,
            now(),
        )
        .unwrap();
    assert_eq!(
        coordinator.ledger().unwrap().reservations()[0].state,
        ReservationState::Reserved
    );
    let _: serde_json::Value = reader
        .client()
        .request("evm_setNextBlockTimestamp", (fixture.terms.expiry + 1,))
        .await
        .unwrap();
    mine_blocks(&reader, 4).await;
    let chain = EvmChain::connect(deployment(&fixture), Duration::from_secs(5))
        .await
        .unwrap();
    let evidence = chain
        .finalized_deal_evidence(&prepared.deal_nullifier, ObservationLimits::default())
        .await
        .unwrap();
    let assessment = coordinator
        .reconcile(prepared.operation_ref, &evidence, now())
        .unwrap();
    assert_eq!(
        assessment.state,
        erebus_core::deal_state::DealState::ClosedUnpaid
    );
    assert_eq!(
        coordinator.ledger().unwrap().reservations()[0].state,
        ReservationState::Released
    );
    let signer = TransactionKey::from_bytes(&RELAYER_KEY).unwrap().address();
    assert!(matches!(
        journal.release(&chain.verified_finalized_nonce(signer).await.unwrap()),
        Err(erebus_evm::chain::NonceError::NotConsumed)
    ));
    assert!(journal
        .resume(&deployment(&fixture), &prepared)
        .unwrap()
        .is_some());
    assert_eq!(token_balance(&reader, fixture.token, &RECIPIENT).await, 0);
    assert_eq!(
        token_balance(&reader, fixture.token, &FEE_RECIPIENT).await,
        0
    );
    drop(coordinator);
    let (restarted, journal) = stores(&fixture, root.path(), Arc::new(NoFaults)).unwrap();
    assert_eq!(
        restarted.diagnostics().unwrap()[0].stage,
        Stage::ClosedUnpaid
    );
    assert!(journal
        .resume(&deployment(&fixture), &prepared)
        .unwrap()
        .is_some());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires two Anvil endpoints"]
async fn internally_consistent_rpc_histories_can_disagree_about_one_deal() {
    let paid_fixture = fixture().await;
    let unpaid_fixture = fixture().await;
    // Identical deterministic deployments make both endpoints satisfy the accepted domain.
    assert_eq!(paid_fixture.settlement, unpaid_fixture.settlement);
    assert_eq!(paid_fixture.token, unpaid_fixture.token);
    let root = tempfile::tempdir().unwrap();
    let (coordinator, _journal, prepared, _) = coordinated_setup(&paid_fixture, root.path()).await;
    let paid = EvmChain::connect(deployment(&paid_fixture), Duration::from_secs(5))
        .await
        .unwrap();
    paid.broadcast_journaled(&coordinator, prepared.operation_ref, now())
        .await
        .unwrap();
    mine_blocks(&read_only_provider(&paid_fixture.rpc_url), 3).await;
    let paid_evidence = paid
        .finalized_deal_evidence(&prepared.deal_nullifier, ObservationLimits::default())
        .await
        .unwrap();

    let reader = read_only_provider(&unpaid_fixture.rpc_url);
    let _: serde_json::Value = reader
        .client()
        .request(
            "evm_setNextBlockTimestamp",
            (paid_fixture.terms.expiry + 1,),
        )
        .await
        .unwrap();
    mine_blocks(&reader, 4).await;
    let mut other_deployment = deployment(&paid_fixture);
    other_deployment.rpc_url = unpaid_fixture.rpc_url.clone();
    let unpaid = EvmChain::connect(other_deployment, Duration::from_secs(5))
        .await
        .unwrap();
    let unpaid_evidence = unpaid
        .finalized_deal_evidence(&prepared.deal_nullifier, ObservationLimits::default())
        .await
        .unwrap();
    let (_, revisions) = coordinator.recorded_deal(prepared.operation_ref).unwrap();
    // This demonstrates the single-provider trust limit, not consensus proof.
    // Each endpoint passes its own canonical/state checks but yields a conflicting outcome.
    let paid_state = erebus_core::deal_state::assess_deal(&revisions, &paid_evidence);
    let unpaid_state = erebus_core::deal_state::assess_deal(&revisions, &unpaid_evidence);
    assert!(matches!(
        paid_state.state,
        erebus_core::deal_state::DealState::PaidFinalized { .. }
    ));
    assert_eq!(
        unpaid_state.state,
        erebus_core::deal_state::DealState::ClosedUnpaid
    );
    assert_ne!(paid_state.state, unpaid_state.state);
    assert!(paid
        .finalized_deal_evidence_agreed(
            &unpaid,
            &prepared.deal_nullifier,
            ObservationLimits::default()
        )
        .await
        .is_err());
    assert!(paid
        .verified_finalized_nonce_agreed(
            &unpaid,
            TransactionKey::from_bytes(&RELAYER_KEY).unwrap().address()
        )
        .await
        .is_err());
    let paid_history =
        erebus_evm::chain::ObservationJournal::open(root.path().join("paid-history")).unwrap();
    let unpaid_history =
        erebus_evm::chain::ObservationJournal::open(root.path().join("unpaid-history")).unwrap();
    assert!(paid
        .finalized_deal_evidence_resumable_agreed(
            &paid_history,
            &unpaid,
            &unpaid_history,
            &prepared.deal_nullifier,
            ObservationLimits::default(),
        )
        .await
        .is_err());
    assert_eq!(
        coordinator.ledger().unwrap().reservations()[0].state,
        ReservationState::Reserved,
        "diagnostic comparison must not apply either conflicting result"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Anvil and the local RPC proxy"]
async fn optional_two_provider_reads_require_matching_anchors_and_hold_on_timeout() {
    let fixture = fixture().await;
    let root = tempfile::tempdir().unwrap();
    let (coordinator, journal, prepared, _) = coordinated_setup(&fixture, root.path()).await;
    let chain = EvmChain::connect(deployment(&fixture), Duration::from_secs(5))
        .await
        .unwrap();
    chain
        .broadcast_journaled(&coordinator, prepared.operation_ref, now())
        .await
        .unwrap();
    mine_blocks(&read_only_provider(&fixture.rpc_url), 3).await;
    // This proxy verifies the mechanics, not provider independence: it shares Anvil upstream.
    let proxy =
        FaultProxy::start(fixture.rpc_url.strip_prefix("http://").unwrap().to_owned()).await;
    let peer = EvmChain::connect(
        proxied_deployment(&fixture, &proxy),
        Duration::from_millis(200),
    )
    .await
    .unwrap();
    let history =
        erebus_evm::chain::ObservationJournal::open(root.path().join("primary-history")).unwrap();
    let peer_history =
        erebus_evm::chain::ObservationJournal::open(root.path().join("peer-history")).unwrap();
    assert!(chain
        .finalized_deal_evidence_resumable_agreed(
            &history,
            &peer,
            &history,
            &prepared.deal_nullifier,
            ObservationLimits::default(),
        )
        .await
        .is_err());
    assert!(chain
        .finalized_deal_evidence_agreed(
            &chain,
            &prepared.deal_nullifier,
            ObservationLimits::default()
        )
        .await
        .is_err());
    proxy.set(Fault::DelayFinalizedResponse);
    assert!(chain
        .finalized_deal_evidence_resumable_agreed(
            &history,
            &peer,
            &peer_history,
            &prepared.deal_nullifier,
            ObservationLimits::default(),
        )
        .await
        .is_err());
    assert!(chain
        .finalized_deal_evidence_agreed(
            &peer,
            &prepared.deal_nullifier,
            ObservationLimits::default()
        )
        .await
        .is_err());
    assert_eq!(
        coordinator.ledger().unwrap().reservations()[0].state,
        ReservationState::Reserved
    );
    assert!(journal
        .resume(&deployment(&fixture), &prepared)
        .unwrap()
        .is_some());
    proxy.set(Fault::Pass);
    let mut matched = false;
    for _ in 0..100 {
        match chain
            .finalized_deal_evidence_resumable_agreed(
                &history,
                &peer,
                &peer_history,
                &prepared.deal_nullifier,
                ObservationLimits {
                    log_block_range: 1,
                    max_log_queries: 1,
                    max_ancestry: 1,
                    max_concurrent_queries: 1,
                },
            )
            .await
            .unwrap()
        {
            erebus_evm::chain::HistoricalObservation::Pending { .. } => {
                assert_eq!(
                    coordinator.ledger().unwrap().reservations()[0].state,
                    ReservationState::Reserved
                );
            }
            erebus_evm::chain::HistoricalObservation::Complete { .. } => {
                matched = true;
                break;
            }
        }
    }
    assert!(matched);
    let evidence = chain
        .finalized_deal_evidence_agreed(
            &peer,
            &prepared.deal_nullifier,
            ObservationLimits::default(),
        )
        .await
        .unwrap();
    coordinator
        .reconcile(prepared.operation_ref, &evidence, now())
        .unwrap();
    let signer = TransactionKey::from_bytes(&RELAYER_KEY).unwrap().address();
    journal
        .release(
            &chain
                .verified_finalized_nonce_agreed(&peer, signer)
                .await
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        coordinator.ledger().unwrap().reservations()[0].state,
        ReservationState::Committed
    );
    assert!(journal
        .resume(&deployment(&fixture), &prepared)
        .unwrap()
        .is_none());
}
