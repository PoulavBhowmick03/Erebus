use super::*;
use crate::{
    access::{
        metered::{sign_metered_request, MeteredProfile},
        MAX_REQUEST_LIFETIME,
    },
    observation::SettlementLocation,
};
use erebus_core::{
    auth::{authorization_digest, Authorization, Role},
    commitment::{commit_agreement, CommitmentBlinding},
    ids::{BaseUnits, KeyBytes, SignatureBytes},
    shielded_auth, suite,
    terms::{AgreementTerms, FeePolicy, GuaranteeSet, SettlementMode},
};
use std::{
    collections::HashMap,
    collections::HashSet,
    future::Future,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Barrier, Mutex, OnceLock,
    },
};

const SERVICE_ID: [u8; 32] = [0x42; 32];
const NOW: u64 = 1000;
const BUYER: [u8; 32] = [1; 32];
const SELLER: [u8; 32] = [2; 32];
const OBSERVERS: [[u8; 32]; 2] = [[0x51; 32], [0x52; 32]];
const PINS: PoolRuntimePins = PoolRuntimePins {
    pool: [0x71; 32],
    poseidon: [0x72; 32],
    deposit: [0x73; 32],
    transfer: [0x74; 32],
    withdraw: [0x75; 32],
};
const FAR: u64 = 1_800_000_000;

/// The deployment binding the fixture agreements are certified under.
fn deployment() -> DeploymentBinding {
    DeploymentBinding {
        chain_id: 31_337,
        pool: [3; 20],
        first_block: 1,
        first_hash: [0x31; 32],
        runtime: PINS,
    }
}

fn run<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(future)
}

/// Suite-2 metered agreement over agreement vector 0, with N calls and a delivery deadline.
fn fixture(
    quantity: u128,
    deadline: u64,
    max_response_bytes: u32,
) -> (MeteredPolicy, SelectedAgreement) {
    let value: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../core/tests/fixtures/agreement-v1-vectors.json"
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
        resource: "data.feed.v1".into(),
        profile: MeteredProfile::new(max_response_bytes).unwrap(),
    };
    terms.service.resource = policy.resource.clone();
    terms.service.quantity = BaseUnits::new(quantity);
    terms.service.unit = "call".into();
    terms.service.fulfillment_method = "http-metered-v1".into();
    terms.service.fulfillment_digest = policy.profile.profile_digest();
    terms.service.access_recipient = terms.buyer_authorization_key.clone();
    terms.service.delivery_deadline = deadline;
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
    let evidence = SelectedAgreement {
        buyer: authorization(Role::Buyer, BUYER),
        seller: authorization(Role::Seller, SELLER),
        terms,
        blinding,
        transcript_hash_version: 1,
        messages: Vec::new(),
    };
    (policy, evidence)
}

fn feed() -> (MeteredPolicy, SelectedAgreement) {
    fixture(3, FAR, 65536)
}

type SignedKey = ([u8; 32], u8, u64, u64);

/// Suite-2 signing is slow in debug builds, so identical requests are signed once per process.
/// The seller keeps no nonce cache, so a reused signed request is a legitimate retry.
fn signed(
    evidence: &SelectedAgreement,
    operation: MeteredOperation,
    index: u64,
    now: u64,
) -> MeteredRequest {
    static CACHE: OnceLock<Mutex<HashMap<SignedKey, MeteredRequest>>> = OnceLock::new();
    let commitment = *evidence.buyer.commitment.as_bytes();
    let key = (commitment, operation.code(), index, now + 100);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(request) = cache.lock().unwrap().get(&key) {
        return request.clone();
    }
    let mut nonce = suite::keccak256(&[
        &commitment,
        &[operation.code()],
        &index.to_be_bytes(),
        &now.to_be_bytes(),
    ]);
    nonce[0] |= 1;
    let request = sign_metered_request(
        evidence,
        SERVICE_ID,
        operation,
        index,
        nonce,
        now + 100,
        &BUYER,
    )
    .unwrap();
    cache.lock().unwrap().insert(key, request.clone());
    request
}

