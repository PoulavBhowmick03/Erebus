//! Local proof generation for the experimental Erebus shielded EVM pool.
//!
//! This crate reads only caller-supplied, hash-pinned artifacts. It does not publish
//! proving keys, choose a deployment, upload witnesses, or declare M5 release readiness.

pub mod chain;
pub mod index_store;
pub mod indexer;
pub mod observation;
pub mod preparation;
pub mod recovery;
pub mod rpc;
pub mod wallet;

use std::{fs::File, io::Read, path::PathBuf, str::FromStr};

use ark_bn254::{Bn254, Fr};
use ark_circom::{read_zkey, CircomBuilder, CircomConfig, CircomReduction};
use ark_crypto_primitives::snark::SNARK;
use ark_groth16::{Groth16, Proof};
use ark_std::rand::rngs::OsRng;
use erebus_core::{
    auth::Authorization,
    commitment::CommitmentBlinding,
    shielded::{
        note_commitment, note_nullifier, note_root_from_path, note_spend_tag, ShieldedDeal,
        NOTE_TREE_DEPTH,
    },
    shielded_auth::verify_agreement,
    terms::AgreementTerms,
};
use num_bigint::BigInt;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::{
    indexer::PoolIndex,
    wallet::{OwnedNote, WalletDomain, WalletSnapshot},
};

/// Paths and expected digests for one compiled circuit and proving key.
#[derive(Debug, Clone)]
pub struct ProvingArtifacts {
    /// Circom witness generator binary.
    pub wasm: PathBuf,
    /// Circuit constraints.
    pub r1cs: PathBuf,
    /// snarkjs Groth16 proving key.
    pub zkey: PathBuf,
    /// Expected SHA-256 of the WASM bytes.
    pub wasm_sha256: String,
    /// Expected SHA-256 of the R1CS bytes.
    pub r1cs_sha256: String,
    /// Expected SHA-256 of the zkey bytes.
    pub zkey_sha256: String,
}

/// A locally generated proof whose public inputs were checked before proving.
#[derive(Debug)]
pub struct LocalProof {
    /// Groth16 proof, suitable for backend-specific calldata encoding.
    pub proof: Proof<Bn254>,
    /// Public inputs in circuit order.
    pub public_inputs: Vec<Fr>,
}

impl LocalProof {
    /// Exports `[a, b, c, publicInputs]` for the snarkjs-generated Solidity verifier.
    ///
    /// BN254 G2 coordinates are reversed within each pair for the EVM pairing precompile.
    /// The proof and public inputs contain no private witness fields.
    pub fn solidity_calldata(&self) -> Value {
        let proof = &self.proof;
        serde_json::json!([
            [proof.a.x.to_string(), proof.a.y.to_string()],
            [
                [proof.b.x.c1.to_string(), proof.b.x.c0.to_string()],
                [proof.b.y.c1.to_string(), proof.b.y.c0.to_string()]
            ],
            [proof.c.x.to_string(), proof.c.y.to_string()],
            self.public_inputs
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        ])
    }
}

/// One buyer-owned note opening. Its spend secret must remain on the local operator machine.
pub struct InputNote {
    /// Note value in token base units.
    pub amount: u128,
    /// Canonical BN254 spend secret.
    pub spend_secret: [u8; 32],
    /// Canonical BN254 note salt.
    pub salt: [u8; 32],
}

impl Drop for InputNote {
    fn drop(&mut self) {
        self.spend_secret.zeroize();
        self.salt.zeroize();
    }
}

/// One depth-20 inclusion path and the root the backend is expected to recognize.
#[derive(Clone)]
pub struct NotePath {
    /// Zero-based leaf index.
    pub index: u32,
    /// Siblings from leaf to root.
    pub siblings: [[u8; 32]; NOTE_TREE_DEPTH],
    /// Claimed public root, checked against the path before proving.
    pub root: [u8; 32],
}

/// Buyer-owned change note opening. Persist this secret before submitting the transfer.
pub struct ChangeNote {
    /// Buyer-held secret from which the circuit's destination tag is derived.
    pub spend_secret: [u8; 32],
    /// Buyer-selected note salt.
    pub salt: [u8; 32],
}

impl Drop for ChangeNote {
    fn drop(&mut self) {
        self.spend_secret.zeroize();
        self.salt.zeroize();
    }
}

