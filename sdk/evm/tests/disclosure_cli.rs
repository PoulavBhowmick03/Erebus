//! Fresh disclosure command processes, without RPC, prover, or participant services.

use std::fs::OpenOptions;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use erebus_coordinator::Coordinator;
use erebus_core::{
    auth::{authorization_digest, Authorization, Role},
    commitment::{commit_agreement, CommitmentBlinding},
    domain::DeploymentDomain,
    ids::{AddressBytes, AssetId, BaseUnits, ChainNamespace, KeyBytes, SignatureBytes},
    policy::SpendingPolicy,
    service::ServiceRecord,
    settlement::{BackendCapabilities, SettlementContext},
    terms::{AgreementTerms, FeePolicy, Guarantee, GuaranteeSet, SettlementMode},
};
use erebus_transport::{
    disclosure::{DisclosureGrant, SelectedAgreement, MAX_DISCLOSURE_BYTES},
    hashing::TRANSCRIPT_HASH_VERSION,
    identity::{AuthorizationIdentity, DisclosureIdentity},
    message::{Message, MessageType},
    store::{FileTranscriptStore, TranscriptStore},
    transcript::Transcript,
};
use serde_json::{json, Value};

const GRANT_EXPIRY: u64 = 4_102_444_800;

fn binary() -> PathBuf {
    std::env::var_os("EREBUS_TEST_DISCLOSURE_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_erebus-disclosure")))
}

fn run_bytes(root: &Path, bytes: &[u8]) -> Output {
    let mut child = Command::new(binary())
        .current_dir(root)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    child.wait_with_output().unwrap()
}

fn run(root: &Path, request: Value) -> (Output, Value) {
    let output = run_bytes(root, &serde_json::to_vec(&request).unwrap());
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response = serde_json::from_slice(&output.stdout).unwrap();
    (output, response)
}

fn private_file(path: &Path, bytes: &[u8]) {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).unwrap();
    file.write_all(bytes).unwrap();
}

