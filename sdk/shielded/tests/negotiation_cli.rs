//! Copied native commands negotiate with separate keys and private participant directories.
//! This suite authorizes agreements only. It neither funds nor submits a chain payment.

use erebus_coordinator::Coordinator;
use erebus_core::{
    auth::Role,
    ids::{BaseUnits, KeyBytes},
    policy::SpendingPolicy,
    settlement::SettlementContext,
    shielded::note_spend_tag,
    shielded_auth::derive_key,
    terms::{AgreementTerms, GuaranteeSet, SettlementMode},
};
use erebus_shielded_prover::{
    preparation,
    wallet::{WalletDomain, WalletStore},
};
use erebus_transport::{
    descriptor::{ServiceDescriptor, MODE_PUBLIC_BOUND, MODE_SHIELDED},
    disclosure::{verify_selected_agreement, SelectedAgreement},
    identity::{AuthorizationIdentity, TransportIdentity},
};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const OPERATION: [u8; 32] = [63; 32];

fn native_binary(name: &str, fallback: &str) -> PathBuf {
    match std::env::var_os("EREBUS_TEST_INSTALLED_BIN_DIR") {
        Some(directory) => {
            let directory = PathBuf::from(directory).canonicalize().unwrap();
            let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .canonicalize()
                .unwrap();
            assert!(
                !directory.starts_with(checkout),
                "installed binaries must be outside the checkout"
            );
            let binary = directory.join(name).canonicalize().unwrap();
            assert!(binary.starts_with(&directory) && binary.is_file());
            binary
        }
        None => fallback.into(),
    }
}

