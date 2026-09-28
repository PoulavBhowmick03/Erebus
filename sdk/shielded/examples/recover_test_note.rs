//! Test-only recipient recovery against the M5 deterministic local fixture.
//! Never reuse the fixture secrets or wallet key with real funds.

use std::{env, error::Error};

use erebus_shielded_prover::{
    index_store::{IndexDomain, IndexStore},
    recovery::recover_wallet,
    rpc::PoolRpc,
    wallet::{OwnedNote, WalletDomain, WalletStore},
};
use serde_json::Value;

fn fixed_hex<const N: usize>(value: &str) -> Result<[u8; N], Box<dyn Error>> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    Ok(hex::decode(value)?
        .try_into()
        .map_err(|_| "wrong hex width")?)
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 9 {
        return Err("usage: recover_test_note <rpc> <chain-id> <pool> <deployment-block> <deployment-hash> <index-cache> <encrypted-wallet> <expected-spendable:0|1>".into());
    }
    let chain_id: u64 = args[2].parse()?;
    let pool = fixed_hex::<20>(&args[3])?;
    let domain = IndexDomain {
        chain_id,
        pool,
        first_block: args[4].parse()?,
        first_hash: fixed_hex::<32>(&args[5])?,
    };
    let index = IndexStore::new(&args[6], domain)?;
    let wallet = WalletStore::new(&args[7], WalletDomain { chain_id, pool }, [8; 32])?;
    let fixture: Value = serde_json::from_str(include_str!(
        "../../core/tests/fixtures/m5-note-vector.json"
    ))?;
    let payment = &fixture["payment"];
    let string = |name: &str| payment[name].as_str().ok_or("missing payment field");
    let asset = fixed_hex::<20>(fixture["assetHex"].as_str().ok_or("missing asset")?)?;
    let note = OwnedNote::new(
        asset,
        string("amount")?.parse()?,
        fixed_hex::<64>(string("ownerHex")?)?,
        fixed_hex::<32>(string("spendSecretHex")?)?,
        fixed_hex::<32>(string("saltHex")?)?,
    )?;
    wallet.update(|snapshot| {
        if snapshot.notes().is_empty() {
            snapshot.add(note)?;
        }
        Ok(())
    })?;
    let rpc = PoolRpc::new(&args[1], chain_id, pool)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let report = runtime.block_on(recover_wallet(&rpc, &index, &wallet, 0))?;
    let spendable = wallet.snapshot()?.select(&asset, 70).is_some();
    let expected = match args[8].as_str() {
        "0" => false,
        "1" => true,
        _ => return Err("expected-spendable must be 0 or 1".into()),
    };
    if spendable != expected {
        return Err("recovered note spendability differs from expected chain state".into());
    }
    println!(
        "recovered wallet through block {} with {} leaves; payment spendable: {}",
        report.through, report.leaves, spendable
    );
    Ok(())
}
