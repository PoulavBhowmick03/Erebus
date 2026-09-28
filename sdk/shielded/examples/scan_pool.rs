//! Checks real local EVM pool logs against the Rust Poseidon index.

use std::{env, error::Error};

use erebus_shielded_prover::{
    index_store::{IndexDomain, IndexStore},
    indexer::PoolIndex,
    rpc::PoolRpc,
};

fn fixed_hex<const N: usize>(value: &str) -> Result<[u8; N], Box<dyn Error>> {
    let value = value.strip_prefix("0x").ok_or("missing 0x prefix")?;
    Ok(hex::decode(value)?
        .try_into()
        .map_err(|_| "wrong hex width")?)
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 8 && args.len() != 10 {
        return Err("usage: scan_pool <rpc> <chain-id> <pool> <first-block> <through-block> <expected-root> <expected-leaves> [cache-path first-block-hash]".into());
    }
    let chain_id = args[2].parse()?;
    let pool = fixed_hex::<20>(&args[3])?;
    let first = args[4].parse()?;
    let through = args[5].parse()?;
    let expected_root = fixed_hex::<32>(&args[6])?;
    let expected_leaves: usize = args[7].parse()?;
    let rpc = PoolRpc::new(&args[1], chain_id, pool)?;
    let store = if args.len() == 10 {
        Some(IndexStore::new(
            &args[8],
            IndexDomain {
                chain_id,
                pool,
                first_block: first,
                first_hash: fixed_hex::<32>(&args[9])?,
            },
        )?)
    } else {
        None
    };
    let mut index = match &store {
        Some(store) => store.load()?,
        None => PoolIndex::new(first)?,
    };
    let previous_tip = index.tip().map(|tip| tip.hash);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        loop {
            if index.tip().is_some_and(|tip| tip.number >= through) {
                break;
            }
            let count = rpc.sync(&mut index, through, 1_000).await?;
            if count == 0 {
                return Err("RPC scan did not advance".into());
            }
        }
        Ok::<(), Box<dyn Error>>(())
    })?;
    if index.root() != expected_root || index.leaves().len() != expected_leaves {
        return Err("indexed pool state differs from expected transition".into());
    }
    for leaf in 0..index.leaves().len() {
        index.path(leaf as u32)?;
    }
    if let Some(store) = store {
        store.save_if_unchanged(&index, previous_tip)?;
    }
    println!(
        "verified {} pool leaves through block {} on chain {}",
        index.leaves().len(),
        through,
        chain_id
    );
    Ok(())
}
