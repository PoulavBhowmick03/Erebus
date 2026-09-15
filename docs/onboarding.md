# Installed onboarding

This flow is implemented in the unreleased **0.3.0 / Protocol 5** packages. It is not
available from the public 0.2.0 wheels. Release the three matching packages together.

## User flow

After installing `erebus-mcp-server`, run:

```bash
erebus-init
```

Select an existing account or type `new`. Choose the network and provide the RPC and prover
endpoints. Mainnet's Starkscan prover also needs a Starkscan API key, entered at a hidden
prompt or provided through `STARKSCAN_API_KEY`.

The initializer prints the account address, public balance, and funding shortfall. Send
STRK to that address on the selected network. The target includes the shield deposit,
live pool fees, and an estimated gas reserve. Gas is an estimate, not a guaranteed quote.
No fixed historical fee is presented as the current pool fee.

Once funded, review the plan and confirm. Setup deploys a new account when necessary,
approves the required allowance, waits for proving depth, and shields the chosen deposit.
It runs `doctor` and prints the MCP launch command when ready.

The terminal waits up to five minutes for funding and confirmation. If it times out or
you close it, continue with the printed `erebus-init --config ... --resume` command.
The selected address, keys, and operation IDs survive the restart.

Setup defaults to a 1 STRK deposit for a new identity and budgets three subsequent charged
writes. Reusing an existing Erebus config defaults to no additional deposit. Change the
initial plan with `--deposit <STRK>` and `--writes <count>`.

## Agents and automation

Discover local accounts without submitting transactions:

```bash
erebus-init --list-accounts --json
```

Select the exact returned ID, or create a dedicated account:

```bash
erebus-init --new --network sepolia \
  --config /absolute/path/buyer.env \
  --rpc-url https://your-rpc.example \
  --prover-url https://your-prover.example \
  --deposit 1 --writes 3 --json
```

Use `--account <id>` instead of `--new` to select a discovered account. An unattended
initializer never chooses a wallet implicitly. Addresses without a usable local signer
are reported rather than treated as spendable wallets.

After reviewing the returned plan, authorize setup:

```bash
erebus-init --config /absolute/path/buyer.env --resume --yes --json --wait 300
```

`--json` produces one JSON result on stdout. Waiting updates go to stderr. Exit code 0
means `ready` or a successful account listing. Exit code 2 means an input, funding,
confirmation, or repair is still required. Results include a resume command.

Common statuses are `selection_required`, `configuration_required`, `funding_required`,
`authorization_required`, `deployment_pending`, `approval_pending`, `shield_pending`,
`pool_key_required`, `recovery_required`, `repair_required`, and `ready`.

## Both sides of a negotiation

Onboarding provisions one identity, and a negotiation needs two — run it once per side. Both
sides must be able to write: each opens its own channel direction, and offers are read through
that direction, so a counterparty that has not opened sees `unknown channel handle` rather than
the offer. A payee that never pays the price still needs an allowance for two charged writes
(its open and its counter).

The plan already budgets the shield deposit, the live pool fee, and an estimated gas reserve,
because the pool fee is pulled from the identity's **public STRK** through `transfer_from`. A
healthy shielded balance and a large allowance are both insufficient if the public balance is
low; a settlement then fails with `INSUFFICIENT_BALANCE` naming the shortfall. Fund each side to
the target the plan prints before starting a session.

## Mock mode

```bash
erebus-init --network mock
erebus-mcp-server
```

Mock mode needs no wallet, endpoint credentials, or funds. Use `--config` to keep the mock
configuration separate from an existing real identity.

## Wallet support and files

Discovery checks Erebus configuration directories and the standard `sncast` accounts file.
Use `--accounts-file` for another local `sncast` file. Browser extensions and hardware
wallets are not discovered by this release. Automatic signing supports OpenZeppelin
single-key accounts; unsupported signers require a separate integration.

New accounts use the OpenZeppelin v1.0.0 class pinned in Starknet Foundry. The packaged
Rust binary generates account and pool keys and imports local account keys. Python receives
public metadata and file paths only. New key and setup files have mode `0600`.

An existing registered pool identity requires its original pool key. Setup does not replace
that key. Keep the config, its `.setup.json` file, identity directory, and Rust state directory
together for recovery. The setup file can contain endpoint credentials; do not share it.

The RPC and prover receive the pool key during proving. Registration also encrypts the pool
key to the pool auditor. Choose these services before authorizing registration.

Account deployment saves the signed request before submission. Approval and shielding use
the Rust operation journal. An uncertain result is checked on resume; it is not treated as
proof that the transaction failed. Existing unrelated pending operations require recovery
before onboarding starts another write.

An explicit `erebus-mcp-server --config <path>` uses that file's identity values even if the
launcher inherited another wallet's environment. Without `--config`, existing environment
values retain precedence over an automatically discovered configuration.

## Validate the package installation

From a development checkout:

```bash
cargo build --manifest-path sdk/rs/Cargo.toml --bin erebus-cli
uv run python scripts/check-onboarding-install.py
```

This builds temporary wheels and installs them with `uv tool install` outside the checkout.
It checks mock initialization and structured missing prerequisites with no `erebus-cli` or
`sncast` on `PATH`. A local RPC checks new account creation and funding-pause recovery.
The installed MCP server must initialize and expose all thirteen tools. No live transaction
is submitted.
