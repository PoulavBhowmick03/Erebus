//! Characterization of reconciliation and resume over historical journal records
//! (Metropolis M6, slice 1).
//!
//! `Finding` (from `reconcile`) and `ResumeOutcome` (from `resume_operation`) cross the
//! `erebus-cli` seam as JSON, so their exact shape is an external contract. These tests pin
//! that JSON, byte for byte, over records written by every historical journal schema, against
//! a local mock node. They pin current behaviour, not intended behaviour: anything that looks
//! wrong is marked `CHARACTERIZATION: current behavior, flagged for review`.
//!
//! The mock chain (see `journal_support::mock_chain`): account nonce `0x6`; receipts only for
//! the accepted (block 120), committed (121) and reverted (122) fixtures, everything else
//! "not found"; block `n` has timestamp `1_700_000_000 + n`, deliberately not the
//! `1_787_000_000 + n` the v3/v4 fixtures recorded, so the goldens show which source was used.
//!
//! Goldens live in `tests/fixtures/journal/golden/`. Regenerate after an intended change with
//! `EREBUS_BLESS_JOURNAL_GOLDENS=1` and review the diff.

mod journal_support;

use std::sync::{Arc, Mutex};

use erebus_sdk::client::{Client, ClientConfig};
use erebus_sdk::journal::{OperationRecord, OperationStage};
use erebus_sdk::operation::OperationId;
use erebus_sdk::reconcile::{reconcile, Finding, NextAction, Outcome};
use erebus_sdk::resume::{plan, ResumeOutcome, ResumePlan};
use erebus_sdk::rpc::StarknetRpc;
use erebus_sdk::wire::WireVersion;
use journal_support::*;
use serde_json::{json, Map, Value};
use starknet_types_core::felt::Felt;

fn no_landings() -> Arc<Mutex<Vec<String>>> {
    Arc::default()
}

async fn reconcile_records(records: &[OperationRecord]) -> (Vec<Finding>, Vec<Value>) {
    let node = MockNode::start(mock_chain(500, no_landings()));
    let rpc = StarknetRpc::new(node.url.clone()).expect("rpc");
    let findings = reconcile(&rpc, ACCOUNT, records).await.expect("reconcile");
    let calls = node.calls();
    (findings, calls)
}

fn methods(calls: &[Value]) -> Vec<&str> {
    calls
        .iter()
        .map(|call| call[0].as_str().expect("method"))
        .collect()
}

fn client(root: &std::path::Path, rpc_url: String) -> Client {
    // Key files are absent unless a test writes one: reconcile and resubmission never read
    // them.
    Client::new(ClientConfig {
        rpc_url,
        prover_url: "http://127.0.0.1:9".to_owned(),
        pool_address: POOL,
        chain_id: CHAIN,
        account_address: ACCOUNT,
        pool_key_file: root.join("pool.key"),
        account_key_file: root.join("account.key"),
        state_dir: root.to_path_buf(),
        token: TOKEN,
        new_channel_wire_version: WireVersion::V3,
    })
    .expect("client")
}

fn plan_json(decision: &ResumePlan) -> Value {
    match decision {
        ResumePlan::Report(outcome) => {
            json!({"report": serde_json::to_value(outcome).expect("outcome")})
        }
        ResumePlan::Resubmit {
            attempt_index,
            transaction_hash,
        } => json!({"resubmit": {
            "attempt_index": attempt_index,
            "transaction_hash": format!("{transaction_hash:#x}"),
        }}),
    }
}

// ---- Wire names --------------------------------------------------------------------------

#[test]
fn outcome_and_next_action_wire_names() {
    let outcomes = [
        Outcome::NoEffect,
        Outcome::Effect,
        Outcome::Reverted,
        Outcome::Pending,
        Outcome::Unknown,
    ]
    .map(|o| serde_json::to_value(o).expect("outcome"));
    assert_eq!(
        Value::Array(outcomes.to_vec()),
        json!(["no_effect", "effect", "reverted", "pending", "unknown"])
    );
    let actions = [
        NextAction::None,
        NextAction::SafeToRetry,
        NextAction::CommitLocalState,
        NextAction::CommitJournal,
        NextAction::Wait,
        NextAction::OperatorAttention,
    ]
    .map(|a| serde_json::to_value(a).expect("action"));
    assert_eq!(
        Value::Array(actions.to_vec()),
        json!([
            "none",
            "safe_to_retry",
            "commit_local_state",
            "commit_journal",
            "wait",
            "operator_attention"
        ])
    );
}

