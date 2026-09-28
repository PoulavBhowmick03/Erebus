//! Compares Rust witness construction with the independently generated M5 JS flow.

use std::{fs, path::PathBuf};

use erebus_core::{
    auth::{Authorization, Role},
    commitment::CommitmentBlinding,
    domain::DeploymentDomain,
    ids::{AddressBytes, AssetId, BaseUnits, ChainNamespace, KeyBytes, SignatureBytes},
    service::ServiceRecord,
    shielded::{note_tree_parent, ShieldedDeal, NOTE_TREE_DEPTH},
    terms::{AgreementTerms, FeePolicy, Guarantee, GuaranteeSet, SettlementMode},
};
use erebus_shielded_prover::{
    build_deposit_witness, build_transfer_witness, build_transfer_witness_from_wallet,
    build_withdraw_witness,
    indexer::{PoolBlock, PoolEvent, PoolIndex},
    wallet::{NoteInclusion, OwnedNote, WalletDomain, WalletSnapshot, WalletStore},
    ChangeNote, InputNote, NotePath, ProverError, TransferRequest, WalletTransferRequest,
};
use num_bigint::BigUint;
use serde_json::Value;
use sha2::{Digest, Sha256};

fn hex_bytes<const N: usize>(value: &str) -> [u8; N] {
    hex::decode(value).expect("hex").try_into().expect("width")
}

fn decimal_bytes(value: &Value) -> [u8; 32] {
    let integer = value
        .as_str()
        .expect("decimal")
        .parse::<BigUint>()
        .expect("integer");
    let bytes = integer.to_bytes_be();
    let mut field = [0u8; 32];
    field[32 - bytes.len()..].copy_from_slice(&bytes);
    field
}

fn test_field(name: &str) -> [u8; 32] {
    let digest = Sha256::digest(format!("EREBUS_M5_TEST_ONLY_{name}").as_bytes());
    let modulus = BigUint::parse_bytes(
        b"21888242871839275222246405745257275088548364400416034343698204186575808495617",
        10,
    )
    .expect("BN254 modulus");
    let bytes = (BigUint::from_bytes_be(&digest) % modulus).to_bytes_be();
    let mut field = [0u8; 32];
    field[32 - bytes.len()..].copy_from_slice(&bytes);
    field
}

fn text<'a>(value: &'a Value, name: &str) -> &'a str {
    value[name].as_str().expect("fixture string")
}

