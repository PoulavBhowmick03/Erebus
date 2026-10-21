//! Shared metered fixtures: a suite-2 agreement, a loopback seller, and its helpers.
//!
//! Two test binaries include this module with different needs, so each item is compiled even
//! where one of them does not use it.
#![allow(dead_code)]

use axum::extract::DefaultBodyLimit;
use erebus_core::{
    auth::{authorization_digest, Authorization, Role},
    commitment::{commit_agreement, CommitmentBlinding},
    ids::{BaseUnits, KeyBytes, SignatureBytes},
    shielded_auth, suite,
    terms::{AgreementTerms, FeePolicy, GuaranteeSet, SettlementMode},
};
use erebus_shielded_prover::{
    access::{
        metered::{
            http::MeteredService, sign_metered_request, DeploymentBinding, MeteredIssuer,
            MeteredOperation, MeteredPolicy, MeteredProfile, MeteredRequest, PaymentCertifier,
            ResourceUnavailable, ResponseSource,
        },
        AccessError, CheckedAgreement,
    },
    disclosure::PaymentCertificate,
    observation::SettlementLocation,
    rpc::PoolRuntimePins,
};
use erebus_transport::disclosure::{SelectedAgreement, VerifiedAgreement};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    io::Write,
    path::Path,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

pub const SERVICE_ID: [u8; 32] = [0x42; 32];
pub const BUYER: [u8; 32] = [1; 32];
pub const SELLER: [u8; 32] = [2; 32];
pub const OBSERVERS: [[u8; 32]; 2] = [[0x51; 32], [0x52; 32]];
pub const PINS: PoolRuntimePins = PoolRuntimePins {
    pool: [0x71; 32],
    poseidon: [0x72; 32],
    deposit: [0x73; 32],
    transfer: [0x74; 32],
    withdraw: [0x75; 32],
};
pub const FAR: u64 = 1_800_000_000;
pub const RESOURCE: &str = "data.feed.v1";

/// The deployment binding an agreement's terms describe, with fixed test pins.
pub fn deployment(evidence: &SelectedAgreement) -> DeploymentBinding {
    DeploymentBinding {
        chain_id: evidence.terms.domain.namespace.reference().parse().unwrap(),
        pool: evidence
            .terms
            .domain
            .pool
            .as_ref()
            .unwrap()
            .as_bytes()
            .try_into()
            .unwrap(),
        first_block: 1,
        first_hash: [0x31; 32],
        runtime: PINS,
    }
}

