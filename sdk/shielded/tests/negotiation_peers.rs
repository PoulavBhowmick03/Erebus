//! One seller with a version 2 peer table negotiates, one operation at a time, with several
//! independently keyed buyers over the routed handshake. Agreements only: nothing is paid.

use erebus_core::{
    auth::Role,
    ids::BaseUnits,
    terms::{AgreementTerms, SettlementMode},
};
use erebus_transport::{
    descriptor::{ServiceDescriptor, MODE_PUBLIC_BOUND},
    identity::{AuthorizationIdentity, TransportIdentity},
};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn clock() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn private(path: &Path, bytes: &[u8]) {
    let _ = fs::remove_file(path);
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

fn private_dir(path: &Path) {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.recursive(true).create(path).unwrap();
}

fn template(resource: &str, amount: u128) -> AgreementTerms {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../core/tests/fixtures/agreement-v1-vectors.json"
    ))
    .unwrap();
    let mut terms = AgreementTerms::decode(
        &hex::decode(
            fixture["vectors"][0]["expected"]["canonicalHex"]
                .as_str()
                .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(terms.settlement_mode, SettlementMode::PublicBound);
    terms.amount = BaseUnits::new(amount);
    terms.transcript_root = [0; 32];
    terms.expiry = clock() + 3600;
    terms.service.delivery_deadline = clock() + 7200;
    terms.service.resource = resource.into();
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

fn spawn(binary: &Path, request: Value) -> Running {
    let mut child = Command::new(binary)
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

fn finish(mut running: Running) -> (bool, Value) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while running.0.as_mut().unwrap().try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "native process stalled");
        thread::sleep(Duration::from_millis(10));
    }
    let output = running.0.take().unwrap().wait_with_output().unwrap();
    assert!(output.stderr.is_empty(), "unexpected diagnostic output");
    (
        output.status.success(),
        serde_json::from_slice(&output.stdout).unwrap(),
    )
}

/// One participant's keys and signed descriptor. Suite 1: the agreement key signs discovery.
struct Party {
    directory: PathBuf,
    descriptor: ServiceDescriptor,
}

fn party(root: &Path, name: &str, seed: u8, endpoint: SocketAddr, terms: &AgreementTerms) -> Party {
    let directory = root.join(name);
    private_dir(&directory);
    let identity = AuthorizationIdentity::from_bytes(&[seed; 32]).unwrap();
    let transport = TransportIdentity::from_private_key([seed + 100; 32]).unwrap();
    transport.store(directory.join("transport.key")).unwrap();
    private(&directory.join("agreement.key"), &[seed; 32]);
    let mut descriptor = ServiceDescriptor::new(
        identity.address(),
        transport.public_key(),
        vec![format!("tcp://{endpoint}")],
        terms.domain.namespace.clone(),
        vec![terms.asset.clone()],
        vec![1],
        MODE_PUBLIC_BOUND,
        terms.required_guarantees,
        clock() - 1,
        clock() + 7200,
    )
    .unwrap();
    descriptor.sign(&identity).unwrap();
    private(
        &directory.join("descriptor.json"),
        descriptor.to_json().unwrap().as_bytes(),
    );
    Party {
        directory,
        descriptor,
    }
}

struct World {
    root: tempfile::TempDir,
    binary: PathBuf,
    seller: Party,
    endpoint: SocketAddr,
}

/// Buyer `b1` offers 60 for `data.snapshot.v1` (seller floor 65, counter 70); buyer `b2` offers
/// 90 for `data.other.v1` (floor 85), which the seller accepts.
const BUYERS: [(&str, u8, &str, u128, u128, u128); 2] = [
    ("b1", 21, "data.snapshot.v1", 60, 65, 70),
    ("b2", 31, "data.other.v1", 90, 85, 90),
];