impl ChangeNote {
    /// Reconstructs the buyer's change opening for local encrypted storage.
    ///
    /// Save a nonzero result before submitting its transfer. An exact-value input has
    /// a zero-value change output in the circuit but no spendable wallet note.
    pub fn owned_note(
        &self,
        terms: &AgreementTerms,
        input_amount: u128,
    ) -> Result<Option<OwnedNote>, ProverError> {
        ShieldedDeal::from_terms(terms).map_err(|_| ProverError::Transfer("agreement shape"))?;
        let amount = input_amount
            .checked_sub(terms.amount.get())
            .ok_or(ProverError::Transfer("insufficient input note"))?;
        if amount == 0 {
            return Ok(None);
        }
        let asset: [u8; 20] = hex::decode(&terms.asset.asset_reference()[2..])
            .map_err(|_| ProverError::Transfer("asset"))?
            .try_into()
            .map_err(|_| ProverError::Transfer("asset"))?;
        let owner: [u8; 64] = terms
            .buyer_authorization_key
            .as_bytes()
            .try_into()
            .map_err(|_| ProverError::Transfer("buyer key"))?;
        let note = OwnedNote::new(asset, amount, owner, self.spend_secret, self.salt)
            .map_err(|_| ProverError::Transfer("change opening"))?;
        Ok(Some(note))
    }
}

/// Exact Circom input and expected public signals for one fixed-shape transfer.
pub struct TransferWitness {
    /// Private and public circuit input. Treat as secret material.
    pub input: Value,
    /// Public signals in `transfer.circom` order.
    pub public_inputs: Vec<String>,
}

/// Exact Circom input and public signals for a deposit or withdrawal.
pub struct NoteWitness {
    /// Private and public circuit input. Treat as secret material.
    pub input: Value,
    /// Public signals in circuit order.
    pub public_inputs: Vec<String>,
}

/// Inputs owned by the local operator for one fixed-shape private transfer.
pub struct TransferRequest<'a> {
    /// Canonical accepted terms.
    pub terms: &'a AgreementTerms,
    /// Private opening of the agreed commitment.
    pub blinding: &'a CommitmentBlinding,
    /// Buyer authorization of the same commitment.
    pub buyer: &'a Authorization,
    /// Seller authorization of the same commitment.
    pub seller: &'a Authorization,
    /// Buyer-owned input note.
    pub note: &'a InputNote,
    /// Inclusion path against a proposed public root.
    pub path: &'a NotePath,
    /// Buyer-owned change destination.
    pub change: &'a ChangeNote,
    /// Current time for expiry validation.
    pub now: u64,
}

/// Inputs for constructing one transfer from a locally indexed wallet note.
pub struct WalletTransferRequest<'a> {
    /// Canonical accepted terms.
    pub terms: &'a AgreementTerms,
    /// Private commitment opening.
    pub blinding: &'a CommitmentBlinding,
    /// Both authorizations over the accepted agreement.
    pub buyer: &'a Authorization,
    /// Seller authorization.
    pub seller: &'a Authorization,
    /// Local decrypted note inventory.
    pub wallet: &'a WalletSnapshot,
    /// Locally verified public pool history.
    pub index: &'a PoolIndex,
    /// Buyer-owned change destination.
    pub change: &'a ChangeNote,
    /// Current time for expiry validation.
    pub now: u64,
}