/// Every `ResumeOutcome` variant as it crosses the CLI seam: internally tagged by `result`.
#[test]
fn resume_outcome_wire_shapes() {
    let hash = Felt::from_hex_unchecked("0x400a01beef");
    let variants = [
        ResumeOutcome::AlreadyComplete {
            transaction_hash: Some(hash),
        },
        ResumeOutcome::AlreadyComplete {
            transaction_hash: None,
        },
        ResumeOutcome::LocalStateBehind {
            transaction_hash: hash,
        },
        ResumeOutcome::Resubmitted {
            transaction_hash: hash,
        },
        ResumeOutcome::RecoveredProof {
            operation_result: json!({"tx_hash": "0x400a01beef"}),
        },
        ResumeOutcome::Rebuilt {
            operation_result: json!(HANDLE),
        },
        ResumeOutcome::RebuildRequired {
            reason: "why".to_owned(),
        },
        ResumeOutcome::ReconciliationRequired {
            reason: "why".to_owned(),
        },
    ];
    let encoded: Vec<Value> = variants
        .iter()
        .map(|variant| serde_json::to_value(variant).expect("outcome"))
        .collect();
    assert_golden_json("resume/outcome-variants.json", &Value::Array(encoded));
}

// ---- reconcile() over every fixture ------------------------------------------------------

/// The module-level classifier over every historical record, sorted by id, and every RPC
/// call it made, in order.
#[tokio::test]
async fn reconcile_findings_over_every_fixture() {
    let installed = install("reconcile-all", &SETS);
    let records = installed.sorted_records();
    let (findings, calls) = reconcile_records(&records).await;
    assert_eq!(findings.len(), records.len());
    assert_golden_json(
        "reconcile/findings.json",
        &serde_json::to_value(&findings).expect("findings"),
    );
    assert_golden_json("reconcile/rpc-calls.json", &Value::Array(calls));
}

/// Where `accepted_at` comes from, per schema version.
///
/// Versions 1 and 2 wrote the local wall clock, so reconciliation ignores the recorded value
/// and re-reads the block: from the durable receipt's block when there is one (v2), else by
/// fetching the receipt (v1). From version 3 on the recorded value is trusted with no RPC.
#[tokio::test]
async fn acceptance_timestamp_source_depends_on_the_record_version() {
    // (set, code, accepted_at reported, RPC methods after the nonce read)
    let cases: [(FixtureSet, u8, Option<u64>, &[&str]); 15] = [
        (
            V1_B784F3F,
            ACCEPTED,
            Some(1_700_000_120),
            &[
                "starknet_getTransactionReceipt",
                "starknet_getBlockWithTxHashes",
            ],
        ),
        (
            V1_7B7B4C8,
            ACCEPTED,
            Some(1_700_000_120),
            &[
                "starknet_getTransactionReceipt",
                "starknet_getBlockWithTxHashes",
            ],
        ),
        (
            V1_448146E,
            ACCEPTED,
            Some(1_700_000_120),
            &[
                "starknet_getTransactionReceipt",
                "starknet_getBlockWithTxHashes",
            ],
        ),
        (
            V2_B1FE0E3,
            ACCEPTED,
            Some(1_700_000_120),
            &["starknet_getBlockWithTxHashes"],
        ),
        (V3_D3DAB59, ACCEPTED, Some(1_787_000_120), &[]),
        (V4_93593ED, ACCEPTED, Some(1_787_000_120), &[]),
        (
            V1_B784F3F,
            COMMITTED,
            Some(1_700_000_121),
            &[
                "starknet_getTransactionReceipt",
                "starknet_getBlockWithTxHashes",
            ],
        ),
        (
            V1_7B7B4C8,
            COMMITTED,
            Some(1_700_000_121),
            &[
                "starknet_getTransactionReceipt",
                "starknet_getBlockWithTxHashes",
            ],
        ),
        (
            V1_448146E,
            COMMITTED,
            Some(1_700_000_121),
            &[
                "starknet_getTransactionReceipt",
                "starknet_getBlockWithTxHashes",
            ],
        ),
        (
            V2_B1FE0E3,
            COMMITTED,
            Some(1_700_000_121),
            &["starknet_getBlockWithTxHashes"],
        ),
        (V3_D3DAB59, COMMITTED, Some(1_787_000_121), &[]),
        (V4_93593ED, COMMITTED, Some(1_787_000_121), &[]),
        // A no-chain commit has no transaction and no block: no timestamp, no RPC.
        (V2_B1FE0E3, NOOP_COMMITTED, None, &[]),
        (V3_D3DAB59, NOOP_COMMITTED, None, &[]),
        (V4_93593ED, NOOP_COMMITTED, None, &[]),
    ];
    for (set, code, accepted_at, after_nonce) in cases {
        let installed = install("accepted-at", &[set]);
        let record = installed
            .sorted_records()
            .into_iter()
            .find(|record| record.operation_id == set.id(code))
            .expect("record");
        let (findings, calls) = reconcile_records(std::slice::from_ref(&record)).await;
        let label = format!("{} {code:#04x}", set.dir);
        assert_eq!(findings[0].outcome, Outcome::Effect, "{label}");
        assert_eq!(findings[0].accepted_at, accepted_at, "{label}");
        let mut expected = vec!["starknet_getNonce"];
        expected.extend_from_slice(after_nonce);
        assert_eq!(methods(&calls), expected, "{label}");
    }
}

