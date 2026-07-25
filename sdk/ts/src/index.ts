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