fn private(path: &Path, bytes: &[u8]) {
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn clock() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn terms(suite: u16, amount: u128) -> AgreementTerms {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../core/tests/fixtures/agreement-v1-vectors.json"
    ))
    .unwrap();
    let bytes = hex::decode(
        fixture["vectors"][0]["expected"]["canonicalHex"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let mut terms = AgreementTerms::decode(&bytes).unwrap();
    terms.amount = BaseUnits::new(amount);
    terms.transcript_root = [0; 32];
    terms.expiry = clock() + 3600;
    terms.service.delivery_deadline = clock() + 7200;
    if suite == 2 {
        terms.suite_id = 2;
        terms.settlement_mode = SettlementMode::Shielded;
        terms.domain.pool = terms.domain.settlement_contract.clone();
        terms.required_guarantees = GuaranteeSet::from_bits(7).unwrap();
        terms.buyer_authorization_key =
            KeyBytes::new(derive_key(&[1; 32]).unwrap().to_vec()).unwrap();
        terms.seller_authorization_key =
            KeyBytes::new(derive_key(&[2; 32]).unwrap().to_vec()).unwrap();
        terms.service.access_recipient = terms.buyer_authorization_key.clone();
        terms.payment_recipient =
            KeyBytes::new(note_spend_tag(&[3; 32]).unwrap().to_vec()).unwrap();
    }
    terms
}

struct Running(Option<Child>);
impl Drop for Running {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn spawn(binary: &Path, cwd: &Path, request: Value) -> Running {
    let mut child = Command::new(binary)
        .current_dir(cwd)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    Running(Some(child))
}
fn finish(running: Running) -> (bool, Value) {
    finish_with_timeout(running, Duration::from_secs(45))
}

fn finish_with_timeout(mut running: Running, timeout: Duration) -> (bool, Value) {
    let deadline = Instant::now() + timeout;
    loop {
        if running.0.as_mut().unwrap().try_wait().unwrap().is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "native negotiation process stalled"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let output = running.0.take().unwrap().wait_with_output().unwrap();
    assert!(
        output.stderr.is_empty(),
        "native command emitted unexpected diagnostic output"
    );
    let response = serde_json::from_slice(&output.stdout).unwrap();
    (output.status.success(), response)
}

struct Fixture {
    root: tempfile::TempDir,
    binary: PathBuf,
    buyer_config: PathBuf,
    seller_config: PathBuf,
    template: AgreementTerms,
}
impl Fixture {
    fn new(suite: u16, initial_price: u128) -> Self {
        Self::with_terms(terms(suite, initial_price))
    }

    fn with_terms(template: AgreementTerms) -> Self {
        let suite = template.suite_id;
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("erebus-negotiate");
        fs::copy(
            native_binary("erebus-negotiate", env!("CARGO_BIN_EXE_erebus-negotiate")),
            &binary,
        )
        .unwrap();
        let address = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let endpoint = format!("tcp://{address}");
        let mut descriptors = Vec::new();
        for role in [Role::Buyer, Role::Seller] {
            let name = if role == Role::Buyer {
                "buyer"
            } else {
                "seller"
            };
            let path = root.path().join(name);
            fs::create_dir(&path).unwrap();
            let identity = AuthorizationIdentity::from_bytes(&[20 + role.tag(); 32]).unwrap();
            let transport = TransportIdentity::from_private_key([40 + role.tag(); 32]).unwrap();
            transport.store(path.join("transport.key")).unwrap();
            private(&path.join("discovery.key"), &[20 + role.tag(); 32]);
            private(
                &path.join("agreement.key"),
                &if suite == 1 {
                    [20 + role.tag(); 32]
                } else {
                    [60 + role.tag(); 32]
                },
            );
            let mut descriptor = ServiceDescriptor::new(
                identity.address(),
                transport.public_key(),
                vec![endpoint.clone()],
                template.domain.namespace.clone(),
                vec![template.asset.clone()],
                vec![suite],
                if suite == 1 {
                    MODE_PUBLIC_BOUND
                } else {
                    MODE_SHIELDED
                },
                template.required_guarantees,
                clock() - 1,
                clock() + 7200,
            )
            .unwrap();
            descriptor.sign(&identity).unwrap();
            private(
                &root.path().join(format!("{name}.json")),
                descriptor.to_json().unwrap().as_bytes(),
            );
            descriptors.push(descriptor);
        }
        let _ = descriptors;
        private(&root.path().join("seller/spend.key"), &[4; 32]);
        private(&root.path().join("seller/wallet.key"), &[5; 32]);
        for role in [Role::Buyer, Role::Seller] {
            let name = if role == Role::Buyer {
                "buyer"
            } else {
                "seller"
            };
            let peer = if role == Role::Buyer {
                "seller"
            } else {
                "buyer"
            };
            let path = root.path().join(name);
            let mut local_terms = template.clone();
            if role == Role::Seller {
                local_terms.amount = BaseUnits::new(70);
            }
            private(&path.join("service.terms"), &local_terms.encode().unwrap());
            private(&path.join("config.json"),json!({
                "version":1,"role":name,"state_root":path.join("state"),
                "transport_key_file":path.join("transport.key"),"agreement_key_file":path.join("agreement.key"),
                "discovery_key_file":path.join("discovery.key"),
                "local_descriptor_file":root.path().join(format!("{name}.json")),"peer_descriptor_file":root.path().join(format!("{peer}.json")),
                "terms_template_file":path.join("service.terms"),"endpoint":address,
                "maximum_price":"75","minimum_price":"65","max_deal_lifetime_seconds":3600,"timeout_seconds":15,
                "seller_spend_secret_file":if role==Role::Seller {Some(path.join("spend.key"))} else {None},
                "seller_wallet_file":if role==Role::Seller {Some(path.join("state/wallet/notes.enc"))} else {None},
                "seller_wallet_key_file":if role==Role::Seller {Some(path.join("wallet.key"))} else {None}
            }).to_string().as_bytes());
        }
        Self {
            binary,
            buyer_config: root.path().join("buyer/config.json"),
            seller_config: root.path().join("seller/config.json"),
            root,
            template,
        }
    }

    fn pair(&self, freeze: bool) -> (Value, Value) {
        let request = |config| json!({"method":"negotiate","config_file":config,"operation_ref":hex::encode(OPERATION),"freeze_only":freeze});
        let seller = spawn(&self.binary, self.root.path(), request(&self.seller_config));
        let buyer = spawn(&self.binary, self.root.path(), request(&self.buyer_config));
        let (buyer_ok, buyer) = finish(buyer);
        let (seller_ok, seller) = finish(seller);
        assert!(
            buyer_ok && seller_ok,
            "suite {} price {} freeze {freeze}: buyer {buyer}; seller {seller}",
            self.template.suite_id,
            self.template.amount.get()
        );
        (buyer, seller)
    }
}

#[path = "support/payment_driver.rs"]
mod payment_driver;

#[test]
fn copied_commands_freeze_restart_authorize_and_retain_recoverable_seller_notes_in_both_modes() {
    for suite in [1, 2] {
        for initial_price in [60, 65] {
            let fixture = Fixture::new(suite, initial_price);
            let publication = fixture.root.path().join("seller/access-evidence");
            fs::create_dir(&publication).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&publication, fs::Permissions::from_mode(0o700)).unwrap();
            }
            let mut config: Value =
                serde_json::from_slice(&fs::read(&fixture.seller_config).unwrap()).unwrap();
            config["access_evidence_root"] = json!(publication);
            fs::write(&fixture.seller_config, config.to_string()).unwrap();
            let (buyer_frozen, seller_frozen) = fixture.pair(true);
            assert_eq!(buyer_frozen["status"], "frozen");
            assert_eq!(
                buyer_frozen["deal_commitment"],
                seller_frozen["deal_commitment"]
            );
            assert_eq!(buyer_frozen["payment_verified"], false);
            let (buyer, seller) = fixture.pair(false);
            assert_eq!(buyer["status"], "authorized");
            assert_eq!(buyer["agreement_verified"], true);
            assert_eq!(buyer["deal_commitment"], buyer_frozen["deal_commitment"]);
            assert_eq!(buyer["deal_commitment"], seller["deal_commitment"]);
            let published = publication.join(format!(
                "{}.evidence",
                seller["deal_commitment"].as_str().unwrap()
            ));
            assert_eq!(
                fs::read(&published).unwrap(),
                fs::read(seller["evidence_file"].as_str().unwrap()).unwrap()
            );
            for response in [&buyer, &seller] {
                assert_eq!(response["payment_verified"], false);
                assert_eq!(response["delivery_verified"], false);
                for private_field in [
                    "amount",
                    "blinding",
                    "signature",
                    "seed",
                    "terms",
                    "rpc_url",
                ] {
                    assert!(response.get(private_field).is_none());
                }
            }
            let bytes = fs::read(buyer["evidence_file"].as_str().unwrap()).unwrap();
            let evidence = SelectedAgreement::decode(&bytes).unwrap();
            verify_selected_agreement(&evidence).unwrap();
            let accepted = if initial_price == 60 { 70 } else { 65 };
            assert_eq!(evidence.terms.amount.get(), accepted);
            assert_eq!(
                evidence.messages.len(),
                if initial_price == 60 { 4 } else { 3 }
            );
            let context = SettlementContext {
                require_local_proving: true,
                mode: evidence.terms.settlement_mode,
                domain: evidence.terms.domain.clone(),
                suite_id: suite,
                asset: evidence.terms.asset.clone(),
                required_guarantees: evidence.terms.required_guarantees,
            };
            let capabilities = if suite == 1 {
                erebus_evm::backend::public_bound_capabilities()
            } else {
                preparation::capabilities()
            };
            let coordinator = Coordinator::open(
                fixture.root.path().join("buyer/state/coordinator"),
                evidence.terms.buyer_authorization_key.clone(),
                context.clone(),
                &context,
                &capabilities,
                SpendingPolicy {
                    per_deal_max: BaseUnits::new(75),
                    allowed_assets: [evidence.terms.asset.clone()].into(),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(coordinator.ledger().unwrap().reservations().len(), 1);
            assert_eq!(
                coordinator
                    .ledger()
                    .unwrap()
                    .reserved_total(&evidence.terms.asset)
                    .unwrap()
                    .get(),
                accepted
            );
            assert_eq!(coordinator.diagnostics().unwrap()[0].broadcast_attempts, 0);
            if suite == 2 {
                let payment_config = fixture.root.path().join("buyer/no-downgrade.json");
                private(&payment_config,json!({"version":1,"state_root":fixture.root.path().join("buyer/state"),
                    "namespace":evidence.terms.domain.namespace.to_string(),"settlement_contract":format!("0x{}",hex::encode(evidence.terms.domain.settlement_contract.as_ref().unwrap().as_bytes())),
                    "verifier_version":evidence.terms.domain.verifier_version,"runtime_keccak256":"01".repeat(32),"first_block":0,"first_hash":"02".repeat(32),
                    "rpc_url":"http://127.0.0.1:1","peer_rpc_url":"http://127.0.0.1:2","buyer_address":format!("0x{}",hex::encode(AuthorizationIdentity::from_bytes(&[21;32]).unwrap().address())),
                    "asset":evidence.terms.asset.to_string(),"signer_address":"0x1111111111111111111111111111111111111111",
                    "signer_journal_root":fixture.root.path().join("unused-signer"),"transaction_key_file":null,
                    "maximum_price":"75","gas_limit":21_000,"max_fee_per_gas":"1","max_priority_fee_per_gas":"0",
                    "timeout_seconds":1,"log_block_range":100,"max_log_queries":1,"max_ancestry":1}).to_string().as_bytes());
                let (ok, rejected) = finish(spawn(
                    Path::new(env!("CARGO_BIN_EXE_erebus-payment")),
                    fixture.root.path(),
                    json!({"method":"settle","config_file":payment_config,"operation_ref":hex::encode(OPERATION)}),
                ));
                assert!(!ok);
                assert_eq!(
                    rejected["error"],
                    "this driver does not support shielded settlement; no downgrade is allowed"
                );
                assert_eq!(coordinator.diagnostics().unwrap()[0].broadcast_attempts, 0);
                assert!(!fixture.root.path().join("unused-signer").exists());
                assert_eq!(
                    evidence.terms.buyer_authorization_key.as_bytes(),
                    derive_key(&[61; 32]).unwrap()
                );
                assert_eq!(
                    evidence.terms.seller_authorization_key.as_bytes(),
                    derive_key(&[62; 32]).unwrap()
                );
                assert_eq!(
                    evidence.terms.payment_recipient.as_bytes(),
                    note_spend_tag(&[4; 32]).unwrap()
                );
                assert_ne!(
                    evidence.terms.buyer_authorization_key,
                    fixture.template.buyer_authorization_key
                );
                let domain = WalletDomain {
                    chain_id: 10143,
                    pool: evidence
                        .terms
                        .domain
                        .pool
                        .as_ref()
                        .unwrap()
                        .as_bytes()
                        .try_into()
                        .unwrap(),
                };
                let wallet = WalletStore::new(
                    fixture.root.path().join("seller/state/wallet/notes.enc"),
                    domain,
                    [5; 32],
                )
                .unwrap();
                assert_eq!(wallet.snapshot().unwrap().notes().len(), 1);
                assert_eq!(wallet.snapshot().unwrap().notes()[0].amount(), accepted);
                assert!(wallet.snapshot().unwrap().notes()[0].inclusion().is_none());
            }
            let (retried, _) = fixture.pair(false);
            assert_eq!(retried["deal_commitment"], buyer["deal_commitment"]);
            assert_eq!(
                fs::read(retried["evidence_file"].as_str().unwrap()).unwrap(),
                bytes
            );
            assert_eq!(coordinator.ledger().unwrap().reservations().len(), 1);
        }
    }
}

#[test]
fn protocol_and_rejections_do_not_echo_private_inputs_or_claim_chain_success() {
    let root = tempfile::tempdir().unwrap();
    let binary = Path::new(env!("CARGO_BIN_EXE_erebus-negotiate"));
    let (ok, version) = finish(spawn(binary, root.path(), json!({"method":"version"})));
    assert!(ok);
    assert_eq!(version["settlement_submission"], false);
    for request in [
        json!({"method":"negotiate","config_file":root.path().join("PRIVATE_CONFIG"),"operation_ref":"ab".repeat(32)}),
        json!({"method":"version","seed":"PRIVATE_KEY_NEVER_ECHO"}),
        json!({"method":"negotiate","config_file":"PRIVATE_CONFIG","operation_ref":"00".repeat(32)}),
    ] {
        let (ok, response) = finish(spawn(binary, root.path(), request));
        assert!(!ok);
        assert_eq!(response["payment_verified"], false);
        assert!(!response.to_string().contains("PRIVATE_"));
    }
}

#[test]
fn payment_protocol_rejects_unknown_fields_without_echoing_operator_inputs() {
    let root = tempfile::tempdir().unwrap();
    let binary = Path::new(env!("CARGO_BIN_EXE_erebus-payment"));
    let (ok, version) = finish(spawn(binary, root.path(), json!({"method":"version"})));
    assert!(ok);
    assert_eq!(version["automatic_rebroadcast"], false);
    assert_eq!(version["modes"], json!(["public-bound", "shielded"]));
    for request in [
        json!({"method":"version","seed":"PRIVATE_KEY_NEVER_ECHO"}),
        json!({"method":"settle","config_file":"PRIVATE_CONFIG","operation_ref":"ab".repeat(32),"amount":"999"}),
        json!({"method":"observe","config_file":"PRIVATE_CONFIG","operation_ref":"ab".repeat(32)}),
    ] {
        let (ok, response) = finish(spawn(binary, root.path(), request));
        assert!(!ok);
        assert_eq!(response["payment_verified"], false);
        assert_eq!(response["retry_without_new_payment"], true);
        assert!(!response.to_string().contains("PRIVATE_"));
    }
}

#[cfg(unix)]
#[test]
fn unsafe_operator_config_is_rejected_before_connecting_or_creating_state() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let fixture = Fixture::new(1, 60);
    let link = fixture.root.path().join("config.link");
    symlink(&fixture.buyer_config, &link).unwrap();
    for path in [&link, &fixture.buyer_config] {
        if path == &fixture.buyer_config {
            fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
        }
        let (ok, _) = finish(spawn(
            &fixture.binary,
            fixture.root.path(),
            json!({"method":"negotiate","config_file":path,"operation_ref":hex::encode(OPERATION)}),
        ));
        assert!(!ok);
        assert!(!fixture.root.path().join("buyer/state").exists());
    }
}

#[test]
fn operator_prepared_participants_negotiate_without_test_fixtures() {
    // Only product requests: no test keys, descriptors, or terms are constructed here.
    let root = tempfile::tempdir().unwrap();
    let binary = native_binary("erebus-negotiate", env!("CARGO_BIN_EXE_erebus-negotiate"));
    let endpoint = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let asset = "eip155:10143/erc20:0x902f79145059910ef875aecf4187c771b204ea14";
    let run = |request: Value| finish(spawn(&binary, root.path(), request));
    let mut prepared = std::collections::HashMap::new();
    for role in ["buyer", "seller"] {
        let (ok, response) = run(
            json!({"method":"prepare_operator","directory":root.path().join(role),
            "role":role,"endpoint":endpoint,"namespace":"eip155:10143","assets":[asset],
            "descriptor_lifetime_seconds":86400}),
        );
        assert!(ok, "{response}");
        assert!(
            response.get("agreement_key").is_none() && !response.to_string().contains("secret")
        );
        prepared.insert(role, response);
    }
    let (ok, again) = run(
        json!({"method":"prepare_operator","directory":root.path().join("buyer"),
        "role":"buyer","endpoint":endpoint,"namespace":"eip155:10143","assets":[asset],"descriptor_lifetime_seconds":86400}),
    );
    assert!(
        !ok,
        "an existing operator directory is never overwritten: {again}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [
            root.path().join("buyer"),
            root.path().join("buyer/agreement.key"),
        ] {
            assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o077, 0);
        }
    }
    let delivery = clock() + 7200;
    let payload_digest = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(
        b"live rehearsal snapshot",
    ));
    for (role, peer, price) in [("buyer", "seller", "60"), ("seller", "buyer", "70")] {
        let directory = root.path().join(role);
        // Public descriptors travel between operators; nothing private does.
        fs::copy(
            prepared[peer]["descriptor_file"].as_str().unwrap(),
            directory.join(format!("{peer}.descriptor.json")),
        )
        .unwrap();
        let (ok, terms) = run(
            json!({"method":"prepare_terms","output":directory.join("service.terms"),"role":role,
            "local_descriptor_file":prepared[role]["descriptor_file"],"peer_descriptor_file":directory.join(format!("{peer}.descriptor.json")),
            "settlement_contract":"0xa5f0c864f434331bef9a7fc5e05450d598d24da4","verifier_version":1,"asset":asset,
            "amount":price,"resource":"dataset.snapshot.v1","unit":"snapshot","quantity":"1",
            "fulfillment_method":"http-access-v1","fulfillment_digest":payload_digest,"delivery_deadline":delivery}),
        );
        assert!(ok, "{terms}");
        assert_eq!(terms["buyer"], prepared["buyer"]["agreement_address"]);
        assert_eq!(
            terms["payment_recipient"],
            prepared["seller"]["agreement_address"]
        );
        private(&directory.join("config.json"), json!({"version":1,"role":role,"state_root":directory.join("state"),
            "transport_key_file":directory.join("transport.key"),"agreement_key_file":directory.join("agreement.key"),
            "discovery_key_file":null,"local_descriptor_file":prepared[role]["descriptor_file"],
            "peer_descriptor_file":directory.join(format!("{peer}.descriptor.json")),
            "terms_template_file":directory.join("service.terms"),"endpoint":endpoint,
            "maximum_price":"75","minimum_price":"65","max_deal_lifetime_seconds":3600,"timeout_seconds":30,
            "seller_spend_secret_file":null,"seller_wallet_file":null,"seller_wallet_key_file":null})
            .to_string().as_bytes());
    }
    // A forged peer descriptor and a shared agreement key are both refused.
    let forged = root.path().join("forged.json");
    let mut tampered: Value = serde_json::from_slice(
        &fs::read(prepared["seller"]["descriptor_file"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    tampered["endpoints"] = json!(["tcp://203.0.113.9:1"]);
    fs::write(&forged, tampered.to_string()).unwrap();
    for peer in [
        forged,
        PathBuf::from(prepared["buyer"]["descriptor_file"].as_str().unwrap()),
    ] {
        let (ok, refused) = run(
            json!({"method":"prepare_terms","output":root.path().join("refused.terms"),"role":"buyer",
            "local_descriptor_file":prepared["buyer"]["descriptor_file"],"peer_descriptor_file":peer,
            "settlement_contract":"0xa5f0c864f434331bef9a7fc5e05450d598d24da4","verifier_version":1,"asset":asset,
            "amount":"60","resource":"dataset.snapshot.v1","unit":"snapshot","quantity":"1",
            "fulfillment_method":"http-access-v1","fulfillment_digest":payload_digest,"delivery_deadline":delivery}),
        );
        assert!(!ok, "{refused}");
    }
    assert!(!root.path().join("refused.terms").exists());

    let request = |role: &str| {
        json!({"method":"negotiate","config_file":root.path().join(role).join("config.json"),
        "operation_ref":hex::encode(OPERATION)})
    };
    let seller = spawn(&binary, root.path(), request("seller"));
    let buyer = spawn(&binary, root.path(), request("buyer"));
    let (buyer_ok, buyer) = finish(buyer);
    let (seller_ok, seller) = finish(seller);
    assert!(buyer_ok && seller_ok, "buyer {buyer}; seller {seller}");
    assert_eq!(buyer["status"], "authorized");
    assert_eq!(buyer["deal_commitment"], seller["deal_commitment"]);
    let selected =
        SelectedAgreement::decode(&fs::read(buyer["evidence_file"].as_str().unwrap()).unwrap())
            .unwrap();
    verify_selected_agreement(&selected).unwrap();
    assert_eq!(
        selected.terms.amount.get(),
        70,
        "the seller's counter within the buyer's maximum"
    );
    assert_eq!(
        format!(
            "0x{}",
            hex::encode(selected.terms.buyer_authorization_key.as_bytes())
        ),
        prepared["buyer"]["agreement_address"]
    );
}