/// A local artifact, input, or proof failed validation.
#[derive(Debug, thiserror::Error)]
pub enum ProverError {
    /// An expected hash was malformed or did not match the file.
    #[error("artifact hash mismatch for {0}")]
    Artifact(&'static str),
    /// The witness JSON is not an object of canonical decimal values.
    #[error("invalid witness input: {0}")]
    Input(&'static str),
    /// The computed public signals differ from the caller's authorized expectation.
    #[error("computed public inputs differ from the expected transition")]
    PublicInputMismatch,
    /// The local circuit, key, or proof engine failed.
    #[error("local Groth16 proving failed during {0}")]
    Proof(&'static str),
    /// The native proof did not verify against its own pinned zkey.
    #[error("local proof did not verify")]
    Verification,
    /// The accepted agreement, note, or tree path does not match the transfer.
    #[error("invalid shielded transfer: {0}")]
    Transfer(&'static str),
}

fn decimal_bytes(bytes: &[u8; 32]) -> String {
    num_bigint::BigUint::from_bytes_be(bytes).to_string()
}

fn insert_field(input: &mut Map<String, Value>, name: &str, bytes: &[u8; 32]) {
    input.insert(name.to_owned(), Value::String(decimal_bytes(bytes)));
}

fn insert_number(input: &mut Map<String, Value>, name: &str, value: impl ToString) {
    input.insert(name.to_owned(), Value::String(value.to_string()));
}

fn address_field(address: &[u8; 20]) -> [u8; 32] {
    let mut field = [0u8; 32];
    field[12..].copy_from_slice(address);
    field
}

fn pool_fields(
    input: &mut Map<String, Value>,
    domain: WalletDomain,
    version: u32,
    asset: [u8; 20],
) -> Result<(), ProverError> {
    if domain.chain_id == 0 || domain.pool == [0; 20] || asset == [0; 20] || version == 0 {
        return Err(ProverError::Transfer("invalid pool domain"));
    }
    insert_number(input, "chainId", domain.chain_id);
    insert_field(input, "contractAddress", &address_field(&domain.pool));
    insert_number(input, "verifierVersion", version);
    insert_field(input, "asset", &address_field(&asset));
    insert_number(input, "privateChainId", domain.chain_id);
    insert_field(
        input,
        "privateContractAddress",
        &address_field(&domain.pool),
    );
    insert_number(input, "privateVerifierVersion", version);
    Ok(())
}

/// Builds a deposit witness for an unspent local note opening.
pub fn build_deposit_witness(
    note: &OwnedNote,
    domain: WalletDomain,
    version: u32,
) -> Result<NoteWitness, ProverError> {
    let mut input = Map::new();
    pool_fields(&mut input, domain, version, note.asset())?;
    let opening = note.input_note();
    let owner = note.owner();
    let tag = note_spend_tag(&opening.spend_secret)
        .map_err(|_| ProverError::Transfer("deposit spend secret"))?;
    insert_number(&mut input, "amount", note.amount());
    insert_field(&mut input, "noteCommitment", &note.commitment());
    insert_field(
        &mut input,
        "ownerAx",
        &owner[..32].try_into().expect("owner width"),
    );
    insert_field(
        &mut input,
        "ownerAy",
        &owner[32..].try_into().expect("owner width"),
    );
    insert_field(&mut input, "spendTag", &tag);
    insert_field(&mut input, "salt", &opening.salt);
    let public_inputs = [
        "chainId",
        "contractAddress",
        "verifierVersion",
        "asset",
        "amount",
        "noteCommitment",
    ]
    .iter()
    .map(|name| input[*name].as_str().expect("fixed field").to_owned())
    .collect();
    Ok(NoteWitness {
        input: Value::Object(input),
        public_inputs,
    })
}

/// Builds a withdrawal witness from a confirmed, unspent local note.
///
/// Withdrawal exposes the recipient and amount publicly, unlike private transfer.
pub fn build_withdraw_witness(
    wallet: &WalletSnapshot,
    index: &PoolIndex,
    commitment: &[u8; 32],
    domain: WalletDomain,
    version: u32,
    recipient: [u8; 20],
) -> Result<NoteWitness, ProverError> {
    if recipient == [0; 20] {
        return Err(ProverError::Transfer("zero withdrawal recipient"));
    }
    let note = wallet
        .spendable(commitment)
        .ok_or(ProverError::Transfer("no spendable note"))?;
    let inclusion = note
        .inclusion()
        .ok_or(ProverError::Transfer("note not included"))?;
    if index.leaves().get(inclusion.index as usize) != Some(commitment) {
        return Err(ProverError::Transfer("note missing from verified tree"));
    }
    let path = index
        .path(inclusion.index)
        .map_err(|_| ProverError::Transfer("verified note path"))?;
    let opening = note.input_note();
    let owner = note.owner();
    let mut input = Map::new();
    pool_fields(&mut input, domain, version, note.asset())?;
    insert_field(&mut input, "root", &path.root);
    insert_field(&mut input, "noteNullifier", &note.nullifier());
    insert_field(&mut input, "recipient", &address_field(&recipient));
    insert_number(&mut input, "amount", note.amount());
    insert_field(
        &mut input,
        "ownerAx",
        &owner[..32].try_into().expect("owner width"),
    );
    insert_field(
        &mut input,
        "ownerAy",
        &owner[32..].try_into().expect("owner width"),
    );
    insert_field(&mut input, "spendSecret", &opening.spend_secret);
    insert_field(&mut input, "salt", &opening.salt);
    input.insert(
        "pathElements".to_owned(),
        Value::Array(
            path.siblings
                .iter()
                .map(|sibling| Value::String(decimal_bytes(sibling)))
                .collect(),
        ),
    );
    input.insert(
        "pathIndices".to_owned(),
        Value::Array(
            (0..NOTE_TREE_DEPTH)
                .map(|level| Value::String(((path.index >> level) & 1).to_string()))
                .collect(),
        ),
    );
    insert_field(&mut input, "privateRecipient", &address_field(&recipient));
    let public_inputs = [
        "chainId",
        "contractAddress",
        "verifierVersion",
        "asset",
        "root",
        "noteNullifier",
        "recipient",
        "amount",
    ]
    .iter()
    .map(|name| input[*name].as_str().expect("fixed field").to_owned())
    .collect();
    Ok(NoteWitness {
        input: Value::Object(input),
        public_inputs,
    })
}

/// Selects a confirmed, unspent note and derives its current-root Merkle path.
///
/// The caller must first sync the index to its chosen chain observation and reconcile
/// the wallet against that index. Proof submission and durable reservations belong to
/// the settlement coordinator, not to this witness-only helper.
pub fn build_transfer_witness_from_wallet(
    request: &WalletTransferRequest<'_>,
) -> Result<TransferWitness, ProverError> {
    ShieldedDeal::from_terms(request.terms)
        .map_err(|_| ProverError::Transfer("agreement shape"))?;
    let asset_hex = request.terms.asset.asset_reference();
    let asset: [u8; 20] = hex::decode(&asset_hex[2..])
        .map_err(|_| ProverError::Transfer("asset"))?
        .try_into()
        .map_err(|_| ProverError::Transfer("asset"))?;
    let note = request
        .wallet
        .notes()
        .iter()
        .filter(|note| {
            note.asset() == asset
                && note.owner().as_slice() == request.terms.buyer_authorization_key.as_bytes()
                && note.amount() >= request.terms.amount.get()
                && request.wallet.spendable(&note.commitment()).is_some()
        })
        .min_by_key(|note| (note.amount(), note.commitment()))
        .ok_or(ProverError::Transfer("no spendable note"))?;
    build_transfer_witness_with_note(request, note)
}

/// Builds a witness for one explicit input owned by the reserving operation.
pub fn build_transfer_witness_for_operation(
    request: &WalletTransferRequest<'_>,
    input: &[u8; 32],
    operation: [u8; 32],
) -> Result<TransferWitness, ProverError> {
    let note = request
        .wallet
        .spendable_for(input, operation)
        .ok_or(ProverError::Transfer("operation input unavailable"))?;
    build_transfer_witness_with_note(request, note)
}

fn build_transfer_witness_with_note(
    request: &WalletTransferRequest<'_>,
    note: &OwnedNote,
) -> Result<TransferWitness, ProverError> {
    if note.owner().as_slice() != request.terms.buyer_authorization_key.as_bytes()
        || format!("0x{}", hex::encode(note.asset())) != request.terms.asset.asset_reference()
    {
        return Err(ProverError::Transfer("input owner or asset mismatch"));
    }
    if request.index.is_consumed(&note.nullifier()) {
        return Err(ProverError::Transfer("input consumed in verified index"));
    }
    let inclusion = note
        .inclusion()
        .ok_or(ProverError::Transfer("note not included"))?;
    if request.index.leaves().get(inclusion.index as usize) != Some(&note.commitment()) {
        return Err(ProverError::Transfer("note missing from verified tree"));
    }
    let path = request
        .index
        .path(inclusion.index)
        .map_err(|_| ProverError::Transfer("verified note path"))?;
    let input = note.input_note();
    build_transfer_witness(&TransferRequest {
        terms: request.terms,
        blinding: request.blinding,
        buyer: request.buyer,
        seller: request.seller,
        note: &input,
        path: &path,
        change: request.change,
        now: request.now,
    })
}

/// Builds the fixed M5 transfer witness from verified Erebus agreement and local note data.
///
/// This does not select a backend, check that `path.root` remains on-chain, or send a
/// transaction. The caller must keep the returned private input out of logs and relays.
pub fn build_transfer_witness(
    request: &TransferRequest<'_>,
) -> Result<TransferWitness, ProverError> {
    let TransferRequest {
        terms,
        blinding,
        buyer,
        seller,
        note,
        path,
        change,
        now,
    } = request;
    let verified = verify_agreement(terms, blinding, buyer, seller, *now)
        .map_err(|_| ProverError::Transfer("agreement authorization"))?;
    let deal =
        ShieldedDeal::from_terms(terms).map_err(|_| ProverError::Transfer("agreement shape"))?;
    let asset_hex = terms.asset.asset_reference();
    let asset: [u8; 20] = hex::decode(&asset_hex[2..])
        .map_err(|_| ProverError::Transfer("asset"))?
        .try_into()
        .map_err(|_| ProverError::Transfer("asset"))?;
    let buyer_key: [u8; 64] = terms
        .buyer_authorization_key
        .as_bytes()
        .try_into()
        .map_err(|_| ProverError::Transfer("buyer key"))?;
    let spend_tag = note_spend_tag(&note.spend_secret)
        .map_err(|_| ProverError::Transfer("input spend secret"))?;
    let input_note = note_commitment(&asset, note.amount, &buyer_key, &spend_tag, &note.salt)
        .map_err(|_| ProverError::Transfer("input note"))?;
    let computed_root = note_root_from_path(&input_note, path.index, &path.siblings)
        .map_err(|_| ProverError::Transfer("input membership"))?;
    if computed_root != path.root {
        return Err(ProverError::Transfer("input root"));
    }
    let change_amount = note
        .amount
        .checked_sub(terms.amount.get())
        .ok_or(ProverError::Transfer("insufficient input note"))?;
    let change_spend_tag = note_spend_tag(&change.spend_secret)
        .map_err(|_| ProverError::Transfer("change spend secret"))?;
    let change_commitment = note_commitment(
        &asset,
        change_amount,
        &buyer_key,
        &change_spend_tag,
        &change.salt,
    )
    .map_err(|_| ProverError::Transfer("change note"))?;
    let input_nullifier = note_nullifier(&note.spend_secret, &input_note)
        .map_err(|_| ProverError::Transfer("input nullifier"))?;

    let mut input = Map::new();
    for (name, bytes) in deal
        .transfer_circuit_fields(blinding)
        .map_err(|_| ProverError::Transfer("agreement fields"))?
    {
        insert_field(&mut input, name, &bytes);
    }
    insert_field(&mut input, "root", &path.root);
    insert_field(&mut input, "inputNullifier", &input_nullifier);
    insert_field(&mut input, "changeCommitment", &change_commitment);
    for (label, authorization) in [("buyer", buyer), ("seller", seller)] {
        for (field, part) in ["R8x", "R8y", "S"]
            .into_iter()
            .zip(authorization.signature.as_bytes().chunks_exact(32))
        {
            let bytes: [u8; 32] = part
                .try_into()
                .map_err(|_| ProverError::Transfer("signature"))?;
            insert_field(&mut input, &format!("{label}{field}"), &bytes);
        }
    }
    insert_field(&mut input, "inputSalt", &note.salt);
    insert_field(&mut input, "inputSpendSecret", &note.spend_secret);
    insert_field(&mut input, "changeSpendTag", &change_spend_tag);
    insert_field(&mut input, "changeSalt", &change.salt);
    input.insert(
        "inputAmount".to_owned(),
        Value::String(note.amount.to_string()),
    );
    input.insert(
        "changeAmount".to_owned(),
        Value::String(change_amount.to_string()),
    );
    input.insert(
        "pathElements".to_owned(),
        Value::Array(
            path.siblings
                .iter()
                .map(|sibling| Value::String(decimal_bytes(sibling)))
                .collect(),
        ),
    );
    input.insert(
        "pathIndices".to_owned(),
        Value::Array(
            (0..NOTE_TREE_DEPTH)
                .map(|level| Value::String(((path.index >> level) & 1).to_string()))
                .collect(),
        ),
    );

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
    if input["dealCommitment"].as_str()
        != Some(decimal_bytes(verified.commitment.as_bytes()).as_str())
        || input["dealNullifier"].as_str()
            != Some(decimal_bytes(verified.nullifier.as_bytes()).as_str())
        || input["paymentCommitment"].as_str()
            != Some(decimal_bytes(&verified.payment_note).as_str())
    {
        return Err(ProverError::Transfer(
            "verified agreement differs from circuit inputs",
        ));
    }
    let public_inputs = public_names
        .iter()
        .map(|name| {
            input[*name]
                .as_str()
                .expect("fixed circuit field exists")
                .to_owned()
        })
        .collect();
    Ok(TransferWitness {
        input: Value::Object(input),
        public_inputs,
    })
}

fn check_artifact(path: &PathBuf, expected: &str, label: &'static str) -> Result<(), ProverError> {
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ProverError::Artifact(label));
    }
    let mut file = File::open(path).map_err(|_| ProverError::Artifact(label))?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| ProverError::Artifact(label))?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    if format!("{:x}", hash.finalize()) != expected.to_ascii_lowercase() {
        return Err(ProverError::Artifact(label));
    }
    Ok(())
}

fn decimal(value: &Value) -> Result<String, ProverError> {
    let text = value
        .as_str()
        .ok_or(ProverError::Input("expected a decimal string"))?;
    if text.is_empty()
        || (text.len() > 1 && text.starts_with('0'))
        || !text.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(ProverError::Input("non-canonical decimal field"));
    }
    let field = Fr::from_str(text).map_err(|_| ProverError::Input("field outside BN254"))?;
    if field.to_string() != text {
        return Err(ProverError::Input("field outside BN254"));
    }
    Ok(text.to_owned())
}

