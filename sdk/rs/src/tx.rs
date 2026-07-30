//! `INVOKE_TXN_V3` construction and transaction hashing.
//!
//! This is a Starknet protocol hash, not a privacy-pool hash. The Cairo contract does not
//! emit a vector for it. The oracle is starknet.js, which the upstream SDK also uses. The
//! known-answer tests use
//! `tests/fixtures/starknetjs-invoke-v3-txhash.json`.
//!
//! ## Why this matters here
//!
//! The Erebus flow hashes two transaction types here:
//!
//! 1. The proof invocation sent to `starknet_proveTransaction`. Its signature is what
//!    `assert_valid_signature` checks inside the prover's virtual `__execute__`
//!    (`privacy.cairo:207`). An incorrect hash makes virtual execution reject the request.
//! 2. The `apply_actions` submission to the chain, which carries `proof_facts`.
//!
//! `proof_facts` is a privacy-specific extension to the v3 hash preimage. A non-empty value
//! adds one `poseidon_hash_many` term. A generic v3 hash omits it and produces an invalid
//! signature for a proof-carrying transaction.

use serde::Serialize;
use starknet_crypto::{poseidon_hash_many, Signature};
use starknet_types_core::felt::Felt;

/// Renders a felt the way the RPC wire format does: `0x`-prefixed, minimal-length hex.
fn hex(value: &Felt) -> String {
    format!("{value:#x}")
}

/// `encodeShortString("invoke")`.
const INVOKE_PREFIX: Felt = Felt::from_hex_unchecked("0x696e766f6b65");
/// `encodeShortString("L1_GAS")`.
const L1_GAS_NAME: Felt = Felt::from_hex_unchecked("0x4c315f474153");
/// `encodeShortString("L2_GAS")`.
const L2_GAS_NAME: Felt = Felt::from_hex_unchecked("0x4c325f474153");
/// `encodeShortString("L1_DATA")`.
const L1_DATA_GAS_NAME: Felt = Felt::from_hex_unchecked("0x4c315f44415441");

/// `2^128`, the shift for `max_amount` in a packed resource bound.
const TWO_POW_128: Felt = Felt::from_hex_unchecked("0x100000000000000000000000000000000");
/// `2^192`, the shift for a resource name.
const TWO_POW_192: Felt =
    Felt::from_hex_unchecked("0x1000000000000000000000000000000000000000000000000");
/// `2^32`, the shift for the nonce DA mode.
const TWO_POW_32: Felt = Felt::from_hex_unchecked("0x100000000");

/// Transaction version 3.
pub const VERSION_3: Felt = Felt::THREE;
/// Query transaction version `2^128 + 3`, used only for fee estimation.
pub const QUERY_VERSION_3: Felt = Felt::from_hex_unchecked("0x100000000000000000000000000000003");

/// Where a transaction's data is made available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DataAvailabilityMode {
    /// Ethereum calldata.
    #[default]
    L1,
    /// A data-availability committee.
    L2,
}

impl DataAvailabilityMode {
    fn as_felt(self) -> Felt {
        match self {
            Self::L1 => Felt::ZERO,
            Self::L2 => Felt::ONE,
        }
    }
}

/// A single resource's limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResourceBound {
    /// Maximum units of the resource.
    pub max_amount: u64,
    /// Maximum price per unit, in FRI.
    pub max_price_per_unit: u128,
}

impl ResourceBound {
    /// Packs as `(name << 192) + (max_amount << 128) + max_price_per_unit`.
    fn encode(self, name: Felt) -> Felt {
        name * TWO_POW_192
            + Felt::from(self.max_amount) * TWO_POW_128
            + Felt::from(self.max_price_per_unit)
    }
}

/// The three resource bounds of a v3 transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResourceBounds {
    /// L1 gas.
    pub l1_gas: ResourceBound,
    /// L2 gas.
    pub l2_gas: ResourceBound,
    /// L1 data gas.
    pub l1_data_gas: ResourceBound,
}

impl ResourceBounds {
    /// The bounds a proof invocation uses: zero prices, so the effective fee is zero.
    ///
    /// `__validate__` requires a zero tip and zero `max_price_per_unit` for each resource
    /// (`privacy.cairo:183-189`). A non-zero price returns `NON_ZERO_RESOURCE_PRICE`.
    pub fn for_proof_invocation() -> Self {
        Self {
            l1_gas: ResourceBound {
                max_amount: 1,
                max_price_per_unit: 0,
            },
            l2_gas: ResourceBound {
                max_amount: 100_000_000,
                max_price_per_unit: 0,
            },
            l1_data_gas: ResourceBound {
                max_amount: 1,
                max_price_per_unit: 0,
            },
        }
    }
}

