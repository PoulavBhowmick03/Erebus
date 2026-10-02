//! Funded crash sweep. One locally generated proof is reused only for the identical
//! deployment, agreement, input note, path, and change across fresh pinned Anvil forks.

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use erebus_coordinator::{Coordinator, Stage};
use erebus_core::{
    policy::SpendingPolicy,
    settlement::{PreparedSettlement, SettlementContext},
};
use erebus_evm::chain::{Eip1559Fees, SignerJournal, TransactionKey};
use erebus_journal::{Boundary, FaultHook, NoFaults, Step};
use erebus_shielded_prover::{
    chain::ShieldedChain,
    index_store::{IndexDomain, IndexStore},
    preparation::{capabilities, prepare_transfer_witness},
    recovery::recover_wallet,
    rpc::PoolRpc,
    wallet::{OwnedNote, WalletDomain, WalletStore},
    WalletTransferRequest,
};
use sha3::{Digest, Keccak256};

type Failure = Box<dyn std::error::Error + Send + Sync>;

struct Fork {
    child: std::process::Child,
    url: String,
}

impl Fork {
    async fn start(source: &str, block: u64, chain_id: u64) -> Result<Self, Failure> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        let child = std::process::Command::new("anvil")
            .args([
                "--port",
                &port.to_string(),
                "--chain-id",
                &chain_id.to_string(),
                "--fork-url",
                source,
                "--fork-block-number",
                &block.to_string(),
                "--slots-in-an-epoch",
                "1",
                "--silent",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let fork = Self {
            child,
            url: format!("http://127.0.0.1:{port}"),
        };
        for _ in 0..100 {
            let response = reqwest::Client::new()
                .post(&fork.url)
                .json(&serde_json::json!({
                    "jsonrpc":"2.0", "id":1, "method":"eth_chainId", "params":[]
                }))
                .send()
                .await;
            if response.is_ok() {
                return Ok(fork);
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        Err("funded trial fork did not start".into())
    }
}

impl Drop for Fork {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Sweep {
    base: PathBuf,
    target: Option<(Step, String, usize)>,
    count: AtomicUsize,
    failures: AtomicUsize,
    steps: Mutex<Vec<(Step, String)>>,
}

impl FaultHook for Sweep {
    fn after(&self, boundary: Boundary<'_>) -> std::io::Result<()> {
        let path = boundary
            .path
            .strip_prefix(&self.base)
            .expect("boundary belongs to this trial");
        let stable = match path.extension().and_then(|extension| extension.to_str()) {
            Some("tmp") if boundary.step == Step::FileSynced => path.with_file_name("atomic.tmp"),
            Some(extension)
                if boundary.step == Step::FileSynced && extension.starts_with("tmp-") =>
            {
                path.with_extension("tmp")
            }
            _ => path.to_path_buf(),
        };
        let name = stable.to_string_lossy().into_owned();
        let mut steps = self.steps.lock().unwrap();
        let occurrence = steps
            .iter()
            .filter(|(step, path)| *step == boundary.step && *path == name)
            .count()
            + 1;
        steps.push((boundary.step, name.clone()));
        self.count.fetch_add(1, Ordering::SeqCst);
        if self.target.as_ref() == Some(&(boundary.step, name, occurrence)) {
            self.failures.fetch_add(1, Ordering::SeqCst);
            Err(std::io::Error::other("funded matrix crash"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn crash_targets_ignore_random_temp_names_and_keep_observers_separate() {
    let hook = Sweep {
        base: PathBuf::from("/trial"),
        target: Some((Step::FileSynced, "primary/index.tmp".to_owned(), 1)),
        count: AtomicUsize::new(0),
        failures: AtomicUsize::new(0),
        steps: Mutex::new(Vec::new()),
    };
    assert!(hook
        .after(Boundary {
            step: Step::FileSynced,
            path: Path::new("/trial/peer/index.tmp-1234")
        })
        .is_ok());
    assert!(hook
        .after(Boundary {
            step: Step::FileSynced,
            path: Path::new("/trial/primary/index.tmp-abcd")
        })
        .is_err());
    assert!(hook
        .after(Boundary {
            step: Step::FileSynced,
            path: Path::new("/trial/primary/index.tmp-5678")
        })
        .is_ok());
    assert_eq!(hook.failures.load(Ordering::SeqCst), 1);
}

pub struct Case<'a> {
    pub root: &'a Path,
    pub rpc_url: &'a str,
    pub context: &'a SettlementContext,
    pub policy: &'a SpendingPolicy,
    pub input: &'a OwnedNote,
    pub request: &'a WalletTransferRequest<'a>,
    pub prepared: &'a PreparedSettlement,
    pub key: &'a TransactionKey,
    pub index_domain: IndexDomain,
}

impl Case<'_> {
    fn open(
        &self,
        root: &Path,
        hook: Arc<dyn FaultHook>,
    ) -> Result<(Coordinator, SignerJournal, WalletStore, IndexStore), Failure> {
        let coordinator = Coordinator::open_with_faults(
            root.join("coordinator"),
            self.request.terms.buyer_authorization_key.clone(),
            self.context.clone(),
            self.context,
            &capabilities(),
            self.policy.clone(),
            hook.clone(),
        )?;
        let signer = SignerJournal::open_with_faults(
            root.join("signer"),
            self.index_domain.chain_id,
            self.key.address(),
            hook.clone(),
        )?;
        let wallet = WalletStore::with_faults(
            root.join("private/wallet.enc"),
            WalletDomain {
                chain_id: self.index_domain.chain_id,
                pool: self.index_domain.pool,
            },
            [7; 32],
            hook.clone(),
        )?;
        let index =
            IndexStore::with_faults(root.join("primary/index.json"), self.index_domain, hook)?;
        Ok((coordinator, signer, wallet, index))
    }

    async fn rpc(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, Failure> {
        let result: serde_json::Value = reqwest::Client::new()
            .post(self.rpc_url)
            .json(&serde_json::json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params}))
            .send()
            .await?
            .json()
            .await?;
        if result.get("error").is_some() {
            return Err("Anvil matrix request failed".into());
        }
        Ok(result["result"].clone())
    }

    async fn drive(&self, root: &Path, hook: Arc<dyn FaultHook>) -> Result<(), Failure> {
        let (coordinator, journal, wallet, index) = self.open(root, hook.clone())?;
        let operation = self.prepared.operation_ref;
        let chain = ShieldedChain::connect(
            self.context.clone(),
            self.rpc_url,
            std::time::Duration::from_secs(10),
        )
        .await?;
        let rpc = PoolRpc::new(
            self.rpc_url,
            self.index_domain.chain_id,
            self.index_domain.pool,
        )?;
        let peer_proxy = super::FaultProxy::start(self.rpc_url).await;
        *peer_proxy.state.selected.lock().unwrap() = None;
        let peer = ShieldedChain::connect(
            self.context.clone(),
            &peer_proxy.url,
            std::time::Duration::from_secs(10),
        )
        .await?;
        let peer_rpc = PoolRpc::new(
            &peer_proxy.url,
            self.index_domain.chain_id,
            self.index_domain.pool,
        )?;
        let peer_index =
            IndexStore::with_faults(root.join("peer/index.json"), self.index_domain, hook)?;
        if coordinator
            .diagnostics()?
            .first()
            .is_some_and(|d| d.stage == Stage::Finalized)
        {
            chain
                .reconcile(
                    &peer,
                    &coordinator,
                    &journal,
                    &rpc,
                    &index,
                    &peer_rpc,
                    &peer_index,
                    &wallet,
                    operation,
                    1005,
                )
                .await?;
            return Ok(());
        }
        // A prior send may have landed even if persistence or the response failed.
        // Observe it before deciding whether the exact signed bytes need broadcasting.
        if coordinator.prepared_settlement(operation).is_ok()
            && !coordinator.broadcast_history(operation)?.is_empty()
        {
            let state = chain
                .reconcile(
                    &peer,
                    &coordinator,
                    &journal,
                    &rpc,
                    &index,
                    &peer_rpc,
                    &peer_index,
                    &wallet,
                    operation,
                    1005,
                )
                .await?;
            if matches!(
                state.state,
                erebus_core::deal_state::DealState::PaidFinalized { .. }
            ) {
                return Ok(());
            }
        }
        coordinator.record_intent(operation, self.request.terms, self.request.blinding, 1000)?;
        coordinator.authorize_buyer(operation, 1001, |_, _| {
            Ok::<_, ()>(self.request.buyer.clone())
        })?;
        coordinator.accept_seller(operation, self.request.seller)?;
        coordinator.prepare(operation, 1002, |_, _, _, _| {
            prepare_transfer_witness(
                self.context,
                &wallet,
                self.request,
                self.input.commitment(),
                operation,
            )?;
            Ok::<_, erebus_shielded_prover::preparation::PreparationError>(self.prepared.clone())
        })?;
        chain
            .sign(
                &coordinator,
                &journal,
                operation,
                self.key,
                1003,
                Eip1559Fees::new(2_000_000_000, 1_000_000_000)?,
                3_000_000,
            )
            .await?;
        chain.replace(
            &coordinator,
            operation,
            self.key,
            1003,
            Eip1559Fees::new(3_000_000_000, 1_500_000_000)?,
        )?;
        chain.broadcast(&coordinator, operation, 1004).await?;
        self.rpc("anvil_mine", serde_json::json!(["0x50"])).await?;
        let assessment = chain
            .reconcile(
                &peer,
                &coordinator,
                &journal,
                &rpc,
                &index,
                &peer_rpc,
                &peer_index,
                &wallet,
                operation,
                1005,
            )
            .await?;
        if !matches!(assessment.state, erebus_core::deal_state::DealState::PaidFinalized { commitment } if commitment == self.prepared.deal_commitment)
        {
            return Err("funded matrix did not observe the accepted payment".into());
        }
        Ok(())
    }

    pub async fn run(&self) -> Result<(), Failure> {
        self.rpc("anvil_mine", serde_json::json!(["0x50"])).await?;
        let head = self.rpc("eth_blockNumber", serde_json::json!([])).await?;
        let base_block =
            u64::from_str_radix(head.as_str().unwrap().strip_prefix("0x").unwrap(), 16)?;
        let mut boundaries = 0;
        let mut discovered = Vec::new();
        let mut targets: Vec<(Step, String, usize)> = Vec::new();
        let mut fail = 0;
        loop {
            // Fresh nodes avoid historical-state cache failures after repeated snapshot restores.
            let fork = Fork::start(self.rpc_url, base_block, self.index_domain.chain_id).await?;
            let trial = Case {
                rpc_url: &fork.url,
                ..*self
            };
            let nonce_params =
                serde_json::json!([format!("0x{}", hex::encode(self.key.address())), "latest"]);
            let before_nonce = trial
                .rpc("eth_getTransactionCount", nonce_params.clone())
                .await?;
            let root = self.root.join(format!("fault-{fail}"));
            // Initialization is outside the operation sweep; it belongs to the onboarding gate.
            let (_, _, wallet, index) = self.open(&root, Arc::new(NoFaults))?;
            wallet.update(|state| state.add(self.input.clone()))?;
            let rpc = PoolRpc::new(
                trial.rpc_url,
                self.index_domain.chain_id,
                self.index_domain.pool,
            )?;
            recover_wallet(&rpc, &index, &wallet, 0).await?;
            let hook = Arc::new(Sweep {
                base: root.clone(),
                target: if fail == 0 {
                    None
                } else {
                    Some(targets[fail - 1].clone())
                },
                count: AtomicUsize::new(0),
                failures: AtomicUsize::new(0),
                steps: Mutex::new(Vec::new()),
            });
            let result = trial.drive(&root, hook.clone()).await;
            if fail == 0 {
                result?;
                boundaries = hook.count.load(Ordering::SeqCst);
                for (step, name) in hook.steps.lock().unwrap().iter() {
                    let occurrence = targets
                        .iter()
                        .filter(|(prior_step, prior_name, _)| {
                            prior_step == step && prior_name == name
                        })
                        .count()
                        + 1;
                    targets.push((*step, name.clone(), occurrence));
                    discovered.push(format!("{step:?}: {name} occurrence {occurrence}"));
                }
                assert!(boundaries > 0);
            } else {
                assert!(
                    result.is_err(),
                    "boundary {fail} must interrupt the funded operation"
                );
                // Both observers can finish independent writes after the injected error.
                // Target by path, step, and occurrence, not their concurrent global order.
                assert_eq!(hook.failures.load(Ordering::SeqCst), 1);
                // Finality may lag the transaction that reached the chain before a storage error.
                trial.rpc("anvil_mine", serde_json::json!(["0x50"])).await?;
                trial.drive(&root, Arc::new(NoFaults)).await?;
            }
            let (coordinator, journal, wallet, _) = self.open(&root, Arc::new(NoFaults))?;
            assert_eq!(coordinator.diagnostics()?[0].stage, Stage::Finalized);
            assert_eq!(coordinator.ledger()?.reservations().len(), 1);
            assert_eq!(
                coordinator
                    .ledger()?
                    .reserved_total(&self.context.asset)?
                    .get(),
                0
            );
            assert!(wallet
                .snapshot()?
                .spendable_for(&self.input.commitment(), self.prepared.operation_ref)
                .is_none());
            assert_eq!(
                wallet
                    .snapshot()?
                    .select(&self.input.asset(), 80)
                    .expect("funded change")
                    .amount(),
                80
            );
            let evidence = rpc
                .consumed_deal_at(
                    *self.prepared.deal_nullifier.as_bytes(),
                    rpc.finalized_head().await?.hash,
                )
                .await?;
            assert!(evidence, "actual pool consumption at boundary {fail}");
            let after_nonce = trial.rpc("eth_getTransactionCount", nonce_params).await?;
            let number = |value: &serde_json::Value| {
                u64::from_str_radix(value.as_str().unwrap().strip_prefix("0x").unwrap(), 16)
                    .unwrap()
            };
            assert_eq!(
                number(&after_nonce),
                number(&before_nonce) + 1,
                "one consumed nonce, no gaps"
            );
            let logs = trial.rpc("eth_getLogs", serde_json::json!([{
                "address": format!("0x{}", hex::encode(self.index_domain.pool)),
                "fromBlock": format!("0x{:x}", self.index_domain.first_block),
                "toBlock": "latest",
                "topics": [format!("0x{}", hex::encode(Keccak256::digest(b"DealTransferred(uint256,uint256,uint256)"))),
                    format!("0x{}", hex::encode(self.prepared.deal_commitment.as_bytes())),
                    format!("0x{}", hex::encode(self.prepared.deal_nullifier.as_bytes()))]
            }])).await?;
            assert_eq!(
                logs.as_array().unwrap().len(),
                1,
                "one actual transfer event"
            );
            let deployment = erebus_evm::deployment::EvmDeployment::new(
                self.context.domain.namespace.clone(),
                self.index_domain.pool,
                self.context.domain.verifier_version,
                self.rpc_url,
            )?;
            assert!(
                journal
                    .resume_call(
                        &deployment,
                        self.prepared,
                        self.index_domain.pool,
                        &self.prepared.backend_evidence
                    )?
                    .is_none(),
                "finalized nonce released"
            );
            println!("M6 funded boundary {fail}/{boundaries} recovered");
            fail += 1;
            if fail > boundaries {
                break;
            }
        }
        std::fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "testOnly": true,
                "chainId": self.index_domain.chain_id,
                "pool": format!("0x{}", hex::encode(self.index_domain.pool)),
                "boundaries": boundaries,
                "completedTrials": fail,
                "discoveredSteps": discovered,
                "oneTransferAndNoncePerTrial": true,
                "finalizedWalletAndAccountingRecovered": true
            }))?,
        )?;
        Ok(())
    }
}
