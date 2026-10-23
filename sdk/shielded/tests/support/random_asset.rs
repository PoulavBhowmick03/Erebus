//! Random operator identities and product deposits, not fixture-owned spending material.
use super::*;
use erebus_transport::disclosure::{verify_selected_agreement, SelectedAgreement};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and M5 test artifacts; local ERC-20 asset workflow"]
async fn fresh_operators_deposit_negotiate_reuse_change_and_withdraw_erc20() {
    workflow(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and M5 test artifacts; not live WMON qualification"]
async fn fresh_operators_wrap_deposit_negotiate_reuse_change_and_unwrap() {
    workflow(true).await;
}

async fn workflow(native: bool) {
    let port = free_port();
    let _anvil = ServiceProcess(
        Command::new("anvil")
            .args([
                "--port",
                &port.to_string(),
                "--chain-id",
                "31337",
                "--slots-in-an-epoch",
                "1",
                "--block-time",
                "1",
                "--silent",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let url = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    wait_for_chain(&client, &url).await;
    let fixture_url = url.clone();
    let deployment: Value = tokio::task::spawn_blocking(move || {
        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../circuits/m5/scripts/native-fixture.mjs");
        let mut command = Command::new("node");
        command.arg(script).arg(fixture_url);
        if native {
            command.arg("wmon");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    })
    .await
    .unwrap();
    let pool = deployment["pool"].as_str().unwrap();
    let token = deployment["token"].as_str().unwrap();
    let asset = format!("eip155:31337/erc20:{token}");
    let root = tempfile::tempdir().unwrap();
    let negotiate = native_binary("erebus-negotiate", env!("CARGO_BIN_EXE_erebus-negotiate"));
    let wallet = native_binary("erebus-wallet", env!("CARGO_BIN_EXE_erebus-wallet"));
    let payment = native_binary("erebus-payment", env!("CARGO_BIN_EXE_erebus-payment"));
    let endpoint = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let manifest = serde_json::to_vec(&fixture_manifest(&deployment).await).unwrap();
    let manifest_path = root.path().join("manifest.json");
    private(&manifest_path, &manifest);
    let manifest_hash = hex::encode(Sha256::digest(&manifest));
    let (proxy, sends) = lossy_proxy_with_send_count(url.clone()).await;
    let mut wallet_configs = HashMap::new();
    let mut signers = HashMap::new();
    for role in ["buyer", "seller"] {
        let directory = root.path().join(role);
        let setup = finish(spawn(
            &negotiate,
            root.path(),
            json!({"method":"prepare_operator",
            "suite_id":2,"directory":directory,"role":role,"endpoint":endpoint,
            "namespace":"eip155:31337","assets":[asset],"descriptor_lifetime_seconds":86400}),
        ));
        assert!(setup.0, "{setup:?}");
        let binding = finish(spawn(
            &negotiate,
            root.path(),
            json!({"method":"prepare_binding",
            "output":directory.join("identity.binding"),"descriptor_file":setup.1["descriptor_file"],
            "discovery_key_file":setup.1["discovery_key_file"],"agreement_key_file":setup.1["agreement_key_file"],
            "settlement_contract":pool,"verifier_version":2,"expires_at":unix_now()+7200}),
        ));
        assert!(binding.0, "{binding:?}");
        let seed = random_bytes::<32>();
        let signer = format!(
            "0x{}",
            hex::encode(TransactionKey::from_bytes(&seed).unwrap().address())
        );
        private(&directory.join("transaction.key"), &seed);
        rpc(
            &client,
            &url,
            "anvil_setBalance",
            json!([signer, "0x56bc75e2d63100000"]),
        )
        .await;
        let state = directory.join("state");
        let config = json!({"version":1,"state_root":state,"namespace":"eip155:31337",
            "pool":pool,"verifier_version":2,"asset":token,"wrapper":if native {Some(token)} else {None},
            "rpc_url":proxy,"peer_rpc_url":url,"first_block":deployment["first_block"],"first_hash":deployment["first_hash"],
            "wallet_file":state.join("wallet/notes.enc"),"wallet_key_file":directory.join("wallet.key"),
            "agreement_key_file":setup.1["wallet_agreement_key_file"],"signer_address":signer,
            "signer_journal_root":state.join("signer-journal"),"transaction_key_file":directory.join("transaction.key"),
            "artifact_manifest_file":manifest_path,"artifact_manifest_sha256":manifest_hash,
            "artifact_cache_root":state.join("artifacts"),"allow_test_artifacts":true,"allow_loopback_artifact_http":true,
            "gas_limit":3000000,"max_fee_per_gas":"2000000000","max_priority_fee_per_gas":"1000000000",
            "timeout_seconds":30,"max_scan_blocks":1000,"artifact_timeout_seconds":120});
        private(
            &directory.join("wallet.json"),
            &serde_json::to_vec(&config).unwrap(),
        );
        wallet_configs.insert(role, directory.join("wallet.json"));
        signers.insert(role, signer);
    }
    let wallet_call = |role: &str, method: &str, extra: Value| {
        let mut request = json!({"method":method,"config_file":wallet_configs[role]});
        for (key, value) in extra.as_object().unwrap() {
            request[key] = value.clone();
        }
        finish(spawn(&wallet, root.path(), request))
    };
    if native {
        let result = wallet_call(
            "buyer",
            "wrap",
            json!({"amount":"150","operation_ref":"51".repeat(32)}),
        );
        assert!(result.0, "{result:?}");
        let result = sync_until(&wallet, root.path(), &wallet_configs["buyer"], |v| {
            operation_completed(v, "wrap")
        })
        .await;
        assert!(result.0, "{result:?}");
    } else {
        // WS6: a deposit whose asset balance is not visible yet refuses before proving or any
        // claim, even with the allowance already in place.
        let result = wallet_call("buyer", "approve", json!({"amount":"150"}));
        assert!(result.0, "{result:?}");
        let result = sync_until(&wallet, root.path(), &wallet_configs["buyer"], |v| {
            operation_completed(v, "approve")
        })
        .await;
        assert!(result.0, "{result:?}");
        let before = sends.load(Ordering::SeqCst);
        let refused = wallet_call(
            "buyer",
            "deposit",
            json!({"amount":"150","operation_ref":"52".repeat(32)}),
        );
        assert!(
            !refused.0
                && refused.1["error"]
                    == "deposit asset balance insufficient; wrap or fund the asset first",
            "{refused:?}"
        );
        assert_eq!(sends.load(Ordering::SeqCst), before);
        let owner: [u8; 20] = hex::decode(signers["buyer"].trim_start_matches("0x"))
            .unwrap()
            .try_into()
            .unwrap();
        let result = rpc(
            &client,
            &url,
            "eth_sendTransaction",
            json!([{"from":ANVIL_ACCOUNT,"to":token,
            "data":encode_call(&MINT_SELECTOR,&[word(&owner),word(&150u128.to_be_bytes())])}]),
        )
        .await;
        assert!(result.get("error").is_none(), "{result}");
        assert_eq!(
            wait_for_balance(&client, &url, token, &signers["buyer"], 150).await,
            150
        );
    }
    if native {
        let result = wallet_call("buyer", "approve", json!({"amount":"150"}));
        assert!(result.0, "{result:?}");
        let result = sync_until(&wallet, root.path(), &wallet_configs["buyer"], |v| {
            operation_completed(v, "approve")
        })
        .await;
        assert!(result.0, "{result:?}");
    }
    let deposit_ref = if native { "53" } else { "52" }.repeat(32);
    let result = wallet_call(
        "buyer",
        "deposit",
        json!({"amount":"150","operation_ref":deposit_ref}),
    );
    assert!(result.0, "{result:?}");
    let result = sync_until(&wallet, root.path(), &wallet_configs["buyer"], |v| {
        operation_completed(v, "deposit")
    })
    .await;
    assert!(result.0, "{result:?}");
    let baseline = sends.load(Ordering::SeqCst);
    let expiry = unix_now() + 7200;
    for (role, peer, price) in [("buyer", "seller", "60"), ("seller", "buyer", "70")] {
        let directory = root.path().join(role);
        let other = root.path().join(peer);
        let result = finish(spawn(
            &negotiate,
            root.path(),
            json!({"method":"prepare_terms","suite_id":2,
            "output":directory.join("service.terms"),"role":role,
            "local_descriptor_file":directory.join(format!("{role}.descriptor.json")),
            "peer_descriptor_file":other.join(format!("{peer}.descriptor.json")),
            "local_binding_file":directory.join("identity.binding"),"peer_binding_file":other.join("identity.binding"),
            "settlement_contract":pool,"verifier_version":2,"asset":asset,"amount":price,
            "resource":"dataset.snapshot.v1","unit":"snapshot","quantity":"1","fulfillment_method":"http-access-v1",
            "fulfillment_digest":hex::encode(Sha256::digest(b"random asset workflow")),"delivery_deadline":expiry}),
        ));
        assert!(result.0, "{result:?}");
        private(&directory.join("negotiate.json"),&serde_json::to_vec(&json!({"version":1,"role":role,
            "state_root":directory.join("state"),"transport_key_file":directory.join("transport.key"),
            "agreement_key_file":directory.join("agreement.key"),"discovery_key_file":directory.join("discovery.key"),
            "local_descriptor_file":directory.join(format!("{role}.descriptor.json")),
            "peer_descriptor_file":other.join(format!("{peer}.descriptor.json")),"terms_template_file":directory.join("service.terms"),
            "endpoint":endpoint,"maximum_price":"75","minimum_price":"65","max_deal_lifetime_seconds":3600,"timeout_seconds":30,
            "seller_spend_secret_file":if role=="seller" {Some(directory.join("spend.key"))} else {None},
            "seller_wallet_file":if role=="seller" {Some(directory.join("state/wallet/notes.enc"))} else {None},
            "seller_wallet_key_file":if role=="seller" {Some(directory.join("wallet.key"))} else {None}})).unwrap());
    }
    let buyer_dir = root.path().join("buyer");
    let mut config = deployment.clone();
    for key in ["token", "wmon", "asset_mode", "artifact_build"] {
        config.as_object_mut().unwrap().remove(key);
    }
    let additions = json!({"version":1,"mode":"shielded","state_root":buyer_dir.join("state"),
        "namespace":"eip155:31337","verifier_version":2,"rpc_url":proxy,"peer_rpc_url":url,
        "buyer_key_hex":hex::encode(fs::read(buyer_dir.join("agreement.public")).unwrap()),"asset":asset,
        "signer_address":signers["buyer"],"signer_journal_root":buyer_dir.join("state/signer-journal"),
        "transaction_key_file":buyer_dir.join("transaction.key"),"wallet_file":buyer_dir.join("state/wallet/notes.enc"),
        "wallet_key_file":buyer_dir.join("wallet.key"),"manifest_file":manifest_path,"manifest_sha256":manifest_hash,
        "artifact_cache":buyer_dir.join("state/artifacts"),"allow_test_artifacts":true,"allow_loopback_http":true,
        "maximum_price":"75","gas_limit":3000000,"max_fee_per_gas":"2000000000","max_priority_fee_per_gas":"1000000000",
        "timeout_seconds":30,"max_scan_blocks":1000});
    for (key, value) in additions.as_object().unwrap() {
        config[key] = value.clone();
    }
    let config_path = buyer_dir.join("payment.json");
    private(&config_path, &serde_json::to_vec(&config).unwrap());
    for operation in ["61".repeat(32), "62".repeat(32)] {
        let request = |role: &str| json!({"method":"negotiate","config_file":root.path().join(role).join("negotiate.json"),"operation_ref":operation});
        let seller = spawn(&negotiate, root.path(), request("seller"));
        let buyer = spawn(&negotiate, root.path(), request("buyer"));
        let buyer = finish(buyer);
        let seller = finish(seller);
        assert!(buyer.0 && seller.0, "{buyer:?}; {seller:?}");
        let selected = SelectedAgreement::decode(
            &fs::read(buyer.1["evidence_file"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        verify_selected_agreement(&selected).unwrap();
        assert_eq!(selected.terms.amount.get(), 70);
        let settle = finish(spawn(
            &payment,
            root.path(),
            json!({"method":"settle","config_file":config_path,"operation_ref":operation}),
        ));
        assert!(
            settle.0 || (settle.2 == 2 && settle.1["status"] == "pending"),
            "{settle:?}"
        );
        assert_eq!(settle.1["submitted_this_call"], true, "{settle:?}");
        let estimate = settle.1["funding"]["estimated_gas"].as_u64().unwrap();
        eprintln!("WS6 settle estimate native={native}: {estimate}");
        assert!(estimate * 105 <= erebus_shielded_prover::gas_policy::SETTLE_GAS_LIMIT * 100);
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let recovered = finish(spawn(
                &payment,
                root.path(),
                json!({"method":"observe","config_file":config_path,"operation_ref":operation}),
            ));
            if recovered.1["payment_verified"] == true {
                break;
            }
            assert!(
                recovered.2 == 2
                    || recovered.1["error"]
                        == "paired pool evidence unavailable; retain reservations",
                "unexpected observation failure: {recovered:?}"
            );
            assert!(Instant::now() < deadline, "{recovered:?}");
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let before = sends.load(Ordering::SeqCst);
        let repeat = finish(spawn(
            &payment,
            root.path(),
            json!({"method":"settle","config_file":config_path,"operation_ref":operation}),
        ));
        assert!(
            repeat.0 && repeat.1["payment_verified"] == true,
            "{repeat:?}"
        );
        assert_eq!(sends.load(Ordering::SeqCst), before);
    }
    assert_eq!(sends.load(Ordering::SeqCst), baseline + 2);
    for (role, amount) in [("buyer", 10u128), ("seller", 140u128)] {
        let result = sync_until(&wallet, root.path(), &wallet_configs[role], |v| {
            v["result"]["notes"].is_array()
        })
        .await;
        assert!(result.0, "{result:?}");
        let notes: Vec<_> = result.1["result"]["notes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["included"] == true && n["consumed"] == false && n["reserved"].is_null())
            .cloned()
            .collect();
        assert_eq!(
            notes
                .iter()
                .map(|n| n["amount"].as_str().unwrap().parse::<u128>().unwrap())
                .sum::<u128>(),
            amount,
            "unexpected available wallet notes for {role}: {:?}",
            result.1
        );
        for note in notes {
            let result = wallet_call(
                role,
                "withdraw",
                json!({"commitment":note["commitment"],"recipient":signers[role]}),
            );
            assert!(result.0, "{result:?}");
            let result = sync_until(&wallet, root.path(), &wallet_configs[role], |v| {
                v["result"]["notes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|n| n["commitment"] == note["commitment"] && n["consumed"] == true)
            })
            .await;
            assert!(result.0, "{result:?}");
        }
        assert_eq!(
            token_balance(&client, &url, token, &signers[role]).await,
            amount
        );
        if native {
            let result = wallet_call(
                role,
                "unwrap",
                json!({"amount":amount.to_string(),"operation_ref":"71".repeat(32)}),
            );
            assert!(result.0, "{result:?}");
            let result = sync_until(&wallet, root.path(), &wallet_configs[role], |v| {
                operation_completed(v, "unwrap")
            })
            .await;
            assert!(result.0, "{result:?}");
            assert_eq!(token_balance(&client, &url, token, &signers[role]).await, 0);
        }
    }
    assert_eq!(
        token_balance(&client, &url, token, pool).await,
        150,
        "only the fixture's unrelated note remains"
    );
    eprintln!("random asset workflow native={native}: two payments, change reuse, exact withdrawal balances");
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