/// CHARACTERIZATION: current behavior, flagged for review: `record_result` bumps a legacy
/// record to version 4 (journal.rs:770) and leaves its wall-clock `accepted_at` in place, so
/// reconciliation (reconcile.rs:190-194) then reports the local clock as the block time,
/// with no RPC. Before the bump the same record reported the block's timestamp.
#[tokio::test]
async fn a_recorded_result_makes_a_legacy_wall_clock_accepted_at_authoritative() {
    for (set, wall_clock) in [(V1_448146E, 1_787_450_429), (V2_B1FE0E3, 1_787_529_640)] {
        let installed = install("launder", &[set]);
        let id = set.id(ACCEPTED);
        {
            let mut lease = installed.journal.lock(&id).expect("lock").expect("record");
            lease
                .record_result(json!("result"), 1_900_000_000)
                .expect("result");
            lease
                .advance(OperationStage::Committed, 1_900_000_001)
                .expect("commit");
        }
        let records = installed.sorted_records();
        let record = records
            .iter()
            .find(|record| record.operation_id == id)
            .expect("record");
        assert_eq!(record.version, 4);
        let (findings, calls) = reconcile_records(std::slice::from_ref(record)).await;
        assert_eq!(findings[0].accepted_at, Some(wall_clock), "{}", set.dir);
        assert_ne!(Some(mock_block_timestamp(120)), findings[0].accepted_at);
        assert_eq!(methods(&calls), ["starknet_getNonce"], "{}", set.dir);
    }
}

/// Only the latest attempt is classified.
///
/// CHARACTERIZATION: current behavior, flagged for review: a record whose earlier attempt was
/// submitted (and so may have landed, `may_have_landed() == true`) but whose latest attempt
/// was only just reopened is reported as "no transaction was ever signed", `no_effect`,
/// `safe_to_retry`, with no `transaction_hash`, and the earlier attempt's hash is never looked
/// up. Current code only restarts after proving the old attempt dead, but the journal API of
/// b784f3f through f9bda51 allowed a restart from any non-terminal stage (no shipped client
/// called it before 448146e). reconcile.rs:115-124, resume.rs:141-149.
#[tokio::test]
async fn only_the_latest_attempt_is_classified() {
    for set in SETS {
        let installed = install("latest-attempt", &[set]);
        let records = installed.sorted_records();

        let fresh = records
            .iter()
            .find(|record| record.operation_id == set.id(RESTARTED_FRESH))
            .expect("record");
        assert!(fresh.may_have_landed());
        assert!(fresh.attempts[0].transaction_hash.is_some());
        let (findings, calls) = reconcile_records(std::slice::from_ref(fresh)).await;
        assert_eq!(findings[0].outcome, Outcome::NoEffect, "{}", set.dir);
        assert_eq!(
            findings[0].next_action,
            NextAction::SafeToRetry,
            "{}",
            set.dir
        );
        assert_eq!(findings[0].transaction_hash, None, "{}", set.dir);
        assert_eq!(
            findings[0].reason,
            "no transaction was ever signed, so nothing reached the chain"
        );
        assert_eq!(methods(&calls), ["starknet_getNonce"], "{}", set.dir);

        // With two signed attempts only the second hash is asked about.
        let in_flight = records
            .iter()
            .find(|record| record.operation_id == set.id(RESTARTED_IN_FLIGHT))
            .expect("record");
        let (findings, calls) = reconcile_records(std::slice::from_ref(in_flight)).await;
        let second = format!("{:#x}", set.tx(RESTARTED_IN_FLIGHT, 1));
        assert_eq!(
            findings[0].transaction_hash,
            Some(set.tx(RESTARTED_IN_FLIGHT, 1))
        );
        assert_eq!(
            calls,
            [
                json!(["starknet_getNonce", {"block_id":"latest","contract_address":"0xacc"}]),
                json!(["starknet_getTransactionReceipt", {"transaction_hash": second}]),
            ],
            "{}",
            set.dir
        );
    }
}