impl ProvingArtifacts {
    /// Checks all three artifact hashes before loading any proving material.
    pub fn verify(&self) -> Result<(), ProverError> {
        check_artifact(&self.wasm, &self.wasm_sha256, "wasm")?;
        check_artifact(&self.r1cs, &self.r1cs_sha256, "r1cs")?;
        check_artifact(&self.zkey, &self.zkey_sha256, "zkey")?;
        Ok(())
    }

    /// Builds a witness locally and proves it against the pinned snarkjs key.
    ///
    /// `expected_public` is in the circuit's public-signal order. A mismatch fails
    /// before proof generation, so a caller cannot silently settle a different transition.
    pub fn prove(
        &self,
        input: &Value,
        expected_public: &[&str],
    ) -> Result<LocalProof, ProverError> {
        self.verify()?;
        let object = input
            .as_object()
            .ok_or(ProverError::Input("expected object"))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| ProverError::Proof("runtime setup"))?;
        let _runtime_guard = runtime.enter();
        let config = CircomConfig::<Fr>::new(&self.wasm, &self.r1cs)
            .map_err(|_| ProverError::Proof("circuit load"))?;
        let mut builder = CircomBuilder::new(config);
        for (name, value) in object {
            if let Some(values) = value.as_array() {
                for value in values {
                    let text = decimal(value)?;
                    let integer = BigInt::from_str(&text)
                        .map_err(|_| ProverError::Input("invalid integer"))?;
                    builder.push_input(name, integer);
                }
            } else {
                let text = decimal(value)?;
                let integer =
                    BigInt::from_str(&text).map_err(|_| ProverError::Input("invalid integer"))?;
                builder.push_input(name, integer);
            }
        }
        let circuit = builder
            .build()
            .map_err(|_| ProverError::Proof("witness generation"))?;
        let public_inputs = circuit
            .get_public_inputs()
            .ok_or(ProverError::Input("circuit has no public inputs"))?;
        let expected: Vec<Fr> = expected_public
            .iter()
            .map(|value| {
                let text = decimal(&Value::String((*value).to_owned()))?;
                Fr::from_str(&text).map_err(|_| ProverError::Input("public field"))
            })
            .collect::<Result<_, _>>()?;
        if public_inputs != expected {
            return Err(ProverError::PublicInputMismatch);
        }
        let mut file = File::open(&self.zkey).map_err(|_| ProverError::Artifact("zkey"))?;
        let (key, _) = read_zkey(&mut file).map_err(|_| ProverError::Proof("key load"))?;
        let proof = Groth16::<Bn254, CircomReduction>::prove(&key, circuit, &mut OsRng)
            .map_err(|_| ProverError::Proof("proof generation"))?;
        let processed = Groth16::<Bn254>::process_vk(&key.vk)
            .map_err(|_| ProverError::Proof("verification key processing"))?;
        if !Groth16::<Bn254>::verify_with_processed_vk(&processed, &public_inputs, &proof)
            .map_err(|_| ProverError::Proof("proof verification"))?
        {
            return Err(ProverError::Verification);
        }
        Ok(LocalProof {
            proof,
            public_inputs,
        })
    }
}
