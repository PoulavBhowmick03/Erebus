/**
 * The seam between the two tracks. Transcribed from ARCHITECTURE.md §4.
 *
 * FROZEN during MVP. Ishita builds agents against a mock of exactly this; Poulav
 * implements behind it. Do not change unilaterally — a change here breaks the other
 * track's mock and destroys the parallelism (CLAUDE.md, "The interface contract is
 * frozen during MVP").
 *
 * Names are plain English. Brand vocabulary (Eleusis, Kleidouchos) lives in docs, not
 * here.
 *
 * KNOWN OPEN ITEM — P0.2. The pool cannot store a structured offer payload: a note is
 * `(packed_value: felt252, token: ContractAddress)` and `ClientAction` has no payload
 * variant (see docs/friction.md F1, verified by the passing probe in
 * contracts/probes/). None of the three candidate workarounds changes any *signature*
 * below, which is why this file can be frozen now. What they do change is
 * `readChannelState`'s backing store and how much of `DisclosedRecord` `reveal` can
 * actually reconstruct. Both are documented per-method.
 */

/** Opaque handle to a channel. Format is an implementation detail; do not parse it. */
export type ChannelHandle = string;

/** Opaque identifier for an offer. Format is an implementation detail. */
export type OfferId = string;

/** Agent identity. Maps to a Starknet account address for the MVP. */
export type AgentId = string;

/** Stark-curve public key, hex-encoded felt252. */
export type PublicKey = string;

/** Viewing key material. Never leaves the SDK boundary (CLAUDE.md constraint 6). */
export type ViewingKey = string;

/** ERC-20 contract address, hex-encoded. */
export type ContractAddress = string;

export interface OfferTerms {
  /** Token base units. u128 on-chain — values above 2^128-1 are invalid. */
  amount: bigint;
  token: ContractAddress;
  /** Unix seconds. */
  deadline: number;
  /** felt252 — hash of off-chain detail, never the detail itself. */
  memoHash: string;
  /** Replay protection. */
  nonce: number;
}

export type OfferStatus =
  | "proposed"
  | "countered"
  | "accepted"
  | "expired"
  | "settled"
  | "withdrawn";

export interface Offer {
  offerId: OfferId;
  channelId: ChannelHandle;
  proposer: AgentId;
  replyTo?: OfferId;
  terms: OfferTerms;
  status: OfferStatus;
  /** Unix seconds. */
  createdAt: number;
}

export interface SettlementReceipt {
  offerId: OfferId;
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
  counterparty: AgentId;
  offers: Offer[];
}

export interface DisclosedRecord {
  channelId: ChannelHandle;
  participants: AgentId[];
  offers: Offer[];
  settlement: SettlementReceipt;
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
  | "OFFER_NOT_FOUND"
  | "OFFER_EXPIRED"
  | "OFFER_WITHDRAWN"
  | "OFFER_ALREADY_SETTLED"
  | "INSUFFICIENT_BALANCE"
  | "PROOF_FAILED"
  | "SCREENING_REJECTED"
  | "SCREENING_UNAVAILABLE";

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

  /**
   * Read all offer state visible to this party.
   *
   * Where this reads from depends on the unresolved P0.2 decision — pool notes, an
   * Erebus-owned contract, or an off-chain store. The return type does not change.
   */
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

  /** Grant a viewing key to a third party. */
  grantViewingKey(handle: ChannelHandle, grantee: PublicKey): Promise<void>;

  /**
   * Reconstruct the scoped record using a viewing key.
   *
   * A viewing key is *pool* key material. If the P0.2 decision puts offers off-chain,
   * a viewing key alone cannot reconstruct `offers` — it needs the payload from a
   * participant, verified against the on-chain commitment. Signature unchanged; the
   * guarantee is weaker. See docs/friction.md F1.
   */
  reveal(handle: ChannelHandle, viewingKey: ViewingKey): Promise<DisclosedRecord>;
}
