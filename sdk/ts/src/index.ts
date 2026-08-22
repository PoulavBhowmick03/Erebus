export type {
  AgentId,
  ChannelHandle,
  ChannelState,
  ContractAddress,
  DisclosedRecord,
  ErebusClient,
  Offer,
  OfferId,
  OfferStatus,
  OfferTerms,
  PublicKey,
  SettlementErrorCode,
  SettlementReceipt,
  ViewingKey,
  ViewingKeyGrant,
} from "./interface.js";
export { SettlementError } from "./interface.js";

export type { Felt } from "./crypto/channel-secret.js";
export {
  deriveChannelTransportKey,
  deriveSharedSecret,
  deriveTransportKey,
  deriveViewingPublicKey,
} from "./crypto/channel-secret.js";

export type { MessageType, WireMessage } from "./channel/wire.js";
export {
  decodeMessage,
  encodeMessage,
  noteIndexForMessage,
  NOTES_PER_MESSAGE,
  PAYLOAD_BITS_PER_NOTE,
  truncateMemoHash,
} from "./channel/wire.js";
export type { WireContextV3, WireMessageV3 } from "./channel/wire-v3.js";
export {
  decodeMessageV3,
  decodeMessageV3WithDealKey,
  deriveDealIdV3,
  deriveDealKeyV3,
  encodeMessageV3,
  WIRE_V3_CAPACITY_BITS,
  WIRE_V3_NOTES_PER_MESSAGE,
  WIRE_V3_PAYLOAD_BITS_PER_NOTE,
} from "./channel/wire-v3.js";
