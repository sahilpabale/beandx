use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{account_info::AccountInfo, program_error::ProgramError, pubkey::Pubkey};

use crate::constants::{MAX_ADMINS, MAX_ASSETS, MAX_SYMBOL_LEN};

pub const CONFIG_KEY: u8 = 1;
pub const POOL_KEY: u8 = 2;

/// Global settings, stored at the `[CONFIG_SEED]` PDA.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq)]
pub struct Config {
    pub key: u8,
    pub bump: u8,
    pub super_admin: Pubkey,
    /// Wallet whose receipt-token accounts collect withdrawal fees.
    pub treasury: Pubkey,
    pub withdraw_fee_bps: u16,
    /// Can create and manage pools. The super admin is implicitly an admin.
    pub admins: Vec<Pubkey>,
}

impl Config {
    pub const LEN: usize = 1 + 1 + 32 + 32 + 2 + 4 + 32 * MAX_ADMINS;

    pub fn is_admin(&self, key: &Pubkey) -> bool {
        self.super_admin == *key || self.admins.contains(key)
    }

    pub fn load(account: &AccountInfo, program_id: &Pubkey) -> Result<Self, ProgramError> {
        load_account(account, program_id, CONFIG_KEY)
    }

    pub fn save(&self, account: &AccountInfo) -> Result<(), ProgramError> {
        save_account(self, account)
    }
}

/// One themed basket, stored at the `[POOL_SEED, symbol]` PDA. The pool PDA owns
/// every vault and is the mint authority of the receipt token.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq)]
pub struct Pool {
    pub key: u8,
    pub bump: u8,
    pub mint_bump: u8,
    /// Blocks deposits only, so holders can always exit.
    pub paused: bool,
    pub symbol: String,
    pub mint: Pubkey,
    pub assets: Vec<PoolAsset>,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq)]
pub struct PoolAsset {
    pub mint: Pubkey,
    pub token_program: Pubkey,
    /// The pool's associated token account for `mint`.
    pub vault: Pubkey,
    pub decimals: u8,
    /// Target weight, used by the admin for seeding and rebalancing.
    pub weight_bps: u16,
}

impl PoolAsset {
    pub const LEN: usize = 32 + 32 + 32 + 1 + 2;
}

impl Pool {
    pub const LEN: usize = 1 + 1 + 1 + 1 + (4 + MAX_SYMBOL_LEN) + 32 + 4 + PoolAsset::LEN * MAX_ASSETS;

    pub fn load(account: &AccountInfo, program_id: &Pubkey) -> Result<Self, ProgramError> {
        load_account(account, program_id, POOL_KEY)
    }

    pub fn save(&self, account: &AccountInfo) -> Result<(), ProgramError> {
        save_account(self, account)
    }
}

/// Accounts are allocated at their max size, so trailing bytes are expected.
fn load_account<T: BorshDeserialize>(
    account: &AccountInfo,
    program_id: &Pubkey,
    key: u8,
) -> Result<T, ProgramError> {
    if account.owner != program_id {
        return Err(ProgramError::IncorrectProgramId);
    }
    let data = account.try_borrow_data()?;
    if data.first() != Some(&key) {
        return Err(ProgramError::InvalidAccountData);
    }
    T::deserialize(&mut &data[..]).map_err(|_| ProgramError::InvalidAccountData)
}

fn save_account<T: BorshSerialize>(value: &T, account: &AccountInfo) -> Result<(), ProgramError> {
    let mut data = account.try_borrow_mut_data()?;
    borsh::to_writer(&mut &mut data[..], value).map_err(|_| ProgramError::AccountDataTooSmall)
}
