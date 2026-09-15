//! Share accounting. Pools are valued in kind: a share is a claim on
//! `vault / supply` of every asset, so no oracle is needed on-chain. Rounding
//! always favours the pool.

use crate::constants::BPS_DENOMINATOR;

pub fn mul_div_floor(a: u64, b: u64, denominator: u64) -> Option<u64> {
    if denominator == 0 {
        return None;
    }
    u64::try_from(a as u128 * b as u128 / denominator as u128).ok()
}

pub fn mul_div_ceil(a: u64, b: u64, denominator: u64) -> Option<u64> {
    if denominator == 0 {
        return None;
    }
    u64::try_from((a as u128 * b as u128).div_ceil(denominator as u128)).ok()
}

/// Asset amount a depositor must add to mint `shares`.
pub fn deposit_amount(vault_amount: u64, shares: u64, supply: u64) -> Option<u64> {
    mul_div_ceil(vault_amount, shares, supply)
}

/// Asset amount paid out for burning `shares`.
pub fn withdraw_amount(vault_amount: u64, shares: u64, supply: u64) -> Option<u64> {
    mul_div_floor(vault_amount, shares, supply)
}

pub fn withdraw_fee(shares: u64, fee_bps: u16) -> Option<u64> {
    mul_div_ceil(shares, fee_bps as u64, BPS_DENOMINATOR)
}

/// Largest share amount whose `deposit_amount`s all fit in `balances`. Used by
/// clients after swapping USDC into the basket.
pub fn max_mintable_shares(balances: &[u64], vault_amounts: &[u64], supply: u64) -> u64 {
    balances
        .iter()
        .zip(vault_amounts)
        .filter(|(_, vault)| **vault > 0)
        .map(|(balance, vault)| mul_div_floor(*balance, supply, *vault).unwrap_or(u64::MAX))
        .min()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deposit_rounds_up_withdraw_rounds_down() {
        assert_eq!(deposit_amount(10, 1, 3), Some(4));
        assert_eq!(withdraw_amount(10, 1, 3), Some(3));
        assert_eq!(deposit_amount(0, 5, 3), Some(0));
        assert_eq!(deposit_amount(1, 1, 0), None);
    }

    #[test]
    fn tiny_deposits_still_cost_something() {
        // One raw share of a pool holding a dust amount is never free.
        assert_eq!(deposit_amount(1, 1, 1_000_000), Some(1));
    }

    #[test]
    fn fee_rounds_up() {
        assert_eq!(withdraw_fee(10_000, 20), Some(20));
        assert_eq!(withdraw_fee(1, 20), Some(1));
        assert_eq!(withdraw_fee(1_000_000, 0), Some(0));
    }

    #[test]
    fn max_mintable_fits_every_balance() {
        let vaults = [3_000_000, 7_000_000, 0];
        let supply = 10_000_000;
        let balances = [301_234, 699_999, 0];
        let shares = max_mintable_shares(&balances, &vaults, supply);
        assert_eq!(shares, 999_998);
        for (balance, vault) in balances.iter().zip(vaults) {
            assert!(deposit_amount(vault, shares, supply).unwrap() <= *balance);
        }
    }

    #[test]
    fn no_overflow_on_large_values() {
        assert_eq!(mul_div_floor(u64::MAX, u64::MAX, u64::MAX), Some(u64::MAX));
        assert_eq!(mul_div_floor(u64::MAX, 2, 1), None);
    }
}
