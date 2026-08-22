# Wire v3

This document specifies the final Erebus negotiation wire.

Wire v3 supports more than one deal in a directional channel pair. It also removes the fixed salt shape from wire v2.

Wire v1 and wire v2 remain readable. A channel record selects one wire version. A decoder must not try another version after an error.

## Terms

A **data note** is a zero-value encrypted note that carries one wire salt.

A **frame** is one message and its related value notes. A frame starts at a physical note index.

A **deal ID** identifies one negotiation in a channel pair. Each message in that negotiation carries the same deal ID.

## Message fields

The plaintext contains 464 bits in this order:

| Field | Width | Rule |
|---|---:|---|
| `deal_id` | 64 bits | An opening offer creates it. Replies copy it. |
| `message_type` | 8 bits | `1` is offer, `2` is counter, and `3` is accept. |
| `reply_to` | 32 bits | Physical frame start in the opposite direction. `2^32 - 1` means no reply. |
| `created_at` | 40 bits | Unix time in seconds. |
| `amount` | 128 bits | Token base units. |
| `deadline` | 64 bits | Unix time in seconds. |
| `memo_hash` | 128 bits | Low 128 bits of the source digest. |

The message uses 58 bytes. The encoder uses most-significant-field order.

Rust, Python, and MCP expose `deal_id` as a decimal string in JSON. A 64-bit value can
exceed JavaScript's exact integer range. Internal Rust and Python APIs may parse it as an
unsigned integer.

## AEAD envelope

The semantic record is 464 bits. The encoder places the 64-bit deal ID in an obfuscated
header. It encrypts the remaining 400-bit body with AES-256-GCM-SIV under a native per-deal
key. The envelope is `header:8 || ciphertext:50 || tag:16`, or 592 bits. Five data notes
supply 595 payload bits. The encoder fills the remaining three bits with a derived mask.

Wire v3 has no marker byte. The stored channel version selects the decoder. This rule makes 64-bit deal IDs fit without a sixth data note.

Each salt reserves bit 119. The encoder sets this bit to `1`. Payload bits use positions `0..118`.

## Key derivation

The parent input key material is the 32-byte big-endian directional channel key.

The scope is this byte sequence:

```text
chain_id:32 || pool_address:32 || token:32
```

The encoder first derives one native key for the deal and direction:

```text
salt = "EREBUS_WIRE_V3_DEAL_KEY_HKDF_SHA256"
info = "EREBUS_WIRE_V3_DEAL_KEY" || scope || deal_id:u64be
length = 32
```

This deal key is the input key material for the nonce and spare-bit derivations. A scoped
grant can disclose it without disclosing the parent channel key.

The encoder derives the nonce with these values:

```text
salt = "EREBUS_WIRE_V3_HKDF_SHA256"
info = "EREBUS_WIRE_V3_NONCE" || scope || frame_start:u32be
length = 12
```

The encoder obfuscates the header by XORing the deal ID with this eight-byte stream:

```text
salt = "EREBUS_WIRE_V3_HEADER_HKDF_SHA256"
info = "EREBUS_WIRE_V3_DEAL_HEADER" || scope || frame_start:u32be
length = 8
```

The authenticated data is this byte sequence:

```text
"EREBUS_WIRE_V3_AAD" || scope || frame_start:u32be || deal_id:u64be || header:8
```

The encoder derives the three spare bits with these values:

```text
salt = "EREBUS_WIRE_V3_MASK_HKDF_SHA256"
info = "EREBUS_WIRE_V3_MASK" || scope || frame_start:u32be || header:8
length = 1
```

The encoder uses bits `0..2` of the result. The decoder checks these bits.

## Deal ID derivation

An opening offer derives its deal ID from its outgoing direction and physical frame start.

```text
salt = "EREBUS_WIRE_V3_DEAL_HKDF_SHA256"
info = "EREBUS_WIRE_V3_DEAL_ID" || scope || frame_start:u32be
length = 8
```

The encoder reads the result as an unsigned 64-bit big-endian integer. A counter or acceptance copies the deal ID from its target.

The negotiation state rejects a reply with a different deal ID from its target.

## Frames

An offer frame contains five data notes. A counter frame also contains five data notes.

An acceptance frame contains five data notes and one payment note. The payment note follows the data notes in the same direction.

The frame starts are physical note indices. Therefore, a reply points to a frame start and not to a frame sequence number.

The reader starts at note index `0`. It reads five data notes and decodes the message.

If the message is an acceptance, the reader checks the payment at `frame_start + 5`. Then the next frame starts at `frame_start + 6`.

For an offer or counter, the next frame starts at `frame_start + 5`.

The first missing note ends the transcript. A partial data frame is an error. A missing acceptance payment is also an error.

## Repeat deals

Settlement closes one deal ID. It does not close the channel.

After settlement, either side can write a new opening offer. This offer gets a new deal ID.

An accepted offer and its acceptance get the `settled` status. Other deal IDs keep their own status.

## Settlement shape

Each settlement creates seven encrypted notes:

1. Five acceptance data notes.
2. One payment note for the counterparty.
3. One payer-owned change note.

The change note has value zero for an exact payment. Its salt remains random.

This constant count hides the exact-payment versus change-payment bit. Channel-setup actions can still change the full transaction shape.

## Migration rules

New source-built channels use wire v3 by default. Operators can select wire v2 for an old counterparty.

Existing channel records keep their wire version. The client does not retag a channel.

Both directions of one channel pair must use the same wire version. Both parties must upgrade before they use wire v3.

Wire v1 is read-only. Wire v2 remains readable and writable for explicit compatibility use.

The legacy viewing grant contains both directional parent channel keys. Wire v3 rejects that
shape because it would reveal every deal in the pair. Its replacement contains each
direction's native deal key plus the exact opaque note IDs and amount masks needed for the
selected frames. STRK20 note locations and amount masks still derive from the parent channel
key, so the grantor computes these exact capabilities without exporting that key. The grant
is encrypted to the recipient's registered pool key. Its authenticated metadata binds the
chain, pool, token, parties, deal ID, recipient, expiry, and one-time ECDH public key.

## Normative evidence

The repository must contain these checks:

- Rust and TypeScript produce identical salts for each published vector.
- Wire v2 and wire v3 reject each other.
- A changed context fails authentication.
- A changed spare bit fails the frame check.
- Two deals settle through the same directional channel pair.
- A reply cannot cross a deal ID.
- Historical wire-v1 and wire-v2 records remain readable.
- A native deal key cannot open another deal and does not require the parent channel key.
- A v3 grant rejects the wrong recipient, expiry, and modified ciphertext.
- The historical fifth-salt classifier scores `0.5000` on wire-v3 codec output.

The Sepolia run must record both channel transactions, the offer transaction, and the settlement transaction. It must also record the final receipt state.
