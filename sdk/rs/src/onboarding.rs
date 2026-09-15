//! Account provisioning for installed clients. Secret account data stays in Rust.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

use fs2::FileExt;
use serde_json::{json, Value};
use starknet::{
    accounts::{AccountFactory, OpenZeppelinAccountFactory},
    core::{
        types::{BlockId, BlockTag, BroadcastedDeployAccountTransactionV3, Felt, StarknetError},
        utils::get_contract_address,
    },
    providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider, ProviderError},
    signers::{LocalWallet, SigningKey},
};

/// OpenZeppelin account v1.0.0, also pinned by Starknet Foundry.
pub const ACCOUNT_CLASS: &str =
    "0x05b4b537eaa2399e3aa99c4e2e0208ebd6c71bc1467938cd52c798c601e43564";

/// Provisioning failure, with no private input included in its message.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct OnboardingError(pub String);

fn fail(message: &str) -> OnboardingError {
    OnboardingError(message.into())
}
fn field(v: &Value, key: &str) -> Result<String, OnboardingError> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| fail(&format!("missing {key}")))
}
fn felt(s: &str) -> Result<Felt, OnboardingError> {
    Felt::from_hex(s).map_err(|_| fail("invalid hexadecimal account field"))
}
fn read(path: &Path) -> Result<Value, OnboardingError> {
    serde_json::from_slice(&fs::read(path).map_err(|_| fail("cannot read account metadata"))?)
        .map_err(|_| fail("invalid account metadata"))
}
fn save_new(path: &Path, bytes: &[u8]) -> Result<(), OnboardingError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options
        .open(path)
        .map_err(|_| fail("cannot create protected file; existing files are never replaced"))?;
    f.write_all(bytes)
        .and_then(|()| f.sync_all())
        .map_err(|_| fail("cannot persist protected file"))?;
    fs::File::open(path.parent().ok_or_else(|| fail("missing parent"))?)
        .and_then(|f| f.sync_all())
        .map_err(|_| fail("cannot sync account directory"))
}