// ---- Client::reconcile (what the CLI serializes) -----------------------------------------

/// What `erebus-cli reconcile` returns over every historical record, sorted by id. This adds
/// the client's local-state refinement on top of the classifier.
///
/// CHARACTERIZATION: current behavior, flagged for review: every schema-v1 record with an
/// effect (accepted or committed) refines to `operator_attention` ("the attempt has no durable
/// completion plan", client.rs:418-425), and `begin_operation` refuses new writes while any
/// finding asks for an operator (client.rs:310-336); see the next test.
#[tokio::test]
async fn client_reconcile_findings_over_every_fixture() {
    let installed = install("client-reconcile", &SETS);
    let node = MockNode::start(mock_chain(500, no_landings()));
    let mut findings = client(&installed.root, node.url.clone())
        .reconcile()
        .await
        .expect("reconcile");
    findings.sort_by(|a, b| a.operation_id.as_str().cmp(b.operation_id.as_str()));
    assert_eq!(findings.len(), 75);
    assert!(
        !node
            .methods()
            .iter()
            .any(|m| m == "starknet_addInvokeTransaction"),
        "reconcile submitted something"
    );
    assert_golden_json(
        "reconcile/client-findings.json",
        &serde_json::to_value(&findings).expect("findings"),
    );
}

/// CHARACTERIZATION: current behavior, flagged for review: a finished schema-v1 operation
/// refines to `operator_attention` (no completion plan), and `begin_operation` refuses every
/// new write while one exists (client.rs:310-336). Nothing in `src/` calls `prune`, so an
/// identity whose journal holds a committed v1 record cannot write again until the record is
/// removed by hand.
#[tokio::test]
async fn a_committed_schema_v1_record_blocks_every_new_write() {
    let installed = install_codes("v1-blocks", V1_B784F3F, &[COMMITTED]);
    // Obviously fake, written to the temporary state directory only, like tests/fault_matrix.rs:
    // `approve_pool` checks the account key file before it claims anything.
    std::fs::write(installed.root.join("account.key"), "0xdef\n").expect("fake key");
    let node = MockNode::start(mock_chain(500, no_landings()));

    let error = client(&installed.root, node.url.clone())
        .approve_pool(&fixture_id(0x77, 0x01), 5)
        .await
        .expect_err("blocked");

    assert_eq!(
        error.to_string(),
        format!(
            "operation {} requires recovery at Committed (OperatorAttention) before a new write",
            V1_B784F3F.id(COMMITTED)
        )
    );
    assert!(
        !node
            .methods()
            .iter()
            .any(|m| m == "starknet_addInvokeTransaction"),
        "a blocked write submitted something"
    );
}

// ---- resume::plan over every fixture -----------------------------------------------------

