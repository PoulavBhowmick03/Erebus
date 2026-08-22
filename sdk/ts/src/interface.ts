/**
 * The seam between the two tracks. Transcribed from ARCHITECTURE.md §4.
 *
 * Names are plain English. Brand vocabulary lives in docs, not the API.
 */

/** Opaque handle to a channel. Format is an implementation detail; do not parse it. */
export type ChannelHandle = string;

/** Opaque identifier for an offer. Format is an implementation detail. */
export type OfferId = string;

/** Agent identity. Maps to a Starknet account address for the MVP. */
export type AgentId = string;

/** Stark-curve public key, hex-encoded felt252. Retained for low-level compatibility. */
export type PublicKey = string;

/** Rust-owned disclosure payload. TypeScript transports it without interpretation. */
export type ViewingKey = unknown;

/** ERC-20 contract address, hex-encoded. */
export type ContractAddress = string;

export interface OfferTerms {
  /** Token base units. u128 on-chain — values above 2^128-1 are invalid. */
  amount: bigint;
  token: ContractAddress;
  /** Unix seconds. */
  deadline: number;
  /** Hex form of the low 128 bits of a hash of off-chain detail. */
  memoHash: string;
}

export type OfferStatus =
  | "proposed"
  | "countered"
  | "expired"
  | "settled";

export interface Offer {
  offerId: OfferId;
  /** Wire-v3 deal identifier. Historical wire-v1 and wire-v2 messages use zero. */
  dealId: bigint;
  channelId: ChannelHandle;
  proposer: AgentId;
  replyTo?: OfferId;
  terms: OfferTerms;
  status: OfferStatus;
  /** Unix seconds. */
  createdAt: number;
}

export interface SettlementReceipt {
  offerId?: OfferId;
  txHash: string;
  nullifiers: string[];
  /** Unix seconds. */
  provedAt: number;
  /**
   * Total value of the notes this settlement spent, in token base units.
   *
   * A decimal string rather than a number: one STRK is 1e18 and JavaScript's safe integer
   * range stops at 2^53, so a number would round without erroring. Absent for the
   * administrative shield helper, which selects nothing.
   */
  selectedInput?: string;
  /**
   * Value returned to the payer as a change note, `"0"` when the selected notes summed
   * exactly to the price. Absent for shielding. Absent and `"0"` are different facts.
   */
  change?: string;
}

export interface ChannelState {
  channelId: ChannelHandle;
  participants: AgentId[];
  offers: Offer[];
  settled: boolean;
}

/** Rust-owned encrypted capability capsule, carried without interpretation. */
export interface ViewingKeyGrant {
  channelId: ChannelHandle;
  grantee: AgentId;
  /** Decimal u64. Present on recipient-bound wire-v3 grants. */
  dealId?: string;
  /** Unix verification deadline. Present on recipient-bound wire-v3 grants. */
  expiresAt?: number;
  viewingKey: ViewingKey;
}

export interface DisclosedRecord {
  channelId: ChannelHandle;
  participants: AgentId[];
  offers: Offer[];
  settlement?: {
    acceptance: OfferId;
    acceptedOffer?: OfferId;
    agreedAmount: bigint;
    paidAmount?: bigint;
  };
}

/**
 * Error shape for a failed settlement (P0.3 open item — agree the exact `code` set
 * with Ishita before freezing).
 *
 * `SCREENING_REJECTED` and `SCREENING_UNAVAILABLE` are not ours: the deployed Sepolia
 * pool has a non-zero screener key, so any action set containing a deposit needs a
 * screener-signed attestation fresh within 300s or the transaction reverts
 * (docs/friction.md F6).
 */
export type SettlementErrorCode =
  | "OFFER_EXPIRED"
  | "OFFER_UNKNOWN"
  | "ALREADY_SETTLED"
  | "NOT_YOUR_OFFER"
  | "AMOUNT_MISMATCH"
  | "INSUFFICIENT_NOTES"
  | "INDEX_CONFLICT"
  | "PROVER_UNAVAILABLE"
  | "PROOF_EXPIRED"
  | "SUBMIT_FAILED"
  | "PROOF_FAILED"
  | "SCREENING_REJECTED"
  | "SCREENING_UNAVAILABLE"
  | "INVALID_REQUEST"
  | "IDENTITY_UNAVAILABLE";

export class SettlementError extends Error {
  constructor(
    readonly code: SettlementErrorCode,
    message: string,
    options?: { cause?: unknown }
  ) {
    super(message, options);
    this.name = "SettlementError";
  }
}

export interface ErebusClient {
  /**
   * Establish a private channel with a counterparty.
   *
   * Two pool actions underneath: `OpenChannel` then `OpenSubchannel(token)` — a note
   * cannot be created or spent without a subchannel, and subchannels are per
   * (channel, token). Channels are directional, so a channel that lets B pay A back
   * is a separate one.
   */
  openChannel(counterparty: AgentId): Promise<ChannelHandle>;

  /** Write a structured offer into the channel. */
  proposeOffer(handle: ChannelHandle, terms: OfferTerms): Promise<OfferId>;

  /** Write a counter-offer referencing a prior offer. */
  counterOffer(
    handle: ChannelHandle,
    replyTo: OfferId,
    terms: OfferTerms
  ): Promise<OfferId>;

  /** Read all authenticated offer state visible to this party from pool notes. */
  readChannelState(handle: ChannelHandle): Promise<ChannelState>;

  /**
   * Accept an offer AND settle atomically. One proven state transition — if the proof
   * fails, the acceptance never happened.
   *
   * Rejects with {@link SettlementError}.
   */
  acceptAndSettle(
    handle: ChannelHandle,
    offerId: OfferId
  ): Promise<SettlementReceipt>;

  /** Encrypt one deal capability to the recipient's registered pool key. */
  grantViewingKey(
    handle: ChannelHandle,
    dealId: string,
    grantee: AgentId,
    expiresAt: number
  ): Promise<ViewingKeyGrant>;

  /**
   * Reconstruct the selected deal. The configured pool identity must match the recipient.
   */
  reveal(viewingKey: ViewingKeyGrant): Promise<DisclosedRecord>;
}