fn certificate(
    evidence: &SelectedAgreement,
    agreement: &erebus_transport::disclosure::VerifiedAgreement,
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

/// Counting stand-in for the paired observer. It proves how often finality is observed.
struct Certifier {
    calls: AtomicUsize,
    answer: Mutex<Result<(), AccessError>>,
    observers: [[u8; 32]; 2],
    edit: Mutex<Option<fn(&mut PaymentCertificate)>>,
}

impl Certifier {
    fn new() -> Self {
        Self::observing(OBSERVERS)
    }
    fn observing(observers: [[u8; 32]; 2]) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            answer: Mutex::new(Ok(())),
            observers,
            edit: Mutex::new(None),
        }
    }
    fn answer(&self, answer: Result<(), AccessError>) {
        *self.answer.lock().unwrap() = answer;
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl PaymentCertifier for Certifier {
    fn certify(
        &self,
        checked: &CheckedAgreement,
    ) -> impl Future<Output = Result<PaymentCertificate, AccessError>> + Send {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let result = (*self.answer.lock().unwrap()).map(|()| {
            let mut certificate =
                certificate(checked.evidence(), checked.agreement(), self.observers);
            if let Some(edit) = *self.edit.lock().unwrap() {
                edit(&mut certificate);
            }
            certificate
        });
        std::future::ready(result)
    }
}

/// A feed whose bytes change on every fetch, so a replay that refetched would be visible.
struct Feed {
    fetches: Arc<AtomicUsize>,
}

impl Feed {
    fn new() -> Self {
        Self {
            fetches: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn fetches(&self) -> usize {
        self.fetches.load(Ordering::SeqCst)
    }
}

impl ResponseSource for Feed {
    fn fetch(&self, call_index: u64) -> Result<Vec<u8>, ResourceUnavailable> {
        let fetch = self.fetches.fetch_add(1, Ordering::SeqCst);
        Ok(format!("feed call {call_index} fetch {fetch}").into_bytes())
    }
}

fn open(root: &std::path::Path, policy: &MeteredPolicy) -> MeteredIssuer {
    MeteredIssuer::open(root, policy.clone(), OBSERVERS, deployment()).unwrap()
}

fn call(
    issuer: &MeteredIssuer,
    certifier: &Certifier,
    source: &impl ResponseSource,
    evidence: &SelectedAgreement,
    index: u64,
    now: u64,
) -> Result<MeteredCall, MeteredError> {
    let request = signed(evidence, MeteredOperation::Call, index, now);
    match run(issuer.handle(certifier, source, evidence, &request, now))? {
        MeteredReply::Call(call) => Ok(call),
        MeteredReply::Status(_) => panic!("a call returned status"),
    }
}

fn status(
    issuer: &MeteredIssuer,
    certifier: &Certifier,
    evidence: &SelectedAgreement,
    now: u64,
) -> Result<MeteredStatus, MeteredError> {
    let request = signed(evidence, MeteredOperation::Status, 0, now);
    let source = |_: u64| -> Result<Vec<u8>, ResourceUnavailable> { panic!("status fetched") };
    match run(issuer.handle(certifier, &source, evidence, &request, now))? {
        MeteredReply::Status(status) => Ok(status),
        MeteredReply::Call(_) => panic!("status returned a call"),
    }
}

/// Quantity, next index, and remaining allowance of a status reply. Also checks that the calls
/// list names exactly the indices below `next`.
fn counts(status: MeteredStatus) -> (u64, u64, u64) {
    assert!(status
        .calls
        .iter()
        .map(|call| call.call_index)
        .eq(0..status.next));
    (status.quantity, status.next, status.remaining)
}

/// Every field of a returned response except whether it was repeated.
fn same_response(left: &MeteredCall, right: &MeteredCall) -> bool {
    left.call_index == right.call_index
        && left.body == right.body
        && left.response_sha256 == right.response_sha256
        && left.committed_at == right.committed_at
        && left.late == right.late
}

fn entitlement_ledger_id(evidence: &SelectedAgreement) -> LedgerId {
    LedgerId::new(entitlement_id(&SERVICE_ID, evidence.buyer.commitment.as_bytes()).as_bytes())
}

fn call_ledger_id(evidence: &SelectedAgreement, index: u64) -> LedgerId {
    let entitlement = entitlement_id(&SERVICE_ID, evidence.buyer.commitment.as_bytes());
    LedgerId::new(call_id(&entitlement, index).as_bytes())
}

#[test]
fn errors_map_to_the_profile_statuses_and_exhaustion_is_never_402() {
    let cases = [
        (MeteredError::AllowanceExhausted, 403, "allowance_exhausted"),
        (MeteredError::OutOfOrder, 409, "out_of_order"),
        (MeteredError::Resource, 503, "resource_unavailable"),
        (
            MeteredError::CertificateMismatch,
            400,
            "certificate_mismatch",
        ),
        (AccessError::Pending.into(), 202, "payment_pending"),
        (
            AccessError::Observation.into(),
            503,
            "observation_unavailable",
        ),
        (AccessError::Storage.into(), 503, "storage_unavailable"),
        (AccessError::Authentication.into(), 401, "unauthenticated"),
        (AccessError::Agreement.into(), 400, "agreement_mismatch"),
    ];
    for (error, status, code) in cases {
        assert_eq!(error.status(), status);
        assert_eq!(error.code(), code);
        assert_ne!(error.status(), 402);
    }
}

#[test]
fn open_requires_a_shielded_policy_and_two_distinct_observers() {
    let (policy, _) = feed();
    let root = tempfile::tempdir().unwrap();
    let mut suite_one = policy.clone();
    suite_one.suite_id = 1;
    suite_one.seller = KeyBytes::new(vec![5; 20]).unwrap();
    for (policy, observers) in [
        (suite_one, OBSERVERS),
        (policy.clone(), [[0x51; 32], [0x51; 32]]),
        (policy.clone(), [[0; 32], [0x52; 32]]),
    ] {
        assert_eq!(
            MeteredIssuer::open(root.path(), policy, observers, deployment()).unwrap_err(),
            MeteredError::Access(AccessError::Agreement)
        );
    }
    // A zero deployment field is a configuration error, never a skipped check.
    for deployment in [
        DeploymentBinding {
            chain_id: 0,
            ..deployment()
        },
        DeploymentBinding {
            pool: [0; 20],
            ..deployment()
        },
        DeploymentBinding {
            first_hash: [0; 32],
            ..deployment()
        },
        DeploymentBinding {
            runtime: PoolRuntimePins {
                pool: [0; 32],
                ..PINS
            },
            ..deployment()
        },
    ] {
        assert_eq!(
            MeteredIssuer::open(root.path(), policy.clone(), OBSERVERS, deployment).unwrap_err(),
            MeteredError::Access(AccessError::Agreement)
        );
    }
    let file = root.path().join("file");
    std::fs::write(&file, b"x").unwrap();
    assert_eq!(
        MeteredIssuer::open(&file, policy, OBSERVERS, deployment()).unwrap_err(),
        MeteredError::Access(AccessError::Storage)
    );
}

#[test]
fn one_call_allowance_replays_and_then_answers_exhausted() {
    let (policy, evidence) = fixture(1, FAR, 65536);
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    let certifier = Certifier::new();
    let feed = Feed::new();
    let first = call(&issuer, &certifier, &feed, &evidence, 0, NOW).unwrap();
    assert!(!first.repeated && !first.late);
    assert_eq!(first.body, b"feed call 0 fetch 0");
    assert_eq!(
        first.response_sha256,
        <[u8; 32]>::from(Sha256::digest(&first.body))
    );
    let again = call(&issuer, &certifier, &feed, &evidence, 0, NOW + 1).unwrap();
    assert!(again.repeated && same_response(&first, &again));
    let exhausted = call(&issuer, &certifier, &feed, &evidence, 1, NOW).unwrap_err();
    assert_eq!(exhausted, MeteredError::AllowanceExhausted);
    assert_eq!(
        (exhausted.status(), exhausted.code()),
        (403, "allowance_exhausted")
    );
    assert_eq!(
        call(&issuer, &certifier, &feed, &evidence, 9, NOW).unwrap_err(),
        MeteredError::AllowanceExhausted
    );
    assert_eq!(
        counts(status(&issuer, &certifier, &evidence, NOW).unwrap()),
        (1, 1, 0)
    );
    // Exhaustion still serves old indices, from storage.
    assert!(same_response(
        &first,
        &call(&issuer, &certifier, &feed, &evidence, 0, NOW + 2).unwrap()
    ));
    assert_eq!(feed.fetches(), 1);
    assert_eq!(certifier.calls(), 1);
    assert_eq!(issuer.calls.records().unwrap().len(), 1);
}

#[test]
fn the_largest_allowance_is_accepted_and_out_of_range_quantities_are_not() {
    let (policy, evidence) = fixture(4096, FAR, 65536);
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    let certifier = Certifier::new();
    let feed = Feed::new();
    assert_eq!(
        counts(status(&issuer, &certifier, &evidence, NOW).unwrap()),
        (4096, 0, 4096)
    );
    assert_eq!(
        call(&issuer, &certifier, &feed, &evidence, 4095, NOW).unwrap_err(),
        MeteredError::OutOfOrder
    );
    assert_eq!(
        call(&issuer, &certifier, &feed, &evidence, 4096, NOW).unwrap_err(),
        MeteredError::AllowanceExhausted
    );
    assert!(
        !call(&issuer, &certifier, &feed, &evidence, 0, NOW)
            .unwrap()
            .repeated
    );
    assert_eq!(
        status(&issuer, &certifier, &evidence, NOW)
            .unwrap()
            .remaining,
        4095
    );

    // Canonical encoding rejects a zero quantity, so N=0 can only arrive as altered terms,
    // signed for by the unaltered agreement with the same commitment.
    let (one_policy, one) = fixture(1, FAR, 65536);
    let mut zero = one.clone();
    zero.terms.service.quantity = BaseUnits::new(0);
    let (large_policy, large) = fixture(4097, FAR, 65536);
    for (policy, evidence, signer) in [
        (one_policy, zero, one),
        (large_policy, large.clone(), large),
    ] {
        let root = tempfile::tempdir().unwrap();
        let issuer = open(root.path(), &policy);
        let certifier = Certifier::new();
        let request = signed(&signer, MeteredOperation::Call, 0, NOW);
        assert_eq!(
            run(issuer.handle(&certifier, &feed, &evidence, &request, NOW)).unwrap_err(),
            MeteredError::Access(AccessError::Agreement)
        );
        assert_eq!(certifier.calls(), 0);
        assert!(issuer.entitlements.records().unwrap().is_empty());
    }
}

#[test]
fn out_of_order_calls_consume_nothing() {
    let (policy, evidence) = feed();
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    let certifier = Certifier::new();
    let feed = Feed::new();
    let gap = call(&issuer, &certifier, &feed, &evidence, 1, NOW).unwrap_err();
    assert_eq!((gap, gap.status()), (MeteredError::OutOfOrder, 409));
    assert_eq!(status(&issuer, &certifier, &evidence, NOW).unwrap().next, 0);
    call(&issuer, &certifier, &feed, &evidence, 0, NOW).unwrap();
    assert_eq!(
        call(&issuer, &certifier, &feed, &evidence, 2, NOW).unwrap_err(),
        MeteredError::OutOfOrder
    );
    call(&issuer, &certifier, &feed, &evidence, 1, NOW).unwrap();
    call(&issuer, &certifier, &feed, &evidence, 2, NOW).unwrap();
    assert_eq!(feed.fetches(), 3);
    assert_eq!(issuer.calls.records().unwrap().len(), 3);
}

#[test]
fn finality_is_observed_once_and_calls_continue_through_an_rpc_outage() {
    let (policy, evidence) = fixture(5, FAR, 65536);
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    let certifier = Certifier::new();
    let feed = Feed::new();
    let mut returned = Vec::new();
    for index in 0..3 {
        returned.push(call(&issuer, &certifier, &feed, &evidence, index, NOW).unwrap());
        status(&issuer, &certifier, &evidence, NOW).unwrap();
    }
    assert_eq!(certifier.calls(), 1);
    // Observers are down after activation; the stored certificate still authorizes calls.
    certifier.answer(Err(AccessError::Observation));
    drop(issuer);
    let restarted = open(root.path(), &policy);
    for index in 3..5 {
        returned.push(call(&restarted, &certifier, &feed, &evidence, index, NOW).unwrap());
    }
    for original in &returned {
        let replay = call(
            &restarted,
            &certifier,
            &feed,
            &evidence,
            original.call_index,
            NOW,
        )
        .unwrap();
        assert!(replay.repeated && same_response(original, &replay));
    }
    assert_eq!(
        call(&restarted, &certifier, &feed, &evidence, 5, NOW).unwrap_err(),
        MeteredError::AllowanceExhausted
    );
    assert_eq!(certifier.calls(), 1);
    assert_eq!(feed.fetches(), 5);
}

#[test]
fn pending_or_unavailable_finality_creates_nothing() {
    let (policy, evidence) = feed();
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    let certifier = Certifier::new();
    let feed = Feed::new();
    for (answer, status_code) in [(AccessError::Pending, 202), (AccessError::Observation, 503)] {
        certifier.answer(Err(answer));
        let error = call(&issuer, &certifier, &feed, &evidence, 0, NOW).unwrap_err();
        assert_eq!(
            (error, error.status()),
            (MeteredError::Access(answer), status_code)
        );
        assert!(issuer.entitlements.records().unwrap().is_empty());
        assert!(issuer.calls.records().unwrap().is_empty());
        assert_eq!(
            std::fs::read_dir(root.path().join(ENTITLEMENT_DIR))
                .unwrap()
                .count(),
            0
        );
    }
    assert_eq!(feed.fetches(), 0);
    certifier.answer(Ok(()));
    call(&issuer, &certifier, &feed, &evidence, 0, NOW).unwrap();
    assert_eq!(certifier.calls(), 3);
}

#[test]
fn unauthenticated_or_expired_requests_never_observe_or_consume() {
    let (policy, evidence) = feed();
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    let certifier = Certifier::new();
    let feed = Feed::new();
    let mut expired = signed(&evidence, MeteredOperation::Call, 0, NOW);
    expired.expires_at = NOW;
    let mut far = signed(&evidence, MeteredOperation::Call, 0, NOW);
    far.expires_at = NOW + MAX_REQUEST_LIFETIME + 1;
    let mut forged = signed(&evidence, MeteredOperation::Call, 0, NOW);
    forged.call_index = 1;
    let wrong_buyer = {
        let mut request = signed(&evidence, MeteredOperation::Call, 0, NOW);
        let digest = super::super::metered_request_digest(
            &evidence,
            SERVICE_ID,
            MeteredOperation::Call,
            0,
            request.nonce,
            request.expires_at,
        )
        .unwrap();
        request.signature = shielded_auth::sign_message(&[3; 32], &digest)
            .unwrap()
            .1
            .to_vec();
        request
    };
    for request in [expired, far, forged, wrong_buyer] {
        let error = run(issuer.handle(&certifier, &feed, &evidence, &request, NOW)).unwrap_err();
        assert_eq!(
            (error, error.status()),
            (MeteredError::Access(AccessError::Authentication), 401)
        );
    }
    assert_eq!(certifier.calls(), 0);
    assert_eq!(feed.fetches(), 0);
    assert!(issuer.entitlements.records().unwrap().is_empty());
}

#[test]
fn failed_or_oversize_responses_leave_next_unchanged() {
    let (policy, evidence) = fixture(2, FAR, 16);
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    let certifier = Certifier::new();
    type Source<'a> = &'a dyn Fn(u64) -> Result<Vec<u8>, ResourceUnavailable>;
    let failing = |_: u64| -> Result<Vec<u8>, ResourceUnavailable> { Err(ResourceUnavailable) };
    let oversize = |_: u64| -> Result<Vec<u8>, ResourceUnavailable> { Ok(vec![7; 17]) };
    let exact = |_: u64| -> Result<Vec<u8>, ResourceUnavailable> { Ok(vec![7; 16]) };
    let sources: [Source<'_>; 2] = [&failing, &oversize];
    for source in sources {
        let error = call(&issuer, &certifier, &source, &evidence, 0, NOW).unwrap_err();
        assert_eq!((error, error.status()), (MeteredError::Resource, 503));
        assert_eq!(status(&issuer, &certifier, &evidence, NOW).unwrap().next, 0);
        assert_eq!(
            std::fs::read_dir(root.path().join(CALL_DIR))
                .unwrap()
                .count(),
            0
        );
    }
    assert_eq!(
        call(&issuer, &certifier, &exact, &evidence, 0, NOW)
            .unwrap()
            .body,
        vec![7; 16]
    );
    assert_eq!(status(&issuer, &certifier, &evidence, NOW).unwrap().next, 1);
}

#[test]
fn the_signed_consuming_request_is_stored_with_its_call() {
    let (policy, evidence) = fixture(2, NOW, 65536);
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    let certifier = Certifier::new();
    let feed = Feed::new();
    let request = signed(&evidence, MeteredOperation::Call, 0, NOW + 1);
    let MeteredReply::Call(first) =
        run(issuer.handle(&certifier, &feed, &evidence, &request, NOW + 1)).unwrap()
    else {
        panic!("call")
    };
    // Committed after the delivery deadline: late, but the unit is still served.
    assert!(first.late);
    let stored = issuer
        .calls
        .read(&call_ledger_id(&evidence, 0))
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&stored.request).unwrap(),
        serde_json::to_value(&request).unwrap()
    );
    assert_eq!(stored.committed_at, NOW + 1);
    // A replay with a different signed request keeps the original consent record.
    let replay = call(&issuer, &certifier, &feed, &evidence, 0, NOW + 5).unwrap();
    assert!(replay.late && same_response(&first, &replay));
    let unchanged = issuer
        .calls
        .read(&call_ledger_id(&evidence, 0))
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.request.nonce, request.nonce);
}

#[test]
fn a_mismatched_certificate_is_rejected_and_a_stored_mismatch_is_never_rebuilt() {
    let (policy, evidence) = feed();
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    let certifier = Certifier::new();
    let feed = Feed::new();
    let edits: [fn(&mut PaymentCertificate); 4] = [
        |c| c.amount += 1,
        |c| c.commitment[0] ^= 1,
        |c| c.settlement.block_number = c.finalized_number + 1,
        |c| c.domain_digest[0] ^= 1,
    ];
    for edit in edits {
        *certifier.edit.lock().unwrap() = Some(edit);
        let error = call(&issuer, &certifier, &feed, &evidence, 0, NOW).unwrap_err();
        assert_eq!(
            (error, error.status()),
            (MeteredError::CertificateMismatch, 400)
        );
        assert!(issuer.entitlements.records().unwrap().is_empty());
    }
    // A certifier for other observers is a configuration error, not a certificate.
    let other = Certifier::observing([[0x61; 32], [0x62; 32]]);
    assert_eq!(
        call(&issuer, &other, &feed, &evidence, 0, NOW).unwrap_err(),
        MeteredError::Access(AccessError::Observation)
    );
    assert!(issuer.entitlements.records().unwrap().is_empty());

    *certifier.edit.lock().unwrap() = None;
    call(&issuer, &certifier, &feed, &evidence, 0, NOW).unwrap();
    let id = entitlement_ledger_id(&evidence);
    let mut stored = issuer.entitlements.read(&id).unwrap().unwrap();
    stored.certificate.amount += 1;
    issuer.entitlements.write(&stored).unwrap();
    let before = certifier.calls();
    for index in [0, 1] {
        assert_eq!(
            call(&issuer, &certifier, &feed, &evidence, index, NOW).unwrap_err(),
            MeteredError::CertificateMismatch
        );
    }
    assert_eq!(certifier.calls(), before);
    assert_eq!(
        issuer
            .entitlements
            .read(&id)
            .unwrap()
            .unwrap()
            .certificate
            .amount,
        stored.certificate.amount
    );
    assert_eq!(feed.fetches(), 1);
}

#[test]
fn corrupt_entitlement_or_call_records_fail_closed() {
    let (policy, evidence) = feed();
    let feed = Feed::new();
    let certifier = Certifier::new();
    let storage = MeteredError::Access(AccessError::Storage);

    // Corrupt E: 503, never re-observed, never rebuilt.
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    call(&issuer, &certifier, &feed, &evidence, 0, NOW).unwrap();
    let path = issuer
        .entitlements
        .record_path(&entitlement_ledger_id(&evidence));
    std::fs::write(&path, b"{}").unwrap();
    assert_eq!(
        call(&issuer, &certifier, &feed, &evidence, 0, NOW).unwrap_err(),
        storage
    );
    assert_eq!(
        call(&issuer, &certifier, &feed, &evidence, 1, NOW).unwrap_err(),
        storage
    );
    assert_eq!(
        status(&issuer, &certifier, &evidence, NOW).unwrap_err(),
        storage
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"{}");
    assert_eq!(certifier.calls(), 1);

    // Corrupt C(k) record, or a changed response body: 503, never reconsumed.
    for corrupt_blob in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let issuer = open(root.path(), &policy);
        call(&issuer, &certifier, &feed, &evidence, 0, NOW).unwrap();
        let id = call_ledger_id(&evidence, 0);
        if corrupt_blob {
            std::fs::write(issuer.calls.blob_path(&id, 0), b"another body").unwrap();
            assert_eq!(
                call(&issuer, &certifier, &feed, &evidence, 1, NOW).unwrap_err(),
                storage
            );
        } else {
            std::fs::write(issuer.calls.record_path(&id), b"{").unwrap();
            assert_eq!(
                call(&issuer, &certifier, &feed, &evidence, 1, NOW).unwrap_err(),
                storage
            );
        }
        assert_eq!(
            call(&issuer, &certifier, &feed, &evidence, 0, NOW).unwrap_err(),
            storage
        );
    }

    // A missing blob under an existing record is corruption, not "nothing stored".
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    call(&issuer, &certifier, &feed, &evidence, 0, NOW).unwrap();
    std::fs::remove_file(issuer.calls.blob_path(&call_ledger_id(&evidence, 0), 0)).unwrap();
    assert_eq!(
        call(&issuer, &certifier, &feed, &evidence, 0, NOW).unwrap_err(),
        storage
    );
    let fetches = feed.fetches();
    for index in [0, 1] {
        assert_eq!(
            call(&issuer, &certifier, &feed, &evidence, index, NOW).unwrap_err(),
            storage
        );
    }
    assert_eq!(
        status(&issuer, &certifier, &evidence, NOW).unwrap_err(),
        storage
    );
    assert_eq!(feed.fetches(), fetches);
}