/// The resume decision for every historical record at two heads: 500 (every proof window
/// still open) and 700 (past the 550 and 650 windows the fixtures recorded).
#[tokio::test]
async fn resume_plans_over_every_fixture() {
    let installed = install("plans", &SETS);
    let records = installed.sorted_records();
    let (findings, _) = reconcile_records(&records).await;

    let mut by_head = Map::new();
    for head in [500u64, 700] {
        let mut plans = Map::new();
        for finding in &findings {
            let lease = installed
                .journal
                .lock(&finding.operation_id)
                .expect("lock")
                .expect("record");
            let decision = plan(
                &lease,
                finding.outcome,
                finding.next_action,
                &finding.reason,
                head,
            );
            plans.insert(finding.operation_id.to_string(), plan_json(&decision));
        }
        by_head.insert(head.to_string(), Value::Object(plans));
    }
    assert_golden_json("resume/plans.json", &Value::Object(by_head));
}

// ---- Client::resume_operation (what the CLI serializes) ----------------------------------

/// Masks the wall-clock `updated_at` the client stamps, so the record is deterministic.
fn stable_record(installed: &Installed, id: &OperationId) -> Value {
    let mut value: Value =
        serde_json::from_slice(&std::fs::read(installed.record_path(id)).expect("record"))
            .expect("json");
    for attempt in value["attempts"].as_array_mut().expect("attempts") {
        attempt["updated_at"] = json!("<masked>");
    }
    value
}

/// `resume_operation` on a selection of historical records, with the outcome (or error text),
/// the RPC methods it used, and the record it left behind.
///
/// CHARACTERIZATION: current behavior, flagged for review: `v1-448146e restarted_in_flight`
/// resubmits the stored transaction (a chain effect) and then fails with "the attempt has no
/// durable completion plan" (client.rs:576 via client.rs:399-407), leaving a v1 record at
/// `accepted` with no result that reconciles to `operator_attention` from then on.
#[tokio::test]
async fn client_resume_operation_over_fixtures() {
    // (label, set, code, hash the mock node returns on resubmission)
    let cases: [(&str, FixtureSet, u8, Option<Felt>); 8] = [
        ("v4-93593ed committed", V4_93593ED, COMMITTED, None),
        (
            "v4-93593ed needs_attention",
            V4_93593ED,
            NEEDS_ATTENTION,
            None,
        ),
        (
            "v4-93593ed restarted_in_flight",
            V4_93593ED,
            RESTARTED_IN_FLIGHT,
            Some(V4_93593ED.tx(RESTARTED_IN_FLIGHT, 1)),
        ),
        (
            "v1-448146e restarted_in_flight",
            V1_448146E,
            RESTARTED_IN_FLIGHT,
            Some(V1_448146E.tx(RESTARTED_IN_FLIGHT, 1)),
        ),
        ("v1-448146e submitted", V1_448146E, SUBMITTED, None),
        ("v1-b784f3f submitted", V1_B784F3F, SUBMITTED, None),
        ("v1-b784f3f committed", V1_B784F3F, COMMITTED, None),
        ("v1-7b7b4c8 prepared", V1_7B7B4C8, PREPARED, None),
    ];

    let mut golden = Map::new();
    for (label, set, code, resubmitted) in cases {
        let installed = install("resume", &[set]);
        let id = set.id(code);
        let original = std::fs::read(installed.record_path(&id)).expect("record");

        let landed = no_landings();
        let base = mock_chain(500, Arc::clone(&landed));
        let node = MockNode::start(Box::new(move |method, params| {
            if method == "starknet_addInvokeTransaction" {
                let hash = resubmitted.ok_or((-32000, "unexpected submission".to_owned()))?;
                let hash = format!("{hash:#x}");
                landed.lock().expect("landed").push(hash.clone());
                return Ok(json!({ "transaction_hash": hash }));
            }
            base(method, params)
        }));

        let result = client(&installed.root, node.url.clone())
            .resume_operation(&id)
            .await;
        let outcome = match result {
            Ok(outcome) => json!({"ok": serde_json::to_value(outcome).expect("outcome")}),
            Err(error) => json!({"err": error.to_string()}),
        };
        let unchanged = std::fs::read(installed.record_path(&id)).expect("record") == original;
        golden.insert(
            label.to_owned(),
            json!({
                "outcome": outcome,
                "rpc_methods": node.methods(),
                "record_unchanged": unchanged,
                "record_after": if unchanged { Value::Null } else { stable_record(&installed, &id) },
            }),
        );
    }
    assert_golden_json("resume/client-outcomes.json", &Value::Object(golden));
}