/// Suite-2 metered agreement over agreement vector 0, with N calls.
pub fn fixture(quantity: u128, max_response_bytes: u32) -> (MeteredPolicy, SelectedAgreement) {
    let value: Value = serde_json::from_str(include_str!(
        "../../../core/tests/fixtures/agreement-v1-vectors.json"
    ))
    .unwrap();
    let mut terms = AgreementTerms::decode(
        &hex::decode(
            value["vectors"][0]["expected"]["canonicalHex"]
                .as_str()
                .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    terms.transcript_root = [0; 32];
    terms.suite_id = 2;
    terms.settlement_mode = SettlementMode::Shielded;
    terms.domain.pool = terms.domain.settlement_contract.clone();
    terms.required_guarantees = GuaranteeSet::from_bits(7).unwrap();
    terms.buyer_authorization_key =
        KeyBytes::new(shielded_auth::derive_key(&BUYER).unwrap().to_vec()).unwrap();
    terms.seller_authorization_key =
        KeyBytes::new(shielded_auth::derive_key(&SELLER).unwrap().to_vec()).unwrap();
    terms.payment_recipient = KeyBytes::new([0x0b; 32].to_vec()).unwrap();
    terms.fee_policy = FeePolicy::none();
    let policy = MeteredPolicy {
        service_id: SERVICE_ID,
        seller: terms.seller_authorization_key.clone(),
        suite_id: 2,
        resource: RESOURCE.into(),
        profile: MeteredProfile::new(max_response_bytes).unwrap(),
    };
    terms.service.resource = policy.resource.clone();
    terms.service.quantity = BaseUnits::new(quantity);
    terms.service.unit = "call".into();
    terms.service.fulfillment_method = "http-metered-v1".into();
    terms.service.fulfillment_digest = policy.profile.profile_digest();
    terms.service.access_recipient = terms.buyer_authorization_key.clone();
    terms.service.delivery_deadline = FAR;
    let evidence = agreement(terms, BUYER);
    (policy, evidence)
}

/// Suite-2 evidence for `terms`, signed by the buyer `buyer` and [`SELLER`].
///
/// The buyer and access-recipient keys are set from `buyer`.
pub fn agreement(mut terms: AgreementTerms, buyer: [u8; 32]) -> SelectedAgreement {
    terms.buyer_authorization_key =
        KeyBytes::new(shielded_auth::derive_key(&buyer).unwrap().to_vec()).unwrap();
    terms.service.access_recipient = terms.buyer_authorization_key.clone();
    let blinding = CommitmentBlinding::from_bytes([0x0a; 32]);
    let commitment = commit_agreement(&terms, &blinding).unwrap();
    let authorization = |role, seed| Authorization {
        role,
        suite_id: 2,
        commitment,
        signature: SignatureBytes::new(
            shielded_auth::sign_message(
                &seed,
                &authorization_digest(&terms.domain, role, &commitment, 2).unwrap(),
            )
            .unwrap()
            .1
            .to_vec(),
        )
        .unwrap(),
    };
    SelectedAgreement {
        buyer: authorization(Role::Buyer, buyer),
        seller: authorization(Role::Seller, SELLER),
        terms,
        blinding,
        transcript_hash_version: 1,
        messages: Vec::new(),
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// A freshly signed request. Every request gets a new nonce, as a retrying buyer would.
pub fn signed(
    evidence: &SelectedAgreement,
    operation: MeteredOperation,
    index: u64,
) -> MeteredRequest {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let mut nonce = suite::keccak256(&[&COUNTER.fetch_add(1, Ordering::SeqCst).to_be_bytes()]);
    nonce[0] |= 1;
    sign_metered_request(
        evidence,
        SERVICE_ID,
        operation,
        index,
        nonce,
        now() + 120,
        &BUYER,
    )
    .unwrap()
}

pub fn certificate(
    evidence: &SelectedAgreement,
    agreement: &VerifiedAgreement,
    observers: [[u8; 32]; 2],
) -> PaymentCertificate {
    let terms = &evidence.terms;
    let token = terms.asset.asset_reference().strip_prefix("0x").unwrap();
    PaymentCertificate {
        chain_id: terms.domain.namespace.reference().parse().unwrap(),
        pool: terms
            .domain
            .pool
            .as_ref()
            .unwrap()
            .as_bytes()
            .try_into()
            .unwrap(),
        domain_digest: suite::keccak256(&[&terms.domain.encode().unwrap()]),
        verifier_version: terms.domain.verifier_version,
        asset: hex::decode(token).unwrap().try_into().unwrap(),
        commitment: *agreement.commitment.as_bytes(),
        nullifier: *agreement.nullifier.as_bytes(),
        amount: terms.amount.get(),
        finalized_number: 40,
        finalized_hash: [0x40; 32],
        finalized_timestamp: 900,
        settlement: SettlementLocation {
            block_number: 30,
            block_hash: [0x30; 32],
            transaction_hash: [0x31; 32],
            log_index: 2,
        },
        observers,
        runtime: PINS,
    }
}

/// Counting stand-in for the paired observer.
pub struct Certifier {
    calls: AtomicUsize,
    answer: Mutex<Result<(), AccessError>>,
}

impl Certifier {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            answer: Mutex::new(Ok(())),
        })
    }
    pub fn answer(&self, answer: Result<(), AccessError>) {
        *self.answer.lock().unwrap() = answer;
    }
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl PaymentCertifier for Certifier {
    fn certify(
        &self,
        checked: &CheckedAgreement,
    ) -> impl Future<Output = Result<PaymentCertificate, AccessError>> + Send {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let result = (*self.answer.lock().unwrap())
            .map(|()| certificate(checked.evidence(), checked.agreement(), OBSERVERS));
        std::future::ready(result)
    }
}

/// A source whose bytes change on every fetch, so a replay that refetched would be visible.
#[derive(Clone)]
pub struct Feed(Arc<AtomicUsize>);

impl Feed {
    pub fn new() -> Self {
        Self(Arc::new(AtomicUsize::new(0)))
    }
    pub fn fetches(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

impl ResponseSource for Feed {
    fn fetch(&self, call_index: u64) -> Result<Vec<u8>, ResourceUnavailable> {
        let fetch = self.0.fetch_add(1, Ordering::SeqCst);
        Ok(format!("feed call {call_index} fetch {fetch}").into_bytes())
    }
}

/// Seller state: ledger root and owner-only evidence directory holding the agreement.
pub struct Seller {
    _dir: tempfile::TempDir,
    pub state: std::path::PathBuf,
    pub evidence: std::path::PathBuf,
    pub deployment: DeploymentBinding,
}

impl Seller {
    pub fn new(evidence: &SelectedAgreement) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("metered");
        let root = dir.path().join("evidence");
        std::fs::create_dir(&root).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let commitment = commit_agreement(&evidence.terms, &evidence.blinding).unwrap();
        private_file(
            &root.join(format!("{}.evidence", commitment.to_hex())),
            &evidence.encode().unwrap(),
        );
        Self {
            _dir: dir,
            state,
            evidence: root,
            deployment: deployment(evidence),
        }
    }

    /// Writes raw bytes as the owner-only evidence file of `commitment_hex`.
    pub fn store(&self, commitment_hex: &str, bytes: &[u8]) {
        let path = self.evidence.join(format!("{commitment_hex}.evidence"));
        let _ = std::fs::remove_file(&path);
        private_file(&path, bytes);
    }

    /// Stores `evidence` under its own commitment, as a seller's negotiation does.
    pub fn store_evidence(&self, evidence: &SelectedAgreement) -> String {
        let commitment = commit_agreement(&evidence.terms, &evidence.blinding)
            .unwrap()
            .to_hex();
        self.store(&commitment, &evidence.encode().unwrap());
        commitment
    }

    /// Ledger records written so far: (entitlements, calls).
    pub fn records(&self) -> (usize, usize) {
        let count = |name: &str| {
            std::fs::read_dir(self.state.join(name)).map_or(0, |entries| {
                entries
                    .filter(|entry| {
                        entry
                            .as_ref()
                            .unwrap()
                            .path()
                            .extension()
                            .is_some_and(|extension| extension == "json")
                    })
                    .count()
            })
        };
        (count("entitlements"), count("calls"))
    }
}

pub fn private_file(path: &Path, bytes: &[u8]) {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

/// One in-process service on an ephemeral loopback port. Dropping it stops the server.
pub struct Server {
    pub url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn start<S: ResponseSource + Send + Sync + 'static>(
    seller: &Seller,
    policy: &MeteredPolicy,
    certifier: &Arc<Certifier>,
    source: S,
) -> Server {
    let issuer =
        MeteredIssuer::open(&seller.state, policy.clone(), OBSERVERS, seller.deployment).unwrap();
    let router = MeteredService::new(issuer, Arc::clone(certifier), source, &seller.evidence, 16)
        .router()
        .layer(DefaultBodyLimit::max(8192));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server { url, task }
}

pub async fn post(url: &str, body: &impl serde::Serialize) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("{url}/v1/metered"))
        .json(body)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
}

pub async fn call(server: &Server, evidence: &SelectedAgreement, index: u64) -> (u16, Value) {
    post(
        &server.url,
        &signed(evidence, MeteredOperation::Call, index),
    )
    .await
}

pub async fn status(server: &Server, evidence: &SelectedAgreement) -> (u16, Value) {
    post(&server.url, &signed(evidence, MeteredOperation::Status, 0)).await
}

pub fn body(reply: &Value) -> Vec<u8> {
    let bytes = hex::decode(reply["response_hex"].as_str().unwrap()).unwrap();
    assert_eq!(
        reply["response_sha256"],
        hex::encode(Sha256::digest(&bytes))
    );
    bytes
}

pub fn write_feed(path: &Path, bytes: &[u8]) {
    let staged = path.with_extension("staged");
    std::fs::write(&staged, bytes).unwrap();
    std::fs::rename(staged, path).unwrap();
}