/// An `INVOKE_TXN_V3` transaction, as hashed for signing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvokeV3 {
    /// Sending account. A proof invocation uses the pool account contract.
    pub sender_address: Felt,
    /// Serialized `Array<Call>` calldata for `__execute__`.
    pub calldata: Vec<Felt>,
    /// Chain id as a short-string felt.
    pub chain_id: Felt,
    /// Account nonce.
    pub nonce: Felt,
    /// Account deployment data. Empty for an already-deployed account.
    pub account_deployment_data: Vec<Felt>,
    /// DA mode for the nonce.
    pub nonce_da_mode: DataAvailabilityMode,
    /// DA mode for the fee.
    pub fee_da_mode: DataAvailabilityMode,
    /// Resource bounds.
    pub resource_bounds: ResourceBounds,
    /// Tip. Must be zero for a pool invocation (`NON_ZERO_TIP`).
    pub tip: u64,
    /// Paymaster data. Empty unless a paymaster is wired.
    pub paymaster_data: Vec<Felt>,
    /// Proof facts. Empty for the proof invocation itself; populated for the
    /// `apply_actions` submission, where it becomes an extra hash term.
    pub proof_facts: Vec<Felt>,
}

impl InvokeV3 {
    /// `poseidon(tip, l1_bound, l2_bound, l1_data_bound)`.
    fn fee_field_hash(&self) -> Felt {
        poseidon_hash_many(&[
            Felt::from(self.tip),
            self.resource_bounds.l1_gas.encode(L1_GAS_NAME),
            self.resource_bounds.l2_gas.encode(L2_GAS_NAME),
            self.resource_bounds.l1_data_gas.encode(L1_DATA_GAS_NAME),
        ])
    }

    /// `(nonce_da_mode << 32) + fee_da_mode`.
    fn da_mode_hash(&self) -> Felt {
        self.nonce_da_mode.as_felt() * TWO_POW_32 + self.fee_da_mode.as_felt()
    }

    /// The transaction hash that gets signed.
    pub fn transaction_hash(&self) -> Felt {
        let mut preimage = vec![
            INVOKE_PREFIX,
            VERSION_3,
            self.sender_address,
            self.fee_field_hash(),
            poseidon_hash_many(&self.paymaster_data),
            self.chain_id,
            self.nonce,
            self.da_mode_hash(),
            poseidon_hash_many(&self.account_deployment_data),
            poseidon_hash_many(&self.calldata),
        ];
        // An empty vector must not add `poseidon_hash_many(&[])`. That term changes every
        // ordinary transaction hash.
        if !self.proof_facts.is_empty() {
            preimage.push(poseidon_hash_many(&self.proof_facts));
        }
        poseidon_hash_many(&preimage)
    }

    /// Pairs this transaction with a signature, ready to send to the proving service.
    pub fn with_signature(self, signature: Signature) -> SignedInvokeV3 {
        SignedInvokeV3 {
            invoke: self,
            signature,
            proof: None,
        }
    }
}

/// Ways a transaction can violate the pool's `__validate__` preconditions.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PoolInvocationError {
    /// `__validate__` asserts `NON_ZERO_TIP`.
    #[error("a pool invocation must have a zero tip, got {0}")]
    NonZeroTip(u64),
    /// `__validate__` asserts `NON_ZERO_RESOURCE_PRICE` on every resource.
    #[error("a pool invocation must have zero {resource} price, got {price}")]
    NonZeroResourcePrice {
        /// Which resource.
        resource: &'static str,
        /// The offending price.
        price: u128,
    },
}

/// An [`InvokeV3`] that satisfies the pool's `__validate__` preconditions.
///
/// The pool's `__validate__` rejects a non-zero tip or resource price
/// (`privacy.cairo:177-191`). Construction checks both conditions.
#[derive(Debug)]
pub struct PoolInvocation(InvokeV3);

impl PoolInvocation {
    /// Validates and wraps.
    pub fn new(invoke: InvokeV3) -> Result<Self, PoolInvocationError> {
        if invoke.tip != 0 {
            return Err(PoolInvocationError::NonZeroTip(invoke.tip));
        }
        for (resource, bound) in [
            ("l1_gas", invoke.resource_bounds.l1_gas),
            ("l2_gas", invoke.resource_bounds.l2_gas),
            ("l1_data_gas", invoke.resource_bounds.l1_data_gas),
        ] {
            if bound.max_price_per_unit != 0 {
                return Err(PoolInvocationError::NonZeroResourcePrice {
                    resource,
                    price: bound.max_price_per_unit,
                });
            }
        }
        Ok(Self(invoke))
    }

