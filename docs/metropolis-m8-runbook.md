# Metropolis M8 Runbook: Monad Testnet Deployment

Read [M8 progress](metropolis-m8-progress.md) first. This runbook covers the public-bound
settlement contract only. Shielded deployment and the shared services are separate.

**Deploying spends testnet funds and is irreversible.** Do it only with an explicit operator
decision and a dedicated, funded testnet key. Never use a key that holds value on any network.

## 0. Monad facts and prerequisites

From the official docs (fetched 2026-10-02):

| Fact | Value |
|---|---|
| Testnet chain ID / currency | 10143 / MON |
| Testnet RPC | `https://testnet-rpc.monad.xyz` (50 rps; 25 rps for `eth_call`/`eth_estimateGas`), `https://rpc.ankr.com/monad_testnet`, `https://rpc-testnet.monadinfra.com` |
| Faucet | `https://faucet.monad.xyz` |
| Explorers | `https://testnet.monadvision.com`, `https://testnet.monadscan.com` |
| Current testnet release | v0.15.2 / `MONAD_NINE`; the chain was reset from genesis on 2025-12-16 |
| Foundry | **v1.8 or later** for Monad execution (`network = "monad"`). The legacy Monad Foundry fork is deprecated. This repo's local toolchain is 1.5.1, so the `[profile.monad]` in `contracts/evm/foundry.toml` is inert until the toolchain is upgraded; RPC deployment works either way |
| Size limits | 128 KB contract size, 256 KB initcode; `ErebusSettlement` is about 15 KB runtime |

Canonical testnet contracts that matter to later M8 work: Permit2
`0x000000000022d473030f116ddee9f6b43ac78ba3`, Multicall3
`0xcA11bde05977b3631167028862bE2a173976CA11`, and the x402 proxies
`0x402085c248EeA27D92E8b30b2C58ed07f9E20001` (Exact) and
`0x4020A4f3b7b90ccA423B9fabCc0CE57C6C240002` (Upto).

## 1. Verify the network before deploying

```sh
cargo build --manifest-path sdk/evm/Cargo.toml --locked --bin erebus-network-check
echo '{"chain_id":10143,"rpc_url":"https://testnet-rpc.monad.xyz","timeout_ms":10000}' \
  | ./sdk/evm/target/debug/erebus-network-check
```

Exit 0 requires every check to pass: chain ID, an explicit `finalized` block, hash-pinned
`eth_getTransactionCount`/`eth_getBalance`/`eth_getCode`/`eth_getLogs`, rejection of an unknown
block hash, and the BN254 add, mul, and pairing precompiles (valid and invalid vectors). The
report never contains the endpoint. A passing report is network compatibility evidence, not
deployment or payment evidence.

Recorded live result: all 16 checks passed against `testnet-rpc.monad.xyz`, chain ID 10143,
finalized block 67489534, head 67489537 (2026-10-02). See the progress record.

## 2. Build and record the artifact

```sh
cd contracts/evm
forge build
git -C ../.. rev-parse HEAD
```

The deployment manifest (`deployments/<network>.json`, shape in `deployments.example.json`)
records the chain ID, address, transaction, verifier version, artifact path, and source commit.
Commit the manifest, never the key.

## 3. Deploy

`ErebusSettlement`'s constructor is `(uint256 expectedChainId, uint32 expectedVerifierVersion)`
and reverts with `ChainIdMismatch` when the live chain differs, so a wrong-network deployment
cannot succeed.

```sh
export MONAD_RPC=https://testnet-rpc.monad.xyz
# Keep the key in a file readable only by you; never pass it on a shared command line in logs.
read -rs MONAD_DEPLOY_KEY
BYTECODE=$(python3 -c "import json;print(json.load(open('out/ErebusSettlement.sol/ErebusSettlement.json'))['bytecode']['object'][2:])")
ARGS=$(cast abi-encode "constructor(uint256,uint32)" 10143 1)
cast send --rpc-url "$MONAD_RPC" --private-key "$MONAD_DEPLOY_KEY" --json \
  --create "${BYTECODE}${ARGS#0x}"
```

Take `contractAddress` from the JSON receipt. `forge create` is an alternative, but its
constructor-argument parser rejects the plain argument list this constructor needs, so the
`cast send --create` form above is the tested path.

## 4. Verify the deployment before trusting it

Finality first: Monad's `finalized` tag trails the head. Wait until the deployment block is at
or below `finalized` before reading it.

```sh
python3 scripts/check-evm-deployment.py \
  --rpc-url "$MONAD_RPC" --address "$CONTRACT" --chain-id 10143 --verifier-version 1
```

Exit 0 requires the on-chain runtime code to match the reviewed artifact byte-for-byte with
immutable slots masked, and `verifierVersion()` to equal the manifest. Record the output in the
manifest's evidence. If the code differs, the address is not the reviewed contract: stop.

## 5. What is not covered here

- Shielded pool deployment and verifier-key publication: [M5 progress](metropolis-m5-progress.md).
- Shared services (message relay, transaction relayer, pool indexer): each has its own README
  and the [M6 runbook](metropolis-m6-runbook.md); hosted deployment needs measured capacity,
  quotas, retention, and an incident owner before launch (D05, D07).
- Access issuance, x402 composition, and the installed developer workflow: [M8 progress](metropolis-m8-progress.md).
- Mainnet: a separate release decision after testnet evidence and review. This runbook does not
  authorize it.
