//! ERC-20 calldata for the one token operation Erebus performs outside the pool.
//!
//! The pool pulls value from the submitting account with `transfer_from`, twice: once for a
//! deposit (`privacy.cairo:954`) and once for its own protocol fee, which `apply_actions`
//! collects before it applies anything (`privacy.cairo:790`, `:841`). `transfer_from` moves
//! the *owner's* tokens on the *spender's* instruction, so the account has to have called
//! `approve` naming the pool first. Without that allowance every charged `apply_actions`
//! reverts, and it reverts as a bare `Contract error` naming nothing. See friction.md F20.
//!
//! The fee is a pool storage value, not a constant: `get_fee_amount` reads 6 STRK on mainnet
//! and 2 STRK on Sepolia, and `set_fee_amount` can change either. Size approvals from a live
//! read rather than from a number written down here.
//!
//! Nothing in this module builds an [`crate::action_set::ActionSet`], reaches the prover, or
//! touches the pool. It is a plain token call that happens to be a precondition for the
//! proved path.

use starknet_types_core::felt::Felt;

/// `approve(spender, amount)` calldata.
///
/// Cairo serializes `u256` as two `u128` limbs, low first. Amounts here are `u128`, matching
/// every other amount in this crate, so the high limb is always zero. That bounds what this
/// function can ever authorize at roughly `3.4e20` STRK, which is deliberate: an unbounded
/// approval is not expressible through the SDK.
pub fn approve_calldata(spender: Felt, amount: u128) -> Vec<Felt> {
    vec![spender, Felt::from(amount), Felt::ZERO]
}

/// `allowance(owner, spender)` calldata.
pub fn allowance_calldata(owner: Felt, spender: Felt) -> Vec<Felt> {
    vec![owner, spender]
}

/// Reads a Cairo `u256` return value as a `u128`.
///
/// Errors rather than saturating when the high limb is set. An allowance that large did not
/// come from [`approve_calldata`], so reporting the limbs is more useful than reporting a
/// clamped number that hides where it came from.
pub fn parse_u256(field: &'static str, values: &[Felt]) -> Result<u128, Erc20Error> {
    let [low, high] = values else {
        return Err(Erc20Error::UnexpectedWidth {
            field,
            felts: values.len(),
        });
    };
    if *high != Felt::ZERO {
        return Err(Erc20Error::ExceedsU128 {
            field,
            low: format!("{low:#x}"),
            high: format!("{high:#x}"),
        });
    }
    u128::try_from(*low).map_err(|_| Erc20Error::ExceedsU128 {
        field,
        low: format!("{low:#x}"),
        high: format!("{high:#x}"),
    })
}

/// A malformed ERC-20 return value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Erc20Error {
    /// A `u256` return was not two felts.
    #[error("{field} should be a u256 of two felts, got {felts}")]
    UnexpectedWidth {
        /// Which value was malformed.
        field: &'static str,
        /// How many felts arrived.
        felts: usize,
    },
    /// A `u256` return did not fit a `u128`.
    #[error("{field} does not fit u128: low {low}, high {high}")]
    ExceedsU128 {
        /// Which value was too large.
        field: &'static str,
        /// Low limb.
        low: String,
        /// High limb.
        high: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calldata::selector;

    fn felt(value: &str) -> Felt {
        Felt::from_hex(value).expect("felt")
    }

    /// Pinned against `starknet_py.hash.selector.get_selector_from_name`, an implementation
    /// that shares no code with this crate. A wrong selector calls nothing and the
    /// transaction reverts without naming the entrypoint it failed to find.
    #[test]
    fn erc20_selectors_match_starknet_py() {
        assert_eq!(
            selector("approve"),
            felt("0x219209e083275171774dab1df80982e9df2096516f06319c5c6d71ae0a8480c")
        );
        assert_eq!(
            selector("allowance"),
            felt("0x1e888a1026b19c8c0b57c72d63ed1737106aa10034105b980ba117bd0c29fe1")
        );
    }

    #[test]
    fn approve_serializes_the_amount_low_limb_first() {
        let spender = felt("0x254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91");
        // 2 STRK, the Sepolia fee at the time of writing.
        assert_eq!(
            approve_calldata(spender, 2_000_000_000_000_000_000),
            vec![spender, felt("0x1bc16d674ec80000"), Felt::ZERO]
        );
        assert_eq!(
            approve_calldata(spender, u128::MAX),
            vec![spender, felt("0xffffffffffffffffffffffffffffffff"), Felt::ZERO]
        );
    }

    #[test]
    fn u256_returns_are_read_low_limb_first() {
        // The live allowance held by the Sepolia settling account on 2026-08-16.
        assert_eq!(
            parse_u256("allowance", &[felt("0x5188315f776b80000"), Felt::ZERO]).expect("fits"),
            94_000_000_000_000_000_000
        );
        assert_eq!(
            parse_u256("allowance", &[Felt::ZERO, Felt::ZERO]).expect("fits"),
            0
        );
    }

    #[test]
    fn an_allowance_wider_than_u128_is_reported_rather_than_clamped() {
        let error = parse_u256("allowance", &[Felt::ZERO, Felt::ONE]).expect_err("high limb set");
        assert!(matches!(error, Erc20Error::ExceedsU128 { .. }));
        let error = parse_u256("allowance", &[Felt::ZERO]).expect_err("one felt");
        assert!(matches!(error, Erc20Error::UnexpectedWidth { felts: 1, .. }));
    }
}