    /// The wrapped transaction.
    pub fn inner(&self) -> &InvokeV3 {
        &self.0
    }

    /// Consumes the wrapper.
    pub fn into_inner(self) -> InvokeV3 {
        self.0
    }
}

/// An `INVOKE_TXN_V3` plus its signature, used as the `transaction` parameter of
/// `starknet_proveTransaction`.
///
/// `starknet_crypto::Signature` does not implement `Clone` or `Eq`, so this type only derives
/// `Debug`.
#[derive(Debug)]
pub struct SignedInvokeV3 {
    /// The transaction.
    pub invoke: InvokeV3,
    /// Signature over [`InvokeV3::transaction_hash`].
    pub signature: Signature,
    /// Opaque proof blob on the final `apply_actions` transaction.
    ///
    /// `None` for the proof invocation because that request produces the proof.
    pub proof: Option<String>,
}

/// A resource bound on the wire.
#[derive(Debug, Serialize)]
struct WireBound {
    max_amount: String,
    max_price_per_unit: String,
}

/// Resource bounds on the wire.
#[derive(Debug, Serialize)]
struct WireBounds {
    l1_gas: WireBound,
    l2_gas: WireBound,
    l1_data_gas: WireBound,
}

/// `INVOKE_TXN_V3` in RPC wire form.
///
/// Field order matches the RPC specification and upstream serialization. This makes captured
/// SDK payloads easy to compare even though JSON object order has no meaning.
#[derive(Debug, Serialize)]
pub struct WireInvokeV3 {
    #[serde(rename = "type")]
    tx_type: &'static str,
    sender_address: String,
    calldata: Vec<String>,
    signature: Vec<String>,
    nonce: String,
    resource_bounds: WireBounds,
    tip: String,
    paymaster_data: Vec<String>,
    account_deployment_data: Vec<String>,
    nonce_data_availability_mode: &'static str,
    fee_data_availability_mode: &'static str,
    version: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    proof_facts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof: Option<String>,
}

impl DataAvailabilityMode {
    fn as_wire(self) -> &'static str {
        match self {
            Self::L1 => "L1",
            Self::L2 => "L2",
        }
    }
}

impl SignedInvokeV3 {
    /// Attaches the proof returned by `starknet_proveTransaction`.
    pub fn with_proof(mut self, proof: String) -> Self {
        self.proof = Some(proof);
        self
    }

    /// Converts to the RPC wire representation.
    ///
    pub fn to_wire(&self) -> WireInvokeV3 {
        self.to_wire_with_version(VERSION_3)
    }

    /// Converts to wire form with an explicit version.
    ///
    /// Fee estimation uses query version `2^128 + 3`. The final transaction uses version 3.
    /// This override changes only the JSON request and never the signature hash.
    pub fn to_wire_with_version(&self, version: Felt) -> WireInvokeV3 {
        let bound = |b: &ResourceBound| WireBound {
            max_amount: format!("{:#x}", b.max_amount),
            max_price_per_unit: format!("{:#x}", b.max_price_per_unit),
        };
        WireInvokeV3 {
            tx_type: "INVOKE",
            sender_address: hex(&self.invoke.sender_address),
            calldata: self.invoke.calldata.iter().map(hex).collect(),
            signature: vec![hex(&self.signature.r), hex(&self.signature.s)],
            nonce: hex(&self.invoke.nonce),
            resource_bounds: WireBounds {
                l1_gas: bound(&self.invoke.resource_bounds.l1_gas),
                l2_gas: bound(&self.invoke.resource_bounds.l2_gas),
                l1_data_gas: bound(&self.invoke.resource_bounds.l1_data_gas),
            },
            tip: format!("{:#x}", self.invoke.tip),
            paymaster_data: self.invoke.paymaster_data.iter().map(hex).collect(),
            account_deployment_data: self
                .invoke
                .account_deployment_data
                .iter()
                .map(hex)
                .collect(),
            nonce_data_availability_mode: self.invoke.nonce_da_mode.as_wire(),
            fee_data_availability_mode: self.invoke.fee_da_mode.as_wire(),
            version: hex(&version),
            proof_facts: self.invoke.proof_facts.iter().map(hex).collect(),
            proof: self.proof.clone(),
        }
    }
}