#[cfg(unix)]
#[test]
fn issuer_root_and_journals_are_private_on_create_and_reopen() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let (policy, _) = feed();
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("nested/state");
    drop(open(&state, &policy));
    for directory in [&state, &state.join(ENTITLEMENT_DIR), &state.join(CALL_DIR)] {
        assert_eq!(
            std::fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    drop(open(&state, &policy));
    for directory in [&state, &state.join(ENTITLEMENT_DIR), &state.join(CALL_DIR)] {
        assert_eq!(
            std::fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    let alias = root.path().join("alias");
    symlink(&state, &alias).unwrap();
    assert!(matches!(
        MeteredIssuer::open(&alias, policy.clone(), OBSERVERS, deployment()),
        Err(MeteredError::Access(AccessError::Storage))
    ));
    for name in [ENTITLEMENT_DIR, CALL_DIR] {
        let state = root.path().join(name);
        std::fs::create_dir(&state).unwrap();
        let target = root.path().join(format!("foreign-{name}"));
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&target, state.join(name)).unwrap();
        assert!(matches!(
            MeteredIssuer::open(&state, policy.clone(), OBSERVERS, deployment()),
            Err(MeteredError::Access(AccessError::Storage))
        ));
        assert_eq!(
            std::fs::metadata(target).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
}

#[test]
fn lost_or_stale_call_state_never_reissues_an_index() {
    let (policy, evidence) = fixture(4, FAR, 65536);
    let feed = Feed::new();
    let certifier = Certifier::new();
    let storage = MeteredError::Access(AccessError::Storage);

    // A restored or lost call directory behind the hint: fail closed rather than reissue.
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    for index in 0..2 {
        call(&issuer, &certifier, &feed, &evidence, index, NOW).unwrap();
    }
    std::fs::remove_file(issuer.calls.record_path(&call_ledger_id(&evidence, 1))).unwrap();
    assert_eq!(
        call(&issuer, &certifier, &feed, &evidence, 1, NOW).unwrap_err(),
        storage
    );
    assert_eq!(
        status(&issuer, &certifier, &evidence, NOW).unwrap_err(),
        storage
    );

    // An older record missing below the hint is not reconsumed either.
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    for index in 0..2 {
        call(&issuer, &certifier, &feed, &evidence, index, NOW).unwrap();
    }
    std::fs::remove_file(issuer.calls.record_path(&call_ledger_id(&evidence, 0))).unwrap();
    assert_eq!(
        call(&issuer, &certifier, &feed, &evidence, 0, NOW).unwrap_err(),
        storage
    );
    // Status lists every committed call, so it cannot report the lost one either.
    assert_eq!(
        status(&issuer, &certifier, &evidence, NOW).unwrap_err(),
        storage
    );
    // The hint region is intact, so the next index is still consumed exactly once.
    assert!(
        !call(&issuer, &certifier, &feed, &evidence, 2, NOW)
            .unwrap()
            .repeated
    );

    // A record after a gap means the ledger is not what this process wrote.
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    for index in 0..3 {
        call(&issuer, &certifier, &feed, &evidence, index, NOW).unwrap();
    }
    let id = entitlement_ledger_id(&evidence);
    let mut stored = issuer.entitlements.read(&id).unwrap().unwrap();
    stored.next_hint = 0;
    issuer.entitlements.write(&stored).unwrap();
    std::fs::remove_file(issuer.calls.record_path(&call_ledger_id(&evidence, 1))).unwrap();
    assert_eq!(
        status(&issuer, &certifier, &evidence, NOW).unwrap_err(),
        storage
    );

    // A stale (behind) hint is only a cache: the probe counts each unit once.
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    for index in 0..2 {
        call(&issuer, &certifier, &feed, &evidence, index, NOW).unwrap();
    }
    let id = entitlement_ledger_id(&evidence);
    let mut stored = issuer.entitlements.read(&id).unwrap().unwrap();
    stored.next_hint = 0;
    issuer.entitlements.write(&stored).unwrap();
    assert_eq!(status(&issuer, &certifier, &evidence, NOW).unwrap().next, 2);
    assert!(
        call(&issuer, &certifier, &feed, &evidence, 1, NOW)
            .unwrap()
            .repeated
    );
    assert!(
        !call(&issuer, &certifier, &feed, &evidence, 2, NOW)
            .unwrap()
            .repeated
    );

    // A hint ahead of N is corruption.
    let mut stored = issuer.entitlements.read(&id).unwrap().unwrap();
    stored.next_hint = 5;
    issuer.entitlements.write(&stored).unwrap();
    assert_eq!(
        status(&issuer, &certifier, &evidence, NOW).unwrap_err(),
        storage
    );

    // Deleting E re-observes payment but finds the committed calls: no second allowance.
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    for index in 0..2 {
        call(&issuer, &certifier, &feed, &evidence, index, NOW).unwrap();
    }
    let calls_before = certifier.calls();
    std::fs::remove_file(
        issuer
            .entitlements
            .record_path(&entitlement_ledger_id(&evidence)),
    )
    .unwrap();
    assert_eq!(status(&issuer, &certifier, &evidence, NOW).unwrap().next, 2);
    assert_eq!(certifier.calls(), calls_before + 1);
}

#[test]
fn time_rollback_fails_closed() {
    let (policy, evidence) = feed();
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    let certifier = Certifier::new();
    let feed = Feed::new();
    let storage = MeteredError::Access(AccessError::Storage);
    call(&issuer, &certifier, &feed, &evidence, 0, NOW).unwrap();
    // Before the certificate was stored.
    assert_eq!(
        call(&issuer, &certifier, &feed, &evidence, 0, NOW - 1).unwrap_err(),
        storage
    );
    assert_eq!(
        status(&issuer, &certifier, &evidence, NOW - 1).unwrap_err(),
        storage
    );
    // Before a call was committed.
    call(&issuer, &certifier, &feed, &evidence, 1, NOW + 50).unwrap();
    assert_eq!(
        call(&issuer, &certifier, &feed, &evidence, 1, NOW + 49).unwrap_err(),
        storage
    );
    assert_eq!(certifier.calls(), 1);
}

#[test]
fn an_observer_change_quarantines_without_a_second_allowance() {
    let (policy, evidence) = fixture(4, FAR, 65536);
    let root = tempfile::tempdir().unwrap();
    let certifier = Certifier::new();
    let feed = Feed::new();
    let issuer = open(root.path(), &policy);
    let first = call(&issuer, &certifier, &feed, &evidence, 0, NOW).unwrap();
    call(&issuer, &certifier, &feed, &evidence, 1, NOW).unwrap();
    drop(issuer);

    let moved = [[0x61; 32], [0x62; 32]];
    let reconfigured =
        MeteredIssuer::open(root.path(), policy.clone(), moved, deployment()).unwrap();
    let request = signed(&evidence, MeteredOperation::Call, 0, NOW);
    let authorized = reconfigured.authorize(&evidence, &request, NOW).unwrap();
    assert!(authorized.needs_certificate());
    // Serving without revalidation is refused.
    assert_eq!(
        reconfigured.serve(&authorized, &feed, NOW).unwrap_err(),
        MeteredError::Access(AccessError::Observation)
    );

    // New observers that place the settlement elsewhere cannot rebind it.
    let disagreeing = Certifier::observing(moved);
    *disagreeing.edit.lock().unwrap() =
        Some(|c: &mut PaymentCertificate| c.settlement.transaction_hash[0] ^= 1);
    assert_eq!(
        call(&reconfigured, &disagreeing, &feed, &evidence, 2, NOW).unwrap_err(),
        MeteredError::Access(AccessError::Observation)
    );
    // Unavailable new observers leave it quarantined and unchanged.
    let down = Certifier::observing(moved);
    down.answer(Err(AccessError::Observation));
    assert_eq!(
        call(&reconfigured, &down, &feed, &evidence, 2, NOW).unwrap_err(),
        MeteredError::Access(AccessError::Observation)
    );

    let agreeing = Certifier::observing(moved);
    let replay = call(&reconfigured, &agreeing, &feed, &evidence, 0, NOW + 1).unwrap();
    assert!(replay.repeated && same_response(&first, &replay));
    assert_eq!(agreeing.calls(), 1);
    assert_eq!(
        counts(status(&reconfigured, &agreeing, &evidence, NOW + 1).unwrap()),
        (4, 2, 2)
    );
    call(&reconfigured, &agreeing, &feed, &evidence, 2, NOW + 1).unwrap();
    call(&reconfigured, &agreeing, &feed, &evidence, 3, NOW + 1).unwrap();
    assert_eq!(
        call(&reconfigured, &agreeing, &feed, &evidence, 4, NOW + 1).unwrap_err(),
        MeteredError::AllowanceExhausted
    );
    assert_eq!(agreeing.calls(), 1);
    assert_eq!(feed.fetches(), 4);
}

#[test]
fn a_deployment_anchor_change_quarantines_without_a_second_allowance() {
    let (policy, evidence) = fixture(4, FAR, 65536);
    let root = tempfile::tempdir().unwrap();
    let certifier = Certifier::new();
    let feed = Feed::new();
    let issuer = open(root.path(), &policy);
    let first = call(&issuer, &certifier, &feed, &evidence, 0, NOW).unwrap();
    call(&issuer, &certifier, &feed, &evidence, 1, NOW).unwrap();
    drop(issuer);

    // Same observers, chain, and pool; only the trusted deployment origin moved.
    let moved = DeploymentBinding {
        first_block: 2,
        first_hash: [0x32; 32],
        ..deployment()
    };
    let reconfigured = MeteredIssuer::open(root.path(), policy.clone(), OBSERVERS, moved).unwrap();
    let request = signed(&evidence, MeteredOperation::Call, 0, NOW);
    let authorized = reconfigured.authorize(&evidence, &request, NOW).unwrap();
    assert!(authorized.needs_certificate());
    assert_eq!(
        reconfigured.serve(&authorized, &feed, NOW).unwrap_err(),
        MeteredError::Access(AccessError::Observation)
    );

    // Revalidation rebinds the same settlement and preserves consumption.
    let replay = call(&reconfigured, &certifier, &feed, &evidence, 0, NOW + 1).unwrap();
    assert!(replay.repeated && same_response(&first, &replay));
    assert_eq!(certifier.calls(), 2);
    assert_eq!(
        counts(status(&reconfigured, &certifier, &evidence, NOW + 1).unwrap()),
        (4, 2, 2)
    );
}

fn repin(certificate: &mut PaymentCertificate) {
    certificate.runtime = PoolRuntimePins {
        pool: [0x81; 32],
        ..PINS
    };
}

#[test]
fn a_runtime_pin_change_quarantines_and_needs_a_certifier_that_verified_it() {
    let (policy, evidence) = fixture(4, FAR, 65536);
    let root = tempfile::tempdir().unwrap();
    let certifier = Certifier::new();
    let feed = Feed::new();
    let issuer = open(root.path(), &policy);
    call(&issuer, &certifier, &feed, &evidence, 0, NOW).unwrap();
    drop(issuer);

    let repinned = DeploymentBinding {
        runtime: PoolRuntimePins {
            pool: [0x81; 32],
            ..PINS
        },
        ..deployment()
    };
    let reconfigured =
        MeteredIssuer::open(root.path(), policy.clone(), OBSERVERS, repinned).unwrap();

    // A certifier still reporting the old runtime cannot rebind the entitlement.
    let stale = Certifier::new();
    assert_eq!(
        call(&reconfigured, &stale, &feed, &evidence, 0, NOW + 1).unwrap_err(),
        MeteredError::CertificateMismatch
    );

    // A certifier that verified the new pins rebinds the same settlement.
    let current = Certifier::new();
    *current.edit.lock().unwrap() = Some(repin);
    let replay = call(&reconfigured, &current, &feed, &evidence, 0, NOW + 1).unwrap();
    assert!(replay.repeated);
    assert_eq!(current.calls(), 1);
    assert_eq!(
        counts(status(&reconfigured, &current, &evidence, NOW + 1).unwrap()),
        (4, 1, 3)
    );
}

#[test]
fn concurrent_requests_for_one_index_consume_it_once() {
    let (policy, evidence) = feed();
    let root = tempfile::tempdir().unwrap();
    let certifier = Certifier::new();
    status(&open(root.path(), &policy), &certifier, &evidence, NOW).unwrap();
    let fetches = Arc::new(AtomicUsize::new(0));
    let workers = 8;
    let barrier = Arc::new(Barrier::new(workers));
    let handles: Vec<_> = (0..workers)
        .map(|_| {
            // One issuer per worker: only the shared on-disk locks serialize them.
            let issuer = open(root.path(), &policy);
            let evidence = evidence.clone();
            let barrier = Arc::clone(&barrier);
            let fetches = Arc::clone(&fetches);
            std::thread::spawn(move || {
                let request = signed(&evidence, MeteredOperation::Call, 0, NOW);
                let authorized = issuer.authorize(&evidence, &request, NOW).unwrap();
                assert!(!authorized.needs_certificate());
                let source = move |index: u64| -> Result<Vec<u8>, ResourceUnavailable> {
                    let fetch = fetches.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    Ok(format!("call {index} fetch {fetch}").into_bytes())
                };
                barrier.wait();
                match issuer.serve(&authorized, &source, NOW).unwrap() {
                    MeteredReply::Call(call) => call,
                    MeteredReply::Status(_) => panic!("status"),
                }
            })
        })
        .collect();
    let replies: Vec<MeteredCall> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
    assert_eq!(replies.iter().filter(|reply| !reply.repeated).count(), 1);
    assert!(replies
        .iter()
        .all(|reply| same_response(reply, &replies[0])));
    let issuer = open(root.path(), &policy);
    assert_eq!(issuer.calls.records().unwrap().len(), 1);
    assert_eq!(status(&issuer, &certifier, &evidence, NOW).unwrap().next, 1);
    assert_eq!(certifier.calls(), 1);
}

/// Fails the `at`-th durable boundary across both stores, and remembers which stores it saw.
struct Fault {
    at: usize,
    count: Mutex<usize>,
    stores: Mutex<HashSet<(String, erebus_journal::Step)>>,
}

impl Fault {
    fn at(at: usize) -> Arc<Self> {
        Arc::new(Self {
            at,
            count: Mutex::new(0),
            stores: Mutex::new(HashSet::new()),
        })
    }
}

impl FaultHook for Fault {
    fn after(&self, boundary: erebus_journal::Boundary<'_>) -> std::io::Result<()> {
        let directory = boundary
            .path
            .ancestors()
            .find_map(|path| {
                let name = path.file_name()?.to_str()?;
                [ENTITLEMENT_DIR, CALL_DIR]
                    .contains(&name)
                    .then(|| name.to_owned())
            })
            .unwrap_or_default();
        self.stores
            .lock()
            .unwrap()
            .insert((directory, boundary.step));
        let mut count = self.count.lock().unwrap();
        *count += 1;
        if *count == self.at {
            Err(std::io::Error::other("test fault"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn every_write_boundary_of_both_stores_consumes_once_and_never_changes_a_response() {
    use erebus_journal::Step;
    let (policy, evidence) = feed();
    let probe = tempfile::tempdir().unwrap();
    let hook = Fault::at(usize::MAX);
    let issuer = MeteredIssuer::with_faults(
        probe.path(),
        policy.clone(),
        OBSERVERS,
        deployment(),
        hook.clone(),
    )
    .unwrap();
    let certifier = Certifier::new();
    let feed = Feed::new();
    for index in 0..3 {
        call(&issuer, &certifier, &feed, &evidence, index, NOW).unwrap();
    }
    let count = *hook.count.lock().unwrap();
    let seen = hook.stores.lock().unwrap().clone();
    for (store, steps) in [
        (
            ENTITLEMENT_DIR,
            &[
                Step::FileSynced,
                Step::Renamed,
                Step::DirectorySynced,
                Step::RecordWritten,
            ][..],
        ),
        (
            CALL_DIR,
            &[
                Step::FileSynced,
                Step::Renamed,
                Step::DirectorySynced,
                Step::BlobWritten,
                Step::RecordWritten,
            ][..],
        ),
    ] {
        for step in steps {
            assert!(
                seen.contains(&(store.to_owned(), *step)),
                "{store} {step:?}"
            );
        }
    }
    // Activation writes E; each call writes a blob, C(k), and the hint.
    // The two later calls also sync the predecessor's committed response before advancing.
    assert_eq!(count, 4 + 3 * 12 + 2);

    // Each boundary runs in its own directory; spread them over threads to bound wall time.
    let sweep = |at: usize| {
        let root = tempfile::tempdir().unwrap();
        let certifier = Certifier::new();
        let feed = Feed::new();
        let faulty = MeteredIssuer::with_faults(
            root.path(),
            policy.clone(),
            OBSERVERS,
            deployment(),
            Fault::at(at),
        )
        .unwrap();
        let mut returned = Vec::new();
        let mut failed = false;
        for index in 0..3 {
            match call(&faulty, &certifier, &feed, &evidence, index, NOW) {
                Ok(reply) => {
                    assert!(!reply.repeated);
                    returned.push(reply);
                }
                Err(error) => {
                    assert_eq!(error, MeteredError::Access(AccessError::Storage), "at {at}");
                    failed = true;
                    break;
                }
            }
        }
        drop(faulty);

        // Restart without faults.
        let restarted = open(root.path(), &policy);
        let k = u64::try_from(returned.len()).unwrap();
        let next = status(&restarted, &certifier, &evidence, NOW + 1)
            .unwrap()
            .next;
        if failed {
            assert!(
                next == k || next == k + 1,
                "at {at}: consumed {next} after {k} replies"
            );
        } else {
            assert_eq!(next, 3, "at {at}");
        }
        // Every response that was returned is exactly what a retry returns.
        for original in &returned {
            let replay = call(
                &restarted,
                &certifier,
                &feed,
                &evidence,
                original.call_index,
                NOW + 1,
            )
            .unwrap();
            assert!(
                replay.repeated && same_response(original, &replay),
                "at {at}"
            );
        }
        // An ambiguous commit (S5) is served from storage on retry, not consumed again.
        if next == k + 1 && failed {
            let retry = call(&restarted, &certifier, &feed, &evidence, k, NOW + 1).unwrap();
            assert!(retry.repeated, "at {at}");
        }
        for index in next..3 {
            assert!(
                !call(&restarted, &certifier, &feed, &evidence, index, NOW + 1)
                    .unwrap()
                    .repeated
            );
        }
        assert_eq!(
            call(&restarted, &certifier, &feed, &evidence, 3, NOW + 1).unwrap_err(),
            MeteredError::AllowanceExhausted
        );
        assert_eq!(restarted.calls.records().unwrap().len(), 3, "at {at}");
        assert_eq!(
            restarted.entitlements.records().unwrap().len(),
            1,
            "at {at}"
        );
        // Finality is observed again only if the entitlement itself never became durable.
        assert!(certifier.calls() <= 2, "at {at}");
    };
    let next_at = AtomicUsize::new(1);
    std::thread::scope(|scope| {
        for _ in 0..std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) {
            scope.spawn(|| loop {
                let at = next_at.fetch_add(1, Ordering::SeqCst);
                if at > count {
                    break;
                }
                sweep(at);
            });
        }
    });
}

/// Fails the second directory sync of the calls store: the blob syncs first, then the record.
/// The record's rename stays visible but is never made durable, as a power loss would find it.
struct FailCallRecordDirSync {
    seen: Mutex<usize>,
}

impl FaultHook for FailCallRecordDirSync {
    fn after(&self, boundary: erebus_journal::Boundary<'_>) -> std::io::Result<()> {
        let in_calls = boundary
            .path
            .ancestors()
            .any(|path| path.file_name().and_then(|name| name.to_str()) == Some(CALL_DIR));
        if boundary.step == erebus_journal::Step::DirectorySynced && in_calls {
            let mut seen = self.seen.lock().unwrap();
            *seen += 1;
            if *seen == 2 {
                return Err(std::io::Error::other("record rename not durable"));
            }
        }
        Ok(())
    }
}

#[test]
fn a_replay_establishes_directory_durability_before_returning_bytes() {
    let (policy, evidence) = feed();
    let root = tempfile::tempdir().unwrap();
    let certifier = Certifier::new();
    let feed = Feed::new();

    // The C(0) record's write reports an error at its directory-sync boundary after the rename.
    // The record is visible, and whether the physical sync happened is unknowable, so recovery
    // must re-establish durability before it returns bytes.
    let faulty = MeteredIssuer::with_faults(
        root.path(),
        policy.clone(),
        OBSERVERS,
        deployment(),
        Arc::new(FailCallRecordDirSync {
            seen: Mutex::new(0),
        }),
    )
    .unwrap();
    assert_eq!(
        call(&faulty, &certifier, &feed, &evidence, 0, NOW).unwrap_err(),
        MeteredError::Access(AccessError::Storage)
    );
    assert_eq!(feed.fetches(), 1);
    drop(faulty);

    // A retry finds the visible record. If the directory sync cannot be established, the bytes
    // must not be returned: failing the replay's sync yields 503 and consumes nothing. The fault
    // hook runs after the real fsync, so this proves the sync gate, not physical power-loss
    // recovery.
    let undurable = MeteredIssuer::with_faults(
        root.path(),
        policy.clone(),
        OBSERVERS,
        deployment(),
        Fault::at(1),
    )
    .unwrap();
    assert_eq!(
        call(&undurable, &certifier, &feed, &evidence, 0, NOW + 1).unwrap_err(),
        MeteredError::Access(AccessError::Storage)
    );
    assert_eq!(feed.fetches(), 1);
    drop(undurable);

    // Once the sync succeeds, the retry returns the stored bytes as a repeat.
    let restarted = open(root.path(), &policy);
    let replay = call(&restarted, &certifier, &feed, &evidence, 0, NOW + 1).unwrap();
    assert!(replay.repeated);
    assert_eq!(feed.fetches(), 1);
    assert_eq!(restarted.calls.records().unwrap().len(), 1);
    assert_eq!(
        status(&restarted, &certifier, &evidence, NOW + 1)
            .unwrap()
            .next,
        1
    );
}

#[test]
fn metered_hits_and_misses_spend_the_same_request_signature_work() {
    use crate::access::REQUEST_VERIFICATIONS;
    let count = |step: &dyn Fn()| {
        let before = REQUEST_VERIFICATIONS.with(std::cell::Cell::get);
        step();
        REQUEST_VERIFICATIONS.with(std::cell::Cell::get) - before
    };
    let (policy, evidence) = feed();
    let root = tempfile::tempdir().unwrap();
    let issuer = open(root.path(), &policy);
    let now = FAR - 10_000;
    let request = signed(&evidence, MeteredOperation::Call, 0, now);
    let mut other_policy = policy.clone();
    other_policy.resource = "data.other.v1".into();
    let other = open(&root.path().join("other"), &other_policy);
    let mut renamed = request.clone();
    renamed.deal_commitment = "5a".repeat(32);
    let mut forged = request.clone();
    forged.signature[0] ^= 1;
    let rejected =
        |issuer: &MeteredIssuer, evidence: Option<&SelectedAgreement>, request: &MeteredRequest| {
            count(&|| {
                assert_eq!(
                    issuer.authorize_loaded(evidence, request, now).err(),
                    Some(MeteredError::Access(AccessError::Authentication))
                );
            })
        };
    assert_eq!(rejected(&issuer, None, &request), 1, "miss");
    assert_eq!(
        rejected(&other, Some(&evidence), &request),
        1,
        "wrong profile"
    );
    assert_eq!(
        rejected(&issuer, Some(&evidence), &renamed),
        1,
        "evidence under another name"
    );
    assert_eq!(
        rejected(&issuer, Some(&evidence), &forged),
        1,
        "bad signature"
    );
    assert_eq!(
        count(&|| assert!(issuer
            .authorize_loaded(Some(&evidence), &request, now)
            .is_ok())),
        1
    );
    let mut zero = request.clone();
    zero.nonce = [0; 32];
    let mut status = signed(&evidence, MeteredOperation::Status, 0, now);
    status.call_index = 1;
    for shape in [&zero, &status] {
        assert_eq!(rejected(&issuer, None, shape), 0);
        assert_eq!(rejected(&issuer, Some(&evidence), shape), 0);
    }
    // The same signed request after its expiry: refused before any signature check.
    for evidence in [None, Some(&evidence)] {
        assert_eq!(
            count(&|| {
                assert_eq!(
                    issuer.authorize_loaded(evidence, &request, now + 101).err(),
                    Some(MeteredError::Access(AccessError::Authentication))
                );
            }),
            0
        );
    }
}