fn fixture() -> SelectedAgreement {
    let buyer = AuthorizationIdentity::from_bytes(&[1; 32]).unwrap();
    let seller = AuthorizationIdentity::from_bytes(&[2; 32]).unwrap();
    let deal_id = [7; 16];
    let message = Message::new(
        [9; 32],
        deal_id,
        1,
        Role::Buyer,
        1,
        [0; 32],
        MessageType::Offer,
        b"private commercial terms".to_vec(),
    )
    .unwrap();
    let mut transcript = Transcript::new(deal_id, TRANSCRIPT_HASH_VERSION).unwrap();
    transcript.append(&message).unwrap();
    let terms = AgreementTerms {
        protocol_version: 1,
        suite_id: 1,
        domain: DeploymentDomain {
            namespace: ChainNamespace::parse("eip155:31337").unwrap(),
            settlement_contract: Some(AddressBytes::new(vec![0x11; 20]).unwrap()),
            pool: None,
            verifier_version: 1,
        },
        deal_id,
        revision: 1,
        transcript_root: transcript.root().unwrap(),
        buyer_authorization_key: KeyBytes::new(buyer.address().to_vec()).unwrap(),
        seller_authorization_key: KeyBytes::new(seller.address().to_vec()).unwrap(),
        payment_recipient: KeyBytes::new(seller.address().to_vec()).unwrap(),
        asset: AssetId::parse("eip155:31337/erc20:0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .unwrap(),
        amount: BaseUnits::new(70),
        expiry: GRANT_EXPIRY,
        fee_policy: FeePolicy::none(),
        settlement_mode: SettlementMode::PublicBound,
        required_guarantees: GuaranteeSet::from_guarantee(Guarantee::AgreementBoundSettlement),
        settlement_nonce: [6; 32],
        service: ServiceRecord {
            resource: "private.dataset".into(),
            quantity: BaseUnits::new(1),
            unit: "request".into(),
            access_recipient: KeyBytes::new(buyer.address().to_vec()).unwrap(),
            delivery_deadline: GRANT_EXPIRY,
            fulfillment_method: "http-access".into(),
            fulfillment_digest: [0; 32],
        },
    };
    let blinding = CommitmentBlinding::from_bytes([5; 32]);
    let commitment = commit_agreement(&terms, &blinding).unwrap();
    let authorize = |role, identity: &AuthorizationIdentity| Authorization {
        role,
        suite_id: 1,
        commitment,
        signature: SignatureBytes::new(
            identity
                .sign_digest(&authorization_digest(&terms.domain, role, &commitment, 1).unwrap())
                .to_vec(),
        )
        .unwrap(),
    };
    SelectedAgreement {
        buyer: authorize(Role::Buyer, &buyer),
        seller: authorize(Role::Seller, &seller),
        terms,
        blinding,
        transcript_hash_version: TRANSCRIPT_HASH_VERSION,
        messages: vec![message],
    }
}

fn export_request(public: &str) -> Value {
    json!({
        "method": "export",
        "evidence_file": "selected.evidence",
        "issuer_key_file": "issuer.key",
        "recipient_public_key": public,
        "grant_file": "deal.grant",
        "expires_at": GRANT_EXPIRY,
    })
}

fn verify_request() -> Value {
    json!({
        "method": "verify_agreement",
        "grant_file": "deal.grant",
        "key_file": "auditor.key",
        "expected_issuer": format!("0x{}", hex::encode(AuthorizationIdentity::from_bytes(&[2; 32]).unwrap().address())),
    })
}

fn exported(root: &Path) -> String {
    let (output, response) = run(root, json!({"method": "keygen", "key_file": "auditor.key"}));
    assert!(output.status.success());
    let public = response["recipient_public_key"]
        .as_str()
        .unwrap()
        .to_owned();
    private_file(
        &root.join("selected.evidence"),
        &fixture().encode().unwrap(),
    );
    private_file(&root.join("issuer.key"), &[2; 32]);
    let (output, _) = run(root, export_request(&public));
    assert!(output.status.success());
    public
}

#[test]
fn help_needs_no_configuration_or_stdin() {
    let output = Command::new(binary())
        .env_clear()
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("does not establish payment"));
    assert!(output.stderr.is_empty());
}