#[test]
#[ignore = "requires M5 local artifact generation"]
fn rust_builds_the_exact_m5_transfer_witness() {
    let build = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../circuits/m5/build");
    let agreement: Value =
        serde_json::from_slice(&fs::read(build.join("agreement-input.json")).expect("agreement"))
            .expect("agreement JSON");
    let expected: Value =
        serde_json::from_slice(&fs::read(build.join("transfer-input.json")).expect("transfer"))
            .expect("transfer JSON");
    let data = &agreement["terms"];
    let domain = &data["domain"];
    let service = &data["service"];
    let mut guarantees = GuaranteeSet::empty();
    for guarantee in [
        Guarantee::HiddenAmount,
        Guarantee::HiddenRecipient,
        Guarantee::AgreementBoundSettlement,
    ] {
        guarantees.insert(guarantee);
    }
    let terms = AgreementTerms {
        protocol_version: data["protocolVersion"].as_u64().expect("version") as u16,
        suite_id: data["suiteId"].as_u64().expect("suite") as u16,
        domain: DeploymentDomain {
            namespace: ChainNamespace::parse(text(domain, "namespace")).expect("namespace"),
            settlement_contract: Some(
                AddressBytes::new(
                    hex::decode(text(domain, "settlementContractHex")).expect("address"),
                )
                .expect("contract"),
            ),
            pool: Some(
                AddressBytes::new(hex::decode(text(domain, "poolHex")).expect("address"))
                    .expect("pool"),
            ),
            verifier_version: domain["verifierVersion"].as_u64().expect("version") as u32,
        },
        deal_id: hex_bytes(text(data, "dealIdHex")),
        revision: data["revision"].as_u64().expect("revision") as u32,
        transcript_root: hex_bytes(text(data, "transcriptRootHex")),
        buyer_authorization_key: KeyBytes::new(
            hex::decode(text(data, "buyerAuthorizationKeyHex")).expect("buyer key"),
        )
        .expect("buyer key"),
        seller_authorization_key: KeyBytes::new(
            hex::decode(text(data, "sellerAuthorizationKeyHex")).expect("seller key"),
        )
        .expect("seller key"),
        payment_recipient: KeyBytes::new(
            hex::decode(text(data, "paymentRecipientHex")).expect("recipient"),
        )
        .expect("recipient"),
        asset: AssetId::parse(text(data, "asset")).expect("asset"),
        amount: BaseUnits::new(text(data, "amount").parse().expect("amount")),
        expiry: data["expiry"].as_u64().expect("expiry"),
        fee_policy: FeePolicy::none(),
        settlement_mode: SettlementMode::Shielded,
        required_guarantees: guarantees,
        settlement_nonce: hex_bytes(text(data, "settlementNonceHex")),
        service: ServiceRecord {
            resource: text(service, "resource").to_owned(),
            quantity: BaseUnits::new(text(service, "quantity").parse().expect("quantity")),
            unit: text(service, "unit").to_owned(),
            access_recipient: KeyBytes::new(
                hex::decode(text(service, "accessRecipientHex")).expect("access key"),
            )
            .expect("access key"),
            delivery_deadline: service["deliveryDeadline"]
                .as_u64()
                .expect("delivery deadline"),
            fulfillment_method: text(service, "fulfillmentMethod").to_owned(),
            fulfillment_digest: hex_bytes(text(service, "fulfillmentDigestHex")),
        },
    };
    let blinding = CommitmentBlinding::from_bytes(hex_bytes(text(&agreement, "blindingHex")));
    let commitment = ShieldedDeal::from_terms(&terms)
        .expect("deal")
        .commitment(&blinding)
        .expect("commitment");
    let authorization = |role, name| Authorization {
        role,
        suite_id: 2,
        commitment,
        signature: SignatureBytes::new(hex::decode(text(&agreement, name)).expect("signature"))
            .expect("signature"),
    };
    let buyer = authorization(Role::Buyer, "buyerSignatureHex");
    let seller = authorization(Role::Seller, "sellerSignatureHex");
    let note = InputNote {
        amount: text(&expected, "inputAmount")
            .parse()
            .expect("input amount"),
        spend_secret: decimal_bytes(&expected["inputSpendSecret"]),
        salt: decimal_bytes(&expected["inputSalt"]),
    };
    let siblings: [[u8; 32]; 20] = expected["pathElements"]
        .as_array()
        .expect("path")
        .iter()
        .map(decimal_bytes)
        .collect::<Vec<_>>()
        .try_into()
        .expect("depth");
    let mut index = 0u32;
    for (level, bit) in expected["pathIndices"]
        .as_array()
        .expect("indices")
        .iter()
        .enumerate()
    {
        let bit: u32 = bit.as_str().expect("bit").parse().expect("bit");
        index |= bit << level;
    }
    let path = NotePath {
        index,
        siblings,
        root: decimal_bytes(&expected["root"]),
    };
    let change = ChangeNote {
        spend_secret: test_field("change-spend"),
        salt: decimal_bytes(&expected["changeSalt"]),
    };
    assert_eq!(
        erebus_core::shielded::note_spend_tag(&change.spend_secret).expect("change tag"),
        decimal_bytes(&expected["changeSpendTag"])
    );
    let request = TransferRequest {
        terms: &terms,
        blinding: &blinding,
        buyer: &buyer,
        seller: &seller,
        note: &note,
        path: &path,
        change: &change,
        now: 1,
    };
    let witness = build_transfer_witness(&request).expect("Rust transfer witness");
    assert_eq!(witness.input, expected);
    let invalid_change = ChangeNote {
        spend_secret: [0; 32],
        salt: change.salt,
    };
    assert!(matches!(
        build_transfer_witness(&TransferRequest {
            change: &invalid_change,
            ..request
        }),
        Err(ProverError::Transfer("change spend secret"))
    ));

    let asset = hex_bytes::<20>(
        terms
            .asset
            .asset_reference()
            .strip_prefix("0x")
            .expect("EVM asset"),
    );
    let change_owned = change
        .owned_note(&terms, note.amount)
        .expect("change opening")
        .expect("nonzero change");
    assert_eq!(
        change_owned.commitment(),
        decimal_bytes(&expected["changeCommitment"])
    );
    assert!(change
        .owned_note(&terms, terms.amount.get())
        .unwrap()
        .is_none());
    let mut change_wallet = WalletSnapshot::default();
    change_wallet
        .add(change_owned.clone())
        .expect("backed-up change opening");
    assert!(change_wallet.select(&asset, 80).is_none());
    change_wallet
        .observe_insertion(
            &change_owned.commitment(),
            NoteInclusion {
                index: 2,
                block_number: 101,
                block_hash: [5; 32],
                root: [6; 32],
            },
        )
        .expect("change insertion");
    assert_eq!(
        change_wallet
            .select(&asset, 80)
            .expect("recognized change")
            .commitment(),
        change_owned.commitment()
    );
    let owned = OwnedNote::new(
        asset,
        note.amount,
        terms
            .buyer_authorization_key
            .as_bytes()
            .try_into()
            .expect("buyer key width"),
        note.spend_secret,
        note.salt,
    )
    .expect("owned input note");
    let mut wallet = WalletSnapshot::default();
    wallet.add(owned.clone()).expect("local opening");
    let mut index = PoolIndex::new(100).expect("pool index");
    let block_hash = [1; 32];
    index
        .apply_block(PoolBlock {
            number: 100,
            hash: block_hash,
            parent_hash: [2; 32],
            events: vec![PoolEvent::Inserted {
                index: 0,
                commitment: owned.commitment(),
                root: path.root,
                tx_hash: [3; 32],
                log_index: 0,
            }],
        })
        .expect("verified pool insertion");
    wallet
        .observe_insertion(
            &owned.commitment(),
            NoteInclusion {
                index: 0,
                block_number: 100,
                block_hash,
                root: path.root,
            },
        )
        .expect("wallet discovery");
    let wallet_request = WalletTransferRequest {
        terms: &terms,
        blinding: &blinding,
        buyer: &buyer,
        seller: &seller,
        wallet: &wallet,
        index: &index,
        change: &change,
        now: 1,
    };
    let restored_witness = build_transfer_witness_from_wallet(&wallet_request)
        .expect("wallet-derived witness");
    assert_eq!(restored_witness.input, expected);
    let manifest: Value = serde_json::from_slice(&fs::read(build.join("artifact-manifest.json")).unwrap()).unwrap();
    let artifact = &manifest["circuits"]["transfer"];
    let artifacts = erebus_shielded_prover::ProvingArtifacts {
        wasm: build.join("transfer_js/transfer.wasm"),
        r1cs: build.join("transfer.r1cs"),
        zkey: build.join("transfer.zkey"),
        wasm_sha256: text(artifact, "wasmSha256").to_owned(),
        r1cs_sha256: text(artifact, "r1csSha256").to_owned(),
        zkey_sha256: text(artifact, "zkeySha256").to_owned(),
    };
    let context = erebus_core::settlement::SettlementContext {
        require_local_proving: true,
        mode: terms.settlement_mode,
        domain: terms.domain.clone(),
        suite_id: terms.suite_id,
        asset: terms.asset.clone(),
        required_guarantees: terms.required_guarantees,
    };
    use erebus_shielded_prover::preparation::{prepare_transfer, validate_prepared, PreparationError};
    let mut wrong_context = context.clone();
    wrong_context.domain.verifier_version += 1;
    assert!(matches!(prepare_transfer(&wrong_context, &artifacts, &wallet_request, [4; 32]),
        Err(PreparationError::Context)));
    let prepared = prepare_transfer(&context, &artifacts, &wallet_request, [4; 32])
        .expect("wallet to locally proved prepared settlement");
    validate_prepared(&context, &prepared).unwrap();
    assert_eq!(prepared.backend_evidence.len(), 4 + 19 * 32);
    for secret in [note.spend_secret.as_slice(), change.spend_secret.as_slice(), blinding.as_bytes().as_slice(), buyer.signature.as_bytes(), seller.signature.as_bytes()] {
        assert!(!prepared.backend_evidence.windows(secret.len()).any(|part| part == secret));
    }
    let mut altered = prepared.clone();
    altered.deal_commitment = erebus_core::commitment::DealCommitment::from_bytes([1; 32]);
    assert!(validate_prepared(&context, &altered).is_err());
    let mut altered = prepared.clone();
    altered.backend_evidence[4 + 11 * 32 + 31] ^= 1;
    assert!(validate_prepared(&context, &altered).is_err());
    let mut altered = prepared.clone();
    altered.backend_evidence.push(0);
    assert!(validate_prepared(&context, &altered).is_err());
    fs::write(build.join("rust-prepared-transfer-calldata.json"),
        serde_json::to_vec(&format!("0x{}", hex::encode(&prepared.backend_evidence))).unwrap()).unwrap();
    wallet
        .reserve(&owned.commitment(), [4; 32])
        .expect("reservation");
    assert!(matches!(
        build_transfer_witness_from_wallet(&WalletTransferRequest {
            terms: &terms,
            blinding: &blinding,
            buyer: &buyer,
            seller: &seller,
            wallet: &wallet,
            index: &index,
            change: &change,
            now: 1,
        }),
        Err(ProverError::Transfer("no spendable note"))
    ));

    let note_vector: Value = serde_json::from_str(include_str!(
        "../../core/tests/fixtures/m5-note-vector.json"
    ))
    .expect("pinned note vector");
    let recipient_secret = hex_bytes::<32>(text(&note_vector["payment"], "spendSecretHex"));
    let payment = OwnedNote::expected_payment(&terms, recipient_secret)
        .expect("seller reconstructs payment note");
    assert_eq!(
        payment.commitment(),
        hex_bytes::<32>(text(&note_vector["payment"], "commitmentHex"))
    );
    let mut intermediate_root =
        note_tree_parent(&owned.commitment(), &payment.commitment()).expect("second leaf");
    let mut zero = [0u8; 32];
    for _ in 1..NOTE_TREE_DEPTH {
        zero = note_tree_parent(&zero, &zero).expect("zero tree");
        intermediate_root = note_tree_parent(&intermediate_root, &zero).expect("intermediate root");
    }
    let final_root = hex_bytes::<32>(text(&note_vector["payment"], "rootHex"));
    index
        .apply_block(PoolBlock {
            number: 101,
            hash: [4; 32],
            parent_hash: block_hash,
            events: vec![
                PoolEvent::Inserted {
                    index: 1,
                    commitment: payment.commitment(),
                    root: intermediate_root,
                    tx_hash: [5; 32],
                    log_index: 0,
                },
                PoolEvent::Inserted {
                    index: 2,
                    commitment: hex_bytes(text(&note_vector, "changeCommitmentHex")),
                    root: final_root,
                    tx_hash: [5; 32],
                    log_index: 1,
                },
                PoolEvent::Consumed {
                    nullifier: owned.nullifier(),
                    tx_hash: [5; 32],
                    log_index: 2,
                },
            ],
        })
        .expect("verified transfer outputs");
    let dir = tempfile::tempdir().expect("wallet directory");
    let wallet_path = dir.path().join("private/wallet.enc");
    let domain = WalletDomain {
        chain_id: terms
            .domain
            .namespace
            .reference()
            .parse()
            .expect("chain ID"),
        pool: terms
            .domain
            .pool
            .as_ref()
            .expect("pool")
            .as_bytes()
            .try_into()
            .expect("pool width"),
    };
    let deposit_input: Value = serde_json::from_slice(
        &fs::read(build.join("deposit-input.json")).expect("deposit fixture"),
    )
    .expect("deposit JSON");
    let deposit = build_deposit_witness(&owned, domain, terms.domain.verifier_version)
        .expect("Rust deposit witness");
    assert_eq!(deposit.input, deposit_input);
    let seller_store = WalletStore::new(&wallet_path, domain, [8; 32]).expect("seller wallet");
    seller_store
        .update(|snapshot| snapshot.add(payment))
        .expect("save expected note");
    drop(seller_store);
    let seller_store = WalletStore::new(&wallet_path, domain, [8; 32]).expect("restart wallet");
    seller_store
        .update(|snapshot| {
            index
                .replay_wallet(snapshot)
                .map_err(|_| erebus_shielded_prover::wallet::WalletError::Note("index replay"))
        })
        .expect("discover after restart");
    let restored = seller_store.snapshot().expect("restored wallet");
    let received = restored
        .select(&asset, terms.amount.get())
        .expect("spendable payment");
    assert_eq!(received.inclusion().expect("included").index, 1);
    let withdrawal_input: Value = serde_json::from_slice(
        &fs::read(build.join("withdraw-input.json")).expect("withdraw fixture"),
    )
    .expect("withdraw JSON");
    let recipient_field = decimal_bytes(&withdrawal_input["recipient"]);
    let recipient: [u8; 20] = recipient_field[12..].try_into().expect("recipient width");
    let withdrawal = build_withdraw_witness(
        &restored,
        &index,
        &received.commitment(),
        domain,
        terms.domain.verifier_version,
        recipient,
    )
    .expect("Rust withdrawal witness");
    assert_eq!(withdrawal.input, withdrawal_input);
    let public_names = [
        "chainId",
        "contractAddress",
        "verifierVersion",
        "asset",
        "dealCommitment",
        "dealNullifier",
        "root",
        "inputNullifier",
        "paymentCommitment",
        "changeCommitment",
        "expiry",
    ];
    for (name, value) in public_names.iter().zip(witness.public_inputs.iter()) {
        assert_eq!(witness.input[*name].as_str(), Some(value.as_str()));
    }

    let mut wrong_terms = terms.clone();
    wrong_terms.amount = BaseUnits::new(71);
    assert!(matches!(
        build_transfer_witness(&TransferRequest {
            terms: &wrong_terms,
            ..request
        }),
        Err(ProverError::Transfer("agreement authorization"))
    ));
    let mut wrong_path = path.clone();
    wrong_path.root[31] ^= 1;
    assert!(matches!(
        build_transfer_witness(&TransferRequest {
            path: &wrong_path,
            ..request
        }),
        Err(ProverError::Transfer("input root"))
    ));
}
