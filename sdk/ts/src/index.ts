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
