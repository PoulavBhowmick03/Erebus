//! Funding preconditions for a proof-bearing write.
//!
//! Two different limits are being enforced against two different quantities, and conflating
//! them is the bug these tests exist to prevent: the allowance bounds only what the pool can
//! pull, while the balance has to survive the gas nothing pulls and no allowance governs.

use erebus_sdk::client::{ClientError, PreparedChecks, DEFAULT_GAS_RESERVE};

const FEE: u128 = 2_000_000_000_000_000_000;
const DEPOSIT: u128 = 5_000_000_000_000_000_000;

fn checks(allowance: u128, public_balance: u128) -> PreparedChecks {
    PreparedChecks {
        proof_validity_blocks: 450,
        fee_per_write: FEE,
        allowance,
        public_balance,
    }
}

#[test]
fn a_write_with_exactly_enough_is_allowed() {
    // Exactly the fee approved, and exactly fee plus gas in hand. Nothing to spare, and
    // nothing missing.
    let report = checks(FEE, FEE + DEFAULT_GAS_RESERVE);

    assert!(report.verify(0).is_ok());
}

#[test]
fn one_unit_short_of_the_allowance_is_refused() {
    let report = checks(FEE - 1, u128::MAX);

    assert!(matches!(
        report.verify(0),
        Err(ClientError::InsufficientAllowance { .. })
    ));
}

#[test]
fn a_deposit_is_counted_on_top_of_the_fee() {
    // A shield pulls both. An allowance sized for the fee alone covers a write but not a
    // deposit, which is the case a fee-only check would wave through.
    let report = checks(FEE, u128::MAX);

    assert!(report.verify(0).is_ok());
    assert!(matches!(
        report.verify(DEPOSIT),
        Err(ClientError::InsufficientAllowance { .. })
    ));
    assert!(checks(FEE + DEPOSIT, u128::MAX).verify(DEPOSIT).is_ok());
}

#[test]
fn a_balance_that_covers_the_fee_but_not_the_gas_is_refused() {
    // This is the live failure it exists for: a healthy shielded balance and a generous
    // allowance, and the submission still dies because the account cannot pay gas. See F27.
    let report = checks(u128::MAX, FEE);

    let error = report.verify(0).expect_err("gas is not covered");
    let ClientError::InsufficientPublicBalance {
        required,
        balance,
        gas_reserve,
    } = error
    else {
        panic!("expected an insufficient balance error, got {error}");
    };

    assert_eq!(balance, FEE);
    assert_eq!(gas_reserve, DEFAULT_GAS_RESERVE);
    assert_eq!(required, FEE + DEFAULT_GAS_RESERVE);
}

#[test]
fn the_allowance_is_checked_before_the_balance() {
    // Both are wrong. The allowance is the cheaper thing to fix and the more specific
    // diagnosis, so it is the one reported.
    let report = checks(0, 0);

    assert!(matches!(
        report.verify(0),
        Err(ClientError::InsufficientAllowance { .. })
    ));
}

#[test]
fn an_overflowing_deposit_is_rejected_rather_than_wrapped() {
    let report = checks(u128::MAX, u128::MAX);

    assert!(matches!(
        report.verify(u128::MAX),
        Err(ClientError::InvalidRequest(_))
    ));
}

#[test]
fn the_gas_reserve_does_not_wrap_at_the_top_of_the_range() {
    // `pulled + gas` saturates rather than wrapping. Wrapping would turn an impossible
    // requirement into a small one and let the write through.
    let report = checks(u128::MAX, u128::MAX - 1);

    assert!(matches!(
        report.verify(u128::MAX - DEFAULT_GAS_RESERVE),
        Err(ClientError::InsufficientPublicBalance { .. })
    ));
}