#[test]
fn keygen_preserves_existing_keys_and_key_info_recovers_the_public_key() {
    let dir = tempfile::tempdir().unwrap();
    let (output, generated) = run(
        dir.path(),
        json!({"method": "keygen", "key_file": "auditor.key"}),
    );
    assert!(output.status.success());
    let secret = std::fs::read(dir.path().join("auditor.key")).unwrap();
    assert_eq!(secret.len(), 32);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&hex::encode(&secret)));
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(dir.path().join("auditor.key"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let (output, recovered) = run(
        dir.path(),
        json!({"method": "key_info", "key_file": "auditor.key"}),
    );
    assert!(output.status.success());
    assert_eq!(recovered, generated);
    assert!(!run(
        dir.path(),
        json!({"method": "keygen", "key_file": "auditor.key"})
    )
    .0
    .status
    .success());
    assert_eq!(
        std::fs::read(dir.path().join("auditor.key")).unwrap(),
        secret
    );
}

#[test]
fn independent_recipient_verifies_from_backup_without_the_issuer_or_selected_archive() {
    let dir = tempfile::tempdir().unwrap();
    let public = exported(dir.path());
    let encoded = std::fs::read(dir.path().join("deal.grant")).unwrap();
    assert!(!encoded
        .windows(b"private commercial terms".len())
        .any(|window| window == b"private commercial terms"));
    assert!(!run(dir.path(), export_request(&public)).0.status.success());
    assert_eq!(
        std::fs::read(dir.path().join("deal.grant")).unwrap(),
        encoded
    );
    std::fs::remove_file(dir.path().join("selected.evidence")).unwrap();
    std::fs::remove_file(dir.path().join("issuer.key")).unwrap();
    let (output, verified) = run(dir.path(), verify_request());
    assert!(output.status.success());
    assert_eq!(verified["agreement_verified"], true);
    assert_eq!(verified["payment_verified"], false);
    assert_eq!(verified["delivery_verified"], false);
    assert_eq!(
        verified["deal_commitment"],
        fixture().buyer.commitment.to_hex()
    );
    for secret in [
        "private.dataset",
        "private commercial terms",
        &hex::encode([2; 32]),
        &hex::encode([5; 32]),
    ] {
        assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    }
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(dir.path().join("deal.grant"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn missing_grant_is_reissued_only_from_retained_valid_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let public = exported(dir.path());
    let (_, before) = run(dir.path(), verify_request());
    std::fs::remove_file(dir.path().join("deal.grant")).unwrap();
    assert!(!run(dir.path(), verify_request()).0.status.success());
    assert!(run(dir.path(), export_request(&public)).0.status.success());
    assert_eq!(run(dir.path(), verify_request()).1, before);
    std::fs::remove_file(dir.path().join("deal.grant")).unwrap();
    let mut altered = fixture();
    altered.terms.amount = BaseUnits::new(71);
    std::fs::remove_file(dir.path().join("selected.evidence")).unwrap();
    private_file(
        &dir.path().join("selected.evidence"),
        &altered.encode().unwrap(),
    );
    assert!(!run(dir.path(), export_request(&public)).0.status.success());
    assert!(!dir.path().join("deal.grant").exists());
}

#[test]
fn wrong_recipient_issuer_and_expiry_are_rejected_offline() {
    let dir = tempfile::tempdir().unwrap();
    exported(dir.path());
    DisclosureIdentity::generate_and_store(dir.path().join("other.key")).unwrap();
    let mut wrong_key = verify_request();
    wrong_key["key_file"] = json!("other.key");
    assert!(!run(dir.path(), wrong_key).0.status.success());
    let mut wrong_issuer = verify_request();
    wrong_issuer["expected_issuer"] = json!(format!("0x{}", "99".repeat(20)));
    assert!(!run(dir.path(), wrong_issuer).0.status.success());
    let recipient = DisclosureIdentity::load(dir.path().join("auditor.key")).unwrap();
    let issuer = AuthorizationIdentity::from_bytes(&[2; 32]).unwrap();
    let expired =
        DisclosureGrant::seal(&fixture(), &issuer, recipient.public_key(), 101, 100).unwrap();
    std::fs::remove_file(dir.path().join("deal.grant")).unwrap();
    expired.write_backup(dir.path().join("deal.grant")).unwrap();
    assert!(!run(dir.path(), verify_request()).0.status.success());
}

#[test]
fn a_third_party_cannot_export_or_authenticate_a_participant_grant() {
    let dir = tempfile::tempdir().unwrap();
    let public = exported(dir.path());
    std::fs::remove_file(dir.path().join("deal.grant")).unwrap();
    std::fs::remove_file(dir.path().join("issuer.key")).unwrap();
    private_file(&dir.path().join("issuer.key"), &[3; 32]);
    let (output, response) = run(dir.path(), export_request(&public));
    assert!(!output.status.success());
    assert_eq!(response["error"], "issuer is not an agreement participant");
    assert!(!dir.path().join("deal.grant").exists());
    let issuer = AuthorizationIdentity::from_bytes(&[3; 32]).unwrap();
    let recipient = DisclosureIdentity::load(dir.path().join("auditor.key")).unwrap();
    DisclosureGrant::seal(
        &fixture(),
        &issuer,
        recipient.public_key(),
        GRANT_EXPIRY,
        100,
    )
    .unwrap()
    .write_backup(dir.path().join("deal.grant"))
    .unwrap();
    let mut request = verify_request();
    request["expected_issuer"] = json!(format!("0x{}", hex::encode(issuer.address())));
    assert!(!run(dir.path(), request).0.status.success());
}

#[test]
fn payment_verification_rejects_deployment_mismatch_before_rpc_access() {
    let dir = tempfile::tempdir().unwrap();
    exported(dir.path());
    let mut request = verify_request();
    request["method"] = json!("verify_payment");
    request["deployment"] = json!({
        "namespace": "eip155:1",
        "settlement_contract": format!("0x{}", "11".repeat(20)),
        "verifier_version": 1,
        "rpc_url": "http://private-password@127.0.0.1:1",
    });
    let (output, response) = run(dir.path(), request);
    assert!(!output.status.success());
    assert_eq!(response["error"], "deployment does not match agreement");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-password"));
}

#[test]
fn malformed_unknown_and_oversized_requests_do_not_expose_input() {
    let dir = tempfile::tempdir().unwrap();
    for bytes in [
        b"not JSON private-secret".to_vec(),
        vec![b' '; 64 * 1024 + 1],
        serde_json::to_vec(
            &json!({"method":"keygen","key_file":"auditor.key","secret":"private-secret"}),
        )
        .unwrap(),
    ] {
        let output = run_bytes(dir.path(), &bytes);
        assert!(!output.status.success());
        assert!(output.stderr.is_empty());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private-secret"));
    }
    assert!(!dir.path().join("auditor.key").exists());
}

#[test]
fn oversized_selected_evidence_is_rejected_without_a_grant() {
    let dir = tempfile::tempdir().unwrap();
    private_file(&dir.path().join("selected.evidence"), b"");
    OpenOptions::new()
        .write(true)
        .open(dir.path().join("selected.evidence"))
        .unwrap()
        .set_len(MAX_DISCLOSURE_BYTES as u64 + 1)
        .unwrap();
    let (_, response) = run(dir.path(), export_request(&"11".repeat(32)));
    assert_eq!(response["error"], "invalid private input");
    assert!(!dir.path().join("deal.grant").exists());
}

#[cfg(unix)]
#[test]
fn world_readable_and_symlinked_private_inputs_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let public = exported(dir.path());
    std::fs::remove_file(dir.path().join("deal.grant")).unwrap();
    std::fs::set_permissions(
        dir.path().join("selected.evidence"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let (output, response) = run(dir.path(), export_request(&public));
    assert!(!output.status.success());
    assert_eq!(
        response["error"],
        "private input permissions are not owner-only"
    );
    std::os::unix::fs::symlink(dir.path().join("auditor.key"), dir.path().join("link.key"))
        .unwrap();
    let (_, response) = run(
        dir.path(),
        json!({"method":"key_info","key_file":"link.key"}),
    );
    assert_eq!(response["error"], "invalid private input");
    assert!(!dir.path().join("deal.grant").exists());
}

#[test]
fn select_requires_initialized_participant_state_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let evidence = dir.path().join("selected.bin");
    let request = serde_json::json!({
        "method": "select",
        "state_root": dir.path().join("state"),
        "operation_ref": "01".repeat(32),
        "store_root": dir.path().join("store"),
        "namespace": "test",
        "transcript_hash_version": 1,
        "evidence_file": evidence,
    });
    let (output, response) = run(dir.path(), request);
    assert!(!output.status.success());
    assert_eq!(response["status"], "error");
    assert_eq!(response["error"], "cannot read durable agreement opening");
    assert!(
        !evidence.exists(),
        "a failed select must not create evidence"
    );
}

#[test]
fn issuer_selects_evidence_from_durable_state_and_a_fresh_recipient_verifies() {
    issuer_workflow(
        fixture(),
        verify_request()["expected_issuer"]
            .as_str()
            .unwrap()
            .to_owned(),
    );
}

fn shielded_fixture() -> SelectedAgreement {
    use erebus_core::{
        shielded::note_spend_tag,
        shielded_auth::{derive_key, sign_message},
    };
    let mut agreement = fixture();
    agreement.terms.suite_id = 2;
    agreement.terms.settlement_mode = SettlementMode::Shielded;
    agreement.terms.domain.pool = agreement.terms.domain.settlement_contract.clone();
    agreement.terms.required_guarantees = GuaranteeSet::from_bits(7).unwrap();
    agreement.terms.buyer_authorization_key =
        KeyBytes::new(derive_key(&[1; 32]).unwrap().to_vec()).unwrap();
    agreement.terms.seller_authorization_key =
        KeyBytes::new(derive_key(&[2; 32]).unwrap().to_vec()).unwrap();
    let mut secret = [0; 32];
    secret[31] = 3;
    agreement.terms.payment_recipient =
        KeyBytes::new(note_spend_tag(&secret).unwrap().to_vec()).unwrap();
    agreement.blinding = CommitmentBlinding::from_bytes(secret);
    let commitment = commit_agreement(&agreement.terms, &agreement.blinding).unwrap();
    let authorize = |role, seed| {
        let message = authorization_digest(&agreement.terms.domain, role, &commitment, 2).unwrap();
        let (_, signature) = sign_message(&seed, &message).unwrap();
        Authorization {
            role,
            suite_id: 2,
            commitment,
            signature: SignatureBytes::new(signature.to_vec()).unwrap(),
        }
    };
    agreement.buyer = authorize(Role::Buyer, [1; 32]);
    agreement.seller = authorize(Role::Seller, [2; 32]);
    agreement
}

#[test]
fn shielded_issuer_selects_evidence_and_a_fresh_recipient_verifies() {
    let agreement = shielded_fixture();
    let issuer = format!(
        "0x{}",
        hex::encode(agreement.terms.seller_authorization_key.as_bytes())
    );
    issuer_workflow(agreement, issuer);
}

fn issuer_workflow(agreement: SelectedAgreement, issuer: String) {
    let dir = tempfile::tempdir().unwrap();
    let mode = if agreement.terms.settlement_mode == SettlementMode::Shielded {
        "shielded"
    } else {
        "public_bound"
    };
    let verify = json!({"method":"verify_agreement", "grant_file":"deal.grant", "key_file":"auditor.key", "expected_issuer":issuer});

    // One participant's durable transcript.
    let store_root = dir.path().join("store");
    let store = FileTranscriptStore::open(&store_root).unwrap();
    for message in &agreement.messages {
        store
            .append(
                "workflow",
                agreement.terms.deal_id,
                TRANSCRIPT_HASH_VERSION,
                message,
            )
            .unwrap();
    }

    // The durable agreement opening in a coordinator state directory.
    let state_root = dir.path().join("state");
    let context = SettlementContext {
        require_local_proving: true,
        mode: agreement.terms.settlement_mode,
        domain: agreement.terms.domain.clone(),
        suite_id: agreement.terms.suite_id,
        asset: agreement.terms.asset.clone(),
        required_guarantees: agreement.terms.required_guarantees,
    };
    let capabilities = BackendCapabilities {
        suites: [agreement.terms.suite_id].into_iter().collect(),
        modes: [agreement.terms.settlement_mode].into_iter().collect(),
        guarantees: agreement.terms.required_guarantees,
        local_proving: true,
    };
    let policy = SpendingPolicy {
        per_deal_max: BaseUnits::new(1_000_000),
        allowed_assets: [agreement.terms.asset.clone()].into_iter().collect(),
        ..SpendingPolicy::default()
    };
    let coordinator = Coordinator::open(
        &state_root,
        agreement.terms.buyer_authorization_key.clone(),
        context.clone(),
        &context,
        &capabilities,
        policy,
    )
    .unwrap();
    let operation_ref = [3; 32];
    coordinator
        .record_intent(operation_ref, &agreement.terms, &agreement.blinding, 1_000)
        .unwrap();
    coordinator
        .authorize_buyer(operation_ref, 1_001, |_, _| {
            Ok::<_, ()>(agreement.buyer.clone())
        })
        .unwrap();
    coordinator
        .accept_seller(operation_ref, &agreement.seller)
        .unwrap();
    drop(coordinator);

    // A fresh issuer process rebuilds canonical evidence from those two stores only.
    let snapshot = std::fs::read(state_root.join("coordinator.json")).unwrap();
    let request = json!({
        "method": "select",
        "state_root": state_root,
        "operation_ref": hex::encode(operation_ref),
        "store_root": store_root,
        "namespace": "workflow",
        "transcript_hash_version": TRANSCRIPT_HASH_VERSION,
        "evidence_file": "selected.evidence",
    });
    let (output, response) = run(dir.path(), request.clone());
    assert!(output.status.success(), "{response}");
    assert_eq!(response["status"], "ok");
    assert_eq!(response["deal_id"], hex::encode(agreement.terms.deal_id));
    assert!(dir.path().join("selected.evidence").exists());
    assert_eq!(
        std::fs::read(state_root.join("coordinator.json")).unwrap(),
        snapshot
    );
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(dir.path().join("selected.evidence"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    let selected = std::fs::read(dir.path().join("selected.evidence")).unwrap();
    let (output, _) = run(dir.path(), request.clone());
    assert!(
        !output.status.success(),
        "select must refuse to overwrite evidence"
    );
    assert_eq!(
        std::fs::read(dir.path().join("selected.evidence")).unwrap(),
        selected
    );
    let mut wrong_namespace = request;
    wrong_namespace["namespace"] = json!("missing");
    wrong_namespace["evidence_file"] = json!("unverified.evidence");
    let (output, _) = run(dir.path(), wrong_namespace);
    assert!(!output.status.success());
    assert!(!dir.path().join("unverified.evidence").exists());

    let auditor = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(auditor.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let (output, key) = run(
        auditor.path(),
        json!({"method": "keygen", "key_file": "auditor.key"}),
    );
    assert!(output.status.success());
    let public = key["recipient_public_key"].as_str().unwrap().to_owned();
    private_file(&dir.path().join("issuer.key"), &[2; 32]);
    let (output, response) = run(dir.path(), export_request(&public));
    assert!(output.status.success(), "{response}");
    std::fs::copy(
        dir.path().join("deal.grant"),
        auditor.path().join("deal.grant"),
    )
    .unwrap();
    let deal_id = hex::encode(agreement.terms.deal_id);
    drop(store);
    drop(agreement);
    dir.close().unwrap();

    // The participant directory no longer exists. This process has only the grant and
    // auditor key, in a separate directory, with no inherited environment.
    assert_eq!(std::fs::read_dir(auditor.path()).unwrap().count(), 2);
    let (output, response) = run(auditor.path(), verify.clone());
    assert!(output.status.success(), "{response}");
    assert_eq!(response["agreement_verified"], true);
    assert_eq!(response["payment_verified"], false);
    assert_eq!(response["delivery_verified"], false);
    assert_eq!(response["deal_id"], deal_id);
    assert_eq!(response["mode"], mode);
    let mut wrong_issuer = verify.clone();
    wrong_issuer["expected_issuer"] = json!("0x00");
    assert!(!run(auditor.path(), wrong_issuer).0.status.success());
    if let Some(python) = std::env::var_os("EREBUS_TEST_MCP_PYTHON") {
        let check =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/check-disclosure-mcp.py");
        let output = Command::new(python)
            .env_clear()
            .current_dir(auditor.path())
            .arg(check)
            .arg(binary())
            .arg(auditor.path())
            .arg(&issuer)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["deal_id"], deal_id);
        assert_eq!(response["agreement_verified"], true);
        assert_eq!(response["payment_verified"], false);
    }
}