impl World {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("erebus-negotiate");
        fs::copy(env!("CARGO_BIN_EXE_erebus-negotiate"), &binary).unwrap();
        let endpoint = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let reference = template("data.snapshot.v1", 70);
        let seller = party(root.path(), "seller", 22, endpoint, &reference);
        let world = Self {
            root,
            binary,
            seller,
            endpoint,
        };
        private_dir(&world.peers());
        for (name, seed, resource, offer, floor, price) in BUYERS {
            let buyer = party(world.root.path(), name, seed, endpoint, &reference);
            let peer = world.peers().join(name);
            private_dir(&peer);
            private(
                &peer.join("descriptor.json"),
                buyer.descriptor.to_json().unwrap().as_bytes(),
            );
            private(
                &peer.join("service.terms"),
                &template(resource, price).encode().unwrap(),
            );
            world.write_peer(name, resource, floor, true, false);
            private(
                &buyer.directory.join("service.terms"),
                &template(resource, offer).encode().unwrap(),
            );
            world.write_buyer(name, Some(2));
        }
        world.write_seller(15);
        world
    }

    fn peers(&self) -> PathBuf {
        self.seller.directory.join("peers")
    }

    fn write_peer(&self, name: &str, resource: &str, floor: u128, enabled: bool, legacy: bool) {
        self.write_peer_until(name, resource, floor, enabled, legacy, clock() + 3600);
    }

    #[allow(clippy::too_many_arguments)]
    fn write_peer_until(
        &self,
        name: &str,
        resource: &str,
        floor: u128,
        enabled: bool,
        legacy: bool,
        expires_at: u64,
    ) {
        private(
            &self.peers().join(name).join("peer.json"),
            json!({"version":1,"id":name,"descriptor_file":"descriptor.json",
                "terms_template_file":"service.terms","minimum_price":floor.to_string(),
                "max_deal_lifetime_seconds":3600,"resource":resource,"expires_at":expires_at,
                "enabled":enabled,"legacy_transport":legacy})
            .to_string()
            .as_bytes(),
        );
    }

    fn write_seller(&self, timeout: u64) {
        let directory = &self.seller.directory;
        private(
            &directory.join("config.json"),
            json!({"version":2,"role":"seller","state_root":directory.join("state"),
                "transport_key_file":directory.join("transport.key"),
                "agreement_key_file":directory.join("agreement.key"),"discovery_key_file":null,
                "local_descriptor_file":directory.join("descriptor.json"),"peers_root":self.peers(),
                "endpoint":self.endpoint,"maximum_price":"1000","timeout_seconds":timeout,
                "seller_spend_secret_file":null,"seller_wallet_file":null,"seller_wallet_key_file":null})
            .to_string()
            .as_bytes(),
        );
    }

    fn write_buyer(&self, name: &str, transport: Option<u16>) {
        let directory = self.root.path().join(name);
        let mut config = json!({"version":1,"role":"buyer","state_root":directory.join("state"),
            "transport_key_file":directory.join("transport.key"),
            "agreement_key_file":directory.join("agreement.key"),"discovery_key_file":null,
            "local_descriptor_file":directory.join("descriptor.json"),
            "peer_descriptor_file":self.seller.directory.join("descriptor.json"),
            "terms_template_file":directory.join("service.terms"),"endpoint":self.endpoint,
            "maximum_price":"100","minimum_price":"1","max_deal_lifetime_seconds":3600,
            "timeout_seconds":15,"seller_spend_secret_file":null,"seller_wallet_file":null,
            "seller_wallet_key_file":null});
        if let Some(transport) = transport {
            config["negotiation_transport"] = json!(transport);
        }
        private(
            &directory.join("config.json"),
            config.to_string().as_bytes(),
        );
    }

    fn seller(&self, operation: u8, peer: Option<&str>) -> Running {
        let mut request = json!({"method":"negotiate","config_file":self.seller.directory.join("config.json"),
            "operation_ref":hex::encode([operation; 32])});
        if let Some(peer) = peer {
            request["peer"] = json!(peer);
        }
        spawn(&self.binary, request)
    }

    fn buyer(&self, name: &str, operation: u8) -> Running {
        spawn(
            &self.binary,
            json!({"method":"negotiate","config_file":self.root.path().join(name).join("config.json"),
                "operation_ref":hex::encode([operation; 32])}),
        )
    }

    fn transcripts(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.seller.directory.join("state/transcripts"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
}

#[test]
fn two_buyers_start_together_and_finish_one_after_the_other() {
    let world = World::new();
    let first_seller = world.seller(1, None);
    let mut buyers: Vec<(&str, u8, Running)> = vec![
        ("b1", 11, world.buyer("b1", 11)),
        ("b2", 12, world.buyer("b2", 12)),
    ];
    let (ok, first) = finish(first_seller);
    assert!(ok, "{first}");
    let first_peer = first["peer"].as_str().unwrap().to_owned();
    // The other buyer was refused or never accepted while the seller was busy; it retries.
    let second_seller = world.seller(2, None);
    let mut agreed = std::collections::HashMap::new();
    for (name, operation, running) in buyers.drain(..) {
        let (ok, reply) = finish(running);
        let reply = if ok {
            reply
        } else {
            assert_ne!(name, first_peer, "{reply}");
            let (ok, reply) = finish(world.buyer(name, operation));
            assert!(ok, "{name}: {reply}");
            reply
        };
        agreed.insert(name, reply);
    }
    let (ok, second) = finish(second_seller);
    assert!(ok, "{second}");
    let mut peers = [first_peer, second["peer"].as_str().unwrap().to_owned()];
    peers.sort();
    assert_eq!(peers, ["b1", "b2"]);
    for seller in [&first, &second] {
        let name = seller["peer"].as_str().unwrap();
        assert_eq!(
            seller["deal_commitment"], agreed[name]["deal_commitment"],
            "{name}"
        );
        assert!(
            agreed[name].get("peer").is_none(),
            "a buyer reports no peer"
        );
    }
    assert_eq!(world.transcripts(), ["peer-b1", "peer-b2"]);
    // Each buyer's agreement carries its own peer's price and resource.
    for (name, _, resource, _, _, price) in BUYERS {
        let evidence = fs::read(agreed[name]["evidence_file"].as_str().unwrap()).unwrap();
        let evidence = erebus_transport::disclosure::SelectedAgreement::decode(&evidence).unwrap();
        assert_eq!(evidence.terms.service.resource, resource);
        assert_eq!(evidence.terms.amount.get(), price);
    }
}

#[test]
fn a_named_or_bound_operation_accepts_only_its_own_peer() {
    let world = World::new();
    // The seller waits for b2; b1 connects first and is closed, then b2 binds the operation.
    let seller = world.seller(3, Some("b2"));
    let refused = world.buyer("b1", 13);
    let (ok, refused) = finish(refused);
    assert!(!ok, "{refused}");
    let (buyer_ok, buyer) = finish(world.buyer("b2", 14));
    let (ok, reply) = finish(seller);
    assert!(ok && buyer_ok, "{reply} {buyer}");
    assert_eq!(reply["peer"], "b2");
    let record: Value = serde_json::from_slice(
        &fs::read(
            world
                .seller
                .directory
                .join(format!("state/agent/{}.json", hex::encode([3u8; 32]))),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (record["version"].clone(), record["peer"].clone()),
        (json!(2), json!("b2"))
    );
    // A retry of that operation for another peer is refused before listening.
    let started = Instant::now();
    let (ok, other) = finish(world.seller(3, Some("b1")));
    assert!(!ok);
    assert_eq!(
        other["error"],
        "operation is bound to another negotiation peer"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    let (ok, unknown) = finish(world.seller(4, Some("b9")));
    assert!(!ok);
    assert_eq!(unknown["error"], "negotiation peer is not configured");
    world.write_peer("b1", "data.snapshot.v1", 65, false, false);
    let (ok, disabled) = finish(world.seller(4, Some("b1")));
    assert!(!ok);
    assert_eq!(disabled["error"], "negotiation peer is expired or disabled");
}

#[test]
fn a_seller_with_no_connecting_peer_reports_no_peer_connected() {
    let world = World::new();
    world.write_seller(1);
    let (ok, reply) = finish(world.seller(5, None));
    assert!(!ok);
    assert_eq!(
        reply["error"],
        "no configured peer connected before the deadline"
    );
    assert!(!world
        .seller
        .directory
        .join(format!("state/agent/{}.json", hex::encode([5u8; 32])))
        .exists());
}

#[test]
fn a_direct_buyer_connects_only_as_the_legacy_peer() {
    let world = World::new();
    world.write_seller(3);
    world.write_buyer("b1", None);
    world.write_buyer("b2", None);
    let seller = world.seller(6, None);
    let (ok, buyer) = finish(world.buyer("b1", 16));
    assert!(!ok, "{buyer}");
    let (ok, reply) = finish(seller);
    assert!(!ok);
    assert_eq!(
        reply["error"],
        "no configured peer connected before the deadline"
    );
    world.write_peer("b1", "data.snapshot.v1", 65, true, true);
    world.write_seller(15);
    let seller = world.seller(6, None);
    let (buyer_ok, buyer) = finish(world.buyer("b1", 16));
    let (ok, reply) = finish(seller);
    assert!(ok && buyer_ok, "{reply} {buyer}");
    assert_eq!(reply["peer"], "b1");
    // A legacy peer keeps the version 1 transcript namespace.
    assert_eq!(world.transcripts(), ["agent"]);
    // Two legacy peers would be ambiguous: the configuration is refused.
    world.write_peer("b2", "data.other.v1", 85, true, true);
    let (ok, reply) = finish(world.seller(7, None));
    assert!(!ok);
    assert_eq!(reply["error"], "invalid operator configuration");
}

#[test]
fn list_peers_reports_policy_without_keys() {
    let world = World::new();
    world.write_peer("b2", "data.other.v1", 85, false, false);
    let (ok, listed) = finish(spawn(
        &world.binary,
        json!({"method":"list_peers","config_file":world.seller.directory.join("config.json")}),
    ));
    assert!(ok, "{listed}");
    let peers = listed["peers"].as_array().unwrap();
    assert_eq!(peers.len(), 2);
    assert_eq!(
        (&peers[0]["peer"], &peers[0]["live"], &peers[0]["price"]),
        (&json!("b1"), &json!(true), &json!("70"))
    );
    assert_eq!(
        (&peers[1]["peer"], &peers[1]["enabled"], &peers[1]["live"]),
        (&json!("b2"), &json!(false), &json!(false))
    );
    // Policy fields only: no keys, descriptors, or paths.
    let mut fields: Vec<&String> = peers[0].as_object().unwrap().keys().collect();
    fields.sort();
    assert_eq!(
        fields,
        [
            "enabled",
            "expired",
            "expires_at",
            "legacy_transport",
            "live",
            "minimum_price",
            "peer",
            "price",
            "resource"
        ]
    );
}

#[test]
fn invalid_peer_tables_are_refused() {
    let world = World::new();
    let config = world.seller.directory.join("config.json");
    let negotiate = |world: &World| finish(world.seller(8, None));
    // A peer directory whose name is not a peer id.
    private_dir(&world.peers().join("Bad_Name"));
    assert_eq!(
        negotiate(&world).1["error"],
        "invalid operator configuration"
    );
    fs::remove_dir(world.peers().join("Bad_Name")).unwrap();
    // A staging directory is ignored.
    private_dir(&world.peers().join(".staging-b3"));
    // Two enabled peers with one transport key.
    let b2 = world.peers().join("b2");
    fs::copy(
        world.peers().join("b1/descriptor.json"),
        b2.join("descriptor.json"),
    )
    .unwrap();
    assert_eq!(
        negotiate(&world).1["error"],
        "invalid operator configuration"
    );
    // A file name escaping the peer directory.
    private(
        &b2.join("peer.json"),
        json!({"version":1,"id":"b2","descriptor_file":"../b1/descriptor.json",
            "terms_template_file":"service.terms","minimum_price":"85",
            "max_deal_lifetime_seconds":3600,"resource":"data.other.v1","expires_at":clock()+3600,
            "enabled":true,"legacy_transport":false})
        .to_string()
        .as_bytes(),
    );
    assert_eq!(
        negotiate(&world).1["error"],
        "invalid operator configuration"
    );
    // A buyer may not carry a peer table.
    let mut seller: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    seller["role"] = json!("buyer");
    private(&config, seller.to_string().as_bytes());
    assert!(!negotiate(&world).0);
    let _ = Role::Buyer;
}

#[test]
fn a_version_1_operation_resumes_under_a_version_2_table_as_the_legacy_peer() {
    let world = World::new();
    let directory = &world.seller.directory;
    let config = directory.join("config.json");
    // The single-buyer configuration, with b1's descriptor and promise.
    private(
        &config,
        json!({"version":1,"role":"seller","state_root":directory.join("state"),
            "transport_key_file":directory.join("transport.key"),
            "agreement_key_file":directory.join("agreement.key"),"discovery_key_file":null,
            "local_descriptor_file":directory.join("descriptor.json"),
            "peer_descriptor_file":world.peers().join("b1/descriptor.json"),
            "terms_template_file":world.peers().join("b1/service.terms"),"endpoint":world.endpoint,
            "maximum_price":"1000","minimum_price":"65","max_deal_lifetime_seconds":3600,
            "timeout_seconds":15,"seller_spend_secret_file":null,"seller_wallet_file":null,
            "seller_wallet_key_file":null})
        .to_string()
        .as_bytes(),
    );
    world.write_buyer("b1", None);
    let freeze = |config: &Path, operation: u8| {
        spawn(
            &world.binary,
            json!({"method":"negotiate","config_file":config,"operation_ref":hex::encode([operation; 32]),"freeze_only":true}),
        )
    };
    let seller = freeze(&config, 9);
    let (buyer_ok, buyer) = finish(freeze(&world.root.path().join("b1/config.json"), 19));
    let (ok, frozen) = finish(seller);
    assert!(ok && buyer_ok, "{frozen} {buyer}");
    assert!(
        frozen.get("peer").is_none(),
        "a version 1 seller reports no peer"
    );
    let record_file = directory.join(format!("state/agent/{}.json", hex::encode([9u8; 32])));
    let record: Value = serde_json::from_slice(&fs::read(&record_file).unwrap()).unwrap();
    assert_eq!(record["version"], 1);
    assert!(record.get("peer").is_none());

    // Migrated: b1 becomes the legacy peer of a version 2 table; the operation resumes.
    world.write_peer("b1", "data.snapshot.v1", 65, true, true);
    world.write_seller(15);
    let seller = world.seller(9, None);
    let (buyer_ok, buyer) = finish(world.buyer("b1", 19));
    let (ok, reply) = finish(seller);
    assert!(ok && buyer_ok, "{reply} {buyer}");
    assert_eq!(reply["peer"], "b1");
    assert_eq!(reply["deal_commitment"], frozen["deal_commitment"]);
    assert_eq!(world.transcripts(), ["agent"]);
    // The retained version 1 record cannot be resumed for another peer.
    let (ok, other) = finish(world.seller(9, Some("b2")));
    assert!(!ok);
    assert_eq!(
        other["error"],
        "operation is bound to another negotiation peer"
    );
}

#[test]
fn a_flood_of_reconnecting_clients_cannot_extend_the_call_past_its_deadline() {
    let world = World::new();
    world.write_seller(1);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Several clients connect and hang up in a tight loop, so the accept queue is never empty.
    let flooders: Vec<_> = (0..2)
        .map(|_| {
            let stop = stop.clone();
            let endpoint = world.endpoint;
            thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    if let Ok(stream) = std::net::TcpStream::connect(endpoint) {
                        let _ = stream.set_nodelay(true);
                        let _ = (&stream).write_all(&[0, 0, 0, 1, 0]);
                        drop(stream);
                    }
                }
            })
        })
        .collect();
    thread::sleep(Duration::from_millis(100));
    let started = Instant::now();
    let (ok, reply) = finish(world.seller(10, None));
    let elapsed = started.elapsed();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    for flooder in flooders {
        flooder.join().unwrap();
    }
    assert!(!ok);
    assert_eq!(
        reply["error"],
        "no configured peer connected before the deadline"
    );
    // Without the deadline check the flood, which only stops after this call returns, would hold
    // the call until the harness's 60 s stall guard; the margin covers process startup on a
    // loaded machine.
    assert!(
        elapsed < Duration::from_secs(10),
        "the call outlived its 1 s deadline: {elapsed:?}"
    );
    // The identity lease is free again: the next call runs.
    let (ok, again) = finish(world.seller(11, None));
    assert!(!ok);
    assert_eq!(
        again["error"],
        "no configured peer connected before the deadline"
    );
}

#[test]
fn a_peer_that_expires_after_the_call_starts_is_refused_when_it_connects() {
    for legacy in [false, true] {
        let world = World::new();
        world.write_seller(14);
        world.write_peer("b2", "data.other.v1", 85, false, false);
        world.write_peer_until("b1", "data.snapshot.v1", 65, true, legacy, clock() + 5);
        if legacy {
            world.write_buyer("b1", None);
        }
        let seller = world.seller(12, None);
        // Live when the call starts; expired by the time the buyer authenticates.
        thread::sleep(Duration::from_secs(7));
        let (buyer_ok, buyer) = finish(world.buyer("b1", 22));
        let (ok, reply) = finish(seller);
        assert!(!ok && !buyer_ok, "legacy {legacy}: {reply} {buyer}");
        assert_eq!(
            reply["error"],
            "no configured peer connected before the deadline"
        );
        // Once expired, a new call refuses it before listening.
        let (ok, refused) = finish(world.seller(13, Some("b1")));
        assert!(!ok);
        assert_eq!(refused["error"], "negotiation peer is expired or disabled");
    }
}