/// Runs a provisioning command. Discovery returns public metadata only.
pub async fn run(action: &str, args: Value) -> Result<Value, OnboardingError> {
    if action == "verify_signer" {
        let rpc = crate::rpc::StarknetRpc::new(field(&args, "rpc_url")?)
            .map_err(|_| fail("invalid RPC URL"))?;
        if rpc
            .chain_id()
            .await
            .map_err(|_| fail("cannot verify RPC chain"))?
            != felt(&field(&args, "chain_id")?)?
        {
            return Err(fail("RPC chain mismatch"));
        }
        let key = fs::read_to_string(field(&args, "account_key_file")?)
            .map_err(|_| fail("account key unavailable"))?;
        let public = SigningKey::from_secret_scalar(felt(key.trim())?)
            .verifying_key()
            .scalar();
        let actual = rpc.call_contract(felt(&field(&args, "address")?)?, "get_public_key", &[], &crate::prover::BlockId::Latest)
            .await.map_err(|_| fail("account signer cannot be verified; automatic onboarding supports OpenZeppelin single-key accounts"))?;
        if actual.as_slice() != [public] {
            return Err(fail(
                "account key does not match the deployed account signer",
            ));
        }
        return Ok(json!({"verified": true}));
    }
    if action == "inspect" {
        let rpc = crate::rpc::StarknetRpc::new(field(&args, "rpc_url")?)
            .map_err(|_| fail("invalid RPC URL"))?;
        let chain = felt(&field(&args, "chain_id")?)?;
        if rpc
            .chain_id()
            .await
            .map_err(|_| fail("cannot verify RPC chain"))?
            != chain
        {
            return Err(fail("RPC chain mismatch"));
        }
        let address = felt(&field(&args, "address")?)?;
        let pool = felt(&field(&args, "pool_address")?)?;
        let token = felt(&field(&args, "token")?)?;
        let block = crate::prover::BlockId::Latest;
        let balance = rpc
            .call_contract(token, "balanceOf", &[address], &block)
            .await
            .map_err(|_| fail("cannot read public balance"))?;
        let allowance = rpc
            .call_contract(token, "allowance", &[address, pool], &block)
            .await
            .map_err(|_| fail("cannot read allowance"))?;
        let fee = rpc
            .call_contract(pool, "get_fee_amount", &[], &block)
            .await
            .map_err(|_| fail("cannot read pool fee"))?;
        let registered = rpc
            .call_contract(pool, "get_public_key", &[address], &block)
            .await
            .map_err(|_| fail("cannot check pool registration"))?;
        let fee = fee.first().ok_or_else(|| fail("missing pool fee"))?;
        let registered = registered
            .first()
            .ok_or_else(|| fail("missing pool public key"))?;
        return Ok(
            json!({"public_balance": crate::erc20::parse_u256("balanceOf", &balance).map_err(|_| fail("invalid balance"))?.to_string(),
            "allowance": crate::erc20::parse_u256("allowance", &allowance).map_err(|_| fail("invalid allowance"))?.to_string(),
            "fee_per_write": u128::try_from(*fee).map_err(|_| fail("invalid pool fee"))?.to_string(),
            "registered_public_key": format!("{registered:#x}"), "proving_block_lag": crate::execution::DEFAULT_PROVING_BLOCK_LAG,
            "gas_reserve_per_write": crate::client::DEFAULT_GAS_RESERVE.to_string()}),
        );
    }
    if action == "discover" {
        let source = field(&args, "accounts_file")?;
        if !Path::new(&source).exists() {
            return Ok(json!([]));
        }
        let data = read(Path::new(&source))?;
        let mut rows = Vec::new();
        for (network, accounts) in data
            .as_object()
            .ok_or_else(|| fail("invalid accounts file"))?
        {
            if let Some(accounts) = accounts.as_object() {
                for (name, account) in accounts {
                    if let Some(address) = account.get("address").and_then(Value::as_str) {
                        rows.push(json!({"name": name, "network": network, "address": address,
                            "has_signer": account.get("private_key").and_then(Value::as_str).is_some(),
                            "deployed": account.get("deployed"), "type": account.get("type")}));
                    }
                }
            }
        }
        return Ok(json!(rows));
    }
    let directory = field(&args, "directory")?;
    let root = Path::new(&directory);
    if !root.is_absolute() || !root.is_dir() {
        return Err(fail("identity directory must exist and be absolute"));
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("account.lock"))
        .map_err(|_| fail("cannot lock identity"))?;
    lock.lock_exclusive()
        .map_err(|_| fail("cannot lock identity"))?;
    let metadata_path = root.join("account.json");
    let key_path = root.join("account.key");
    if action == "create" || action == "import" {
        if metadata_path.exists() {
            let metadata = read(&metadata_path)?;
            if field(&metadata, "chain_id")? != field(&args, "chain_id")? {
                return Err(fail("existing account belongs to another chain"));
            }
            return Ok(metadata);
        }
        let (address, public_key, class_hash, salt) = if action == "create" {
            // If creation was interrupted after key persistence, keep that key.
            if !key_path.exists() {
                crate::keys::generate_pool_key_file(&key_path)
                    .map_err(|_| fail("cannot create account signing key"))?;
            }
            let secret =
                fs::read_to_string(&key_path).map_err(|_| fail("account key unavailable"))?;
            let public = SigningKey::from_secret_scalar(felt(secret.trim())?)
                .verifying_key()
                .scalar();
            let class = felt(ACCOUNT_CLASS)?;
            // A fresh signing key makes each address unique; zero salt is deterministic on resume.
            (
                get_contract_address(Felt::ZERO, class, &[public], Felt::ZERO),
                public,
                class,
                Felt::ZERO,
            )
        } else {
            let data = read(Path::new(&field(&args, "accounts_file")?))?;
            let network = field(&args, "source_network")?;
            let name = field(&args, "name")?;
            let account = data
                .get(&network)
                .and_then(|v| v.get(&name))
                .ok_or_else(|| fail("selected account no longer exists"))?;
            let secret = field(account, "private_key")?;
            let public = SigningKey::from_secret_scalar(felt(&secret)?)
                .verifying_key()
                .scalar();
            if let Some(expected) = account.get("public_key").and_then(Value::as_str) {
                if felt(expected)? != public {
                    return Err(fail("account public key does not match its signer"));
                }
            }
            if key_path.exists() {
                let previous = fs::read_to_string(&key_path)
                    .map_err(|_| fail("cannot read interrupted import"))?;
                if felt(previous.trim())? != felt(&secret)? {
                    return Err(fail("another account key already exists in this directory"));
                }
            } else {
                save_new(&key_path, format!("{secret}\n").as_bytes())?;
            }
            (
                felt(&field(account, "address")?)?,
                public,
                felt(
                    account
                        .get("class_hash")
                        .and_then(Value::as_str)
                        .unwrap_or(ACCOUNT_CLASS),
                )?,
                felt(account.get("salt").and_then(Value::as_str).unwrap_or("0x0"))?,
            )
        };
        let metadata = json!({"address": format!("{address:#x}"), "public_key": format!("{public_key:#x}"),
            "class_hash": format!("{class_hash:#x}"), "salt": format!("{salt:#x}"),
            "account_key_file": key_path, "chain_id": field(&args, "chain_id")?});
        save_new(
            &metadata_path,
            serde_json::to_vec(&metadata)
                .map_err(|_| fail("cannot encode metadata"))?
                .as_slice(),
        )?;
        return Ok(metadata);
    }
    let metadata = read(&metadata_path)?;
    let chain = felt(&field(&metadata, "chain_id")?)?;
    let address = felt(&field(&metadata, "address")?)?;
    let url: reqwest::Url = field(&args, "rpc_url")?
        .parse()
        .map_err(|_| fail("invalid RPC URL"))?;
    let provider = JsonRpcClient::new(HttpTransport::new(url));
    if provider
        .chain_id()
        .await
        .map_err(|_| fail("cannot verify RPC chain"))?
        != chain
    {
        return Err(fail("RPC chain does not match selected account"));
    }
    let deployed = match provider
        .get_class_hash_at(BlockId::Tag(BlockTag::Latest), address)
        .await
    {
        Ok(_) => true,
        Err(ProviderError::StarknetError(StarknetError::ContractNotFound)) => false,
        Err(_) => return Err(fail("cannot check account deployment")),
    };
    if action == "status" || deployed {
        return Ok(json!({"address": format!("{address:#x}"), "deployed": deployed}));
    }
    if action != "deploy" {
        return Err(fail("unknown account provisioning action"));
    }
    let record = root.join("deployment.json");
    if !record.exists() {
        let secret = fs::read_to_string(&key_path).map_err(|_| fail("account key unavailable"))?;
        let signer = LocalWallet::from(SigningKey::from_secret_scalar(felt(secret.trim())?));
        let factory = OpenZeppelinAccountFactory::new(
            felt(&field(&metadata, "class_hash")?)?,
            chain,
            signer,
            &provider,
        )
        .await
        .map_err(|_| fail("cannot load deployment signer"))?;
        let deployment = factory
            .deploy_v3(felt(&field(&metadata, "salt")?)?)
            .nonce(Felt::ZERO);
        if deployment.address() != address {
            return Err(fail("account is not a supported OpenZeppelin deployment"));
        }
        let fee = deployment.estimate_fee().await.map_err(|_| {
            fail("deployment estimate failed; verify account funding and class availability")
        })?;
        let gas = |n: u64| {
            n.checked_mul(2)
                .ok_or_else(|| fail("gas estimate overflow"))
        };
        let price = |n: u128| n.checked_mul(2).ok_or_else(|| fail("gas price overflow"));
        let prepared = deployment
            .l1_gas(gas(fee.l1_gas_consumed)?)
            .l1_gas_price(price(fee.l1_gas_price)?)
            .l2_gas(gas(fee.l2_gas_consumed)?)
            .l2_gas_price(price(fee.l2_gas_price)?)
            .l1_data_gas(gas(fee.l1_data_gas_consumed)?)
            .l1_data_gas_price(price(fee.l1_data_gas_price)?)
            .tip(0)
            .prepared()
            .map_err(|_| fail("incomplete deployment estimate"))?;
        let wire = prepared
            .get_deploy_request(false, false)
            .await
            .map_err(|_| fail("cannot sign deployment"))?;
        save_new(&record, &serde_json::to_vec(&json!({"transaction_hash": format!("{:#x}", prepared.transaction_hash(false)), "wire": wire}))
            .map_err(|_| fail("cannot encode deployment"))?)?;
    }
    let saved = read(&record)?;
    let wire: BroadcastedDeployAccountTransactionV3 = serde_json::from_value(saved["wire"].clone())
        .map_err(|_| fail("invalid saved deployment"))?;
    let result = provider.add_deploy_account_transaction(wire).await;
    match result {
        Ok(receipt) => Ok(json!({"address": format!("{address:#x}"), "deployed": false, "transaction_hash": format!("{:#x}", receipt.transaction_hash)})),
        Err(_) => Err(fail("deployment submission uncertain; rerun onboarding to check the account and reuse the saved transaction")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("erebus-onboarding-{}", rand::random::<u64>()));
        fs::create_dir(&path).unwrap();
        path
    }

    #[tokio::test]
    async fn new_account_is_stable_across_retries_and_never_exports_secret() {
        let root = directory();
        let args = json!({"directory": root, "chain_id": "0x534e5f5345504f4c4941"});
        let first = run("create", args.clone()).await.unwrap();
        let secret = fs::read_to_string(root.join("account.key")).unwrap();
        assert!(!first.to_string().contains(secret.trim()));
        let again = run("create", args).await.unwrap();
        assert_eq!(first, again);
        assert_eq!(
            fs::read_to_string(root.join("account.key")).unwrap(),
            secret
        );
        assert!(run(
            "create",
            json!({"directory": root, "chain_id": "0x534e5f4d41494e"})
        )
        .await
        .is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(root.join("account.key"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(root.join("account.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn interrupted_creation_reuses_persisted_key() {
        let root = directory();
        crate::keys::generate_pool_key_file(root.join("account.key")).unwrap();
        let before = fs::read(root.join("account.key")).unwrap();
        run("create", json!({"directory": root, "chain_id": "0x1"}))
            .await
            .unwrap();
        assert_eq!(before, fs::read(root.join("account.key")).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn discovery_sanitizes_wallet_data_and_import_preserves_key() {
        let root = directory();
        let accounts = root.join("accounts.json");
        let secret = "0x123456789abcdef";
        fs::write(&accounts, json!({"alpha-sepolia": {"alice": {"address": "0x123", "private_key": secret,
            "public_key": format!("{:#x}", SigningKey::from_secret_scalar(felt(secret).unwrap()).verifying_key().scalar())}}}).to_string()).unwrap();
        let rows = run("discover", json!({"accounts_file": accounts}))
            .await
            .unwrap();
        assert!(!rows.to_string().contains(secret));
        assert_eq!(rows[0]["has_signer"], true);
        let args = json!({"directory": root, "chain_id": "0x1", "accounts_file": accounts, "source_network": "alpha-sepolia", "name": "alice"});
        let imported = run("import", args.clone()).await.unwrap();
        assert!(!imported.to_string().contains(secret));
        assert_eq!(
            fs::read_to_string(root.join("account.key")).unwrap().trim(),
            secret
        );
        assert_eq!(run("import", args).await.unwrap(), imported);
        fs::remove_dir_all(root).unwrap();
    }
}
