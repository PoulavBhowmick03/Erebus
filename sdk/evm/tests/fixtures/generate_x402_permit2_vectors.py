"""Known-answer vectors for x402 `exact` over Permit2, produced by the reference x402 SDK.

Regenerate from a checkout of github.com/coinbase/x402 (vectors pinned at commit dd927a2,
python package x402 2.8.0):

    uv venv /tmp/x402venv && uv pip install --python /tmp/x402venv/bin/python -e '<x402>/python/x402[evm]'
    /tmp/x402venv/bin/python sdk/evm/tests/fixtures/generate_x402_permit2_vectors.py \
        > sdk/evm/tests/fixtures/x402-permit2-vectors.json

Typed data comes from the SDK's own `_build_permit2_typed_data` and is signed by its
`EthAccountSigner`; calldata is `eth_abi` over the SDK's proxy ABI. Keys are test-only.
"""

import json

from eth_abi import encode
from eth_account import Account
from eth_account.messages import encode_typed_data
from eth_utils import keccak
from x402.mechanisms.evm.constants import X402_EXACT_PERMIT2_PROXY_ABI, X402_EXACT_PERMIT2_PROXY_ADDRESS
from x402.mechanisms.evm.exact.permit2_utils import _build_permit2_typed_data
from x402.mechanisms.evm.signers import EthAccountSigner
from x402.mechanisms.evm.types import ExactPermit2Authorization, ExactPermit2TokenPermissions, ExactPermit2Witness

CASES = [
    # (name, chain id, private key byte, token, amount, nonce hex, deadline, to, valid_after)
    ("anvil-small", 31337, 0x15, "0x5fbdb2315678afecb367f032d93f642f64180aa3", 70,
     "0009882dde8af6ae5817b341acb523cafd35ba296d7132695c7f6c3605e0b05f", 1_800_000_000,
     "0x2b5ad5c4795c026514f8317c7a215e218dccd6cf", 1_759_000_000),
    ("chain-10143-arbitrary-token", 10143, 0x21, "0x1111111111111111111111111111111111111111", 1_250_000,
     "90f55cc8de8092e601a250c40d3b941b2b5922c9e95547adc81d4a3d678ef785", 1_759_503_600,
     "0x6813eb9362372eef6200f3b1dbc3f819671cba69", 1_759_500_000),
    ("high-bit-nonce-max-amount", 143, 0x7f, "0x2222222222222222222222222222222222222222", 2**128 - 1,
     "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff01", 2**64 - 1,
     "0x1efecb61a2f80aa34d3b9218b564a64d05946290", 0),
]

SETTLE = next(item for item in X402_EXACT_PERMIT2_PROXY_ABI if item.get("name") == "settle")


def canonical_type(component):
    if component["type"].startswith("tuple"):
        return "(" + ",".join(canonical_type(c) for c in component["components"]) + ")" + component["type"][5:]
    return component["type"]


def main():
    vectors = []
    signature_text = "settle(" + ",".join(canonical_type(i) for i in SETTLE["inputs"]) + ")"
    for name, chain_id, key_byte, token, amount, nonce_hex, deadline, to, valid_after in CASES:
        key = bytes([key_byte]) * 32
        account = Account.from_key(key)
        authorization = ExactPermit2Authorization(
            from_address=account.address,
            permitted=ExactPermit2TokenPermissions(token=token, amount=str(amount)),
            spender=X402_EXACT_PERMIT2_PROXY_ADDRESS,
            nonce=str(int(nonce_hex, 16)),
            deadline=str(deadline),
            witness=ExactPermit2Witness(to=to, valid_after=str(valid_after)),
        )
        domain, fields, primary, message = _build_permit2_typed_data(authorization, chain_id)
        types = {name: [{"name": f.name, "type": f.type} for f in group] for name, group in fields.items()}
        signable = encode_typed_data(domain_data=domain, message_types=types, message_data=message)
        digest = keccak(b"\x19\x01" + signable.header + signable.body)
        signature = EthAccountSigner(account).sign_typed_data(domain, fields, primary, message)
        assert Account._recover_hash(digest, signature=signature) == account.address
        calldata = keccak(text=signature_text)[:4] + encode(
            [canonical_type(i) for i in SETTLE["inputs"]],
            [((token, amount), int(nonce_hex, 16), deadline), account.address, (to, valid_after), signature],
        )
        vectors.append({
            "name": name, "chain_id": chain_id, "private_key": key.hex(), "owner": account.address.lower(),
            "token": token, "amount": str(amount), "spender": X402_EXACT_PERMIT2_PROXY_ADDRESS.lower(),
            "nonce": nonce_hex, "deadline": str(deadline), "to": to, "valid_after": str(valid_after),
            "expected": {"domain_separator": signable.header.hex(), "struct_hash": signable.body.hex(),
                         "digest": digest.hex(), "signature": signature.hex(), "settle_calldata": calldata.hex()},
        })
    print(json.dumps({"source": "coinbase/x402 dd927a2, python x402 2.8.0", "settle_signature": signature_text,
                      "vectors": vectors}, indent=2))


if __name__ == "__main__":
    main()
