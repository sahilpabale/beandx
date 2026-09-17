use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

use crate::{
    constants::{
        ASSOCIATED_TOKEN_PROGRAM_ID, BPF_LOADER_UPGRADEABLE_ID, CONFIG_SEED, MINT_SEED, POOL_SEED,
        SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID,
    },
    token::get_associated_token_address,
};

/// Per-asset account groups are passed after the fixed accounts, in the same
/// order as `Pool::assets`.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq)]
pub enum BeandxInstruction {
    /// Creates the global config. Only the program's upgrade authority may call it.
    ///
    /// 0. `[signer, writable]` super admin (payer, must be the upgrade authority)
    /// 1. `[writable]` config PDA
    /// 2. `[]` program data account
    /// 3. `[]` system program
    InitializeConfig { treasury: Pubkey, withdraw_fee_bps: u16 },

    /// 0. `[signer]` super admin
    /// 1. `[writable]` config
    UpdateConfig {
        new_super_admin: Option<Pubkey>,
        treasury: Option<Pubkey>,
        withdraw_fee_bps: Option<u16>,
    },

    /// 0. `[signer]` super admin
    /// 1. `[writable]` config
    AddAdmin { admin: Pubkey },

    /// 0. `[signer]` super admin
    /// 1. `[writable]` config
    RemoveAdmin { admin: Pubkey },

    /// Creates a pool, its Token-2022 receipt mint (with on-mint metadata) and
    /// one vault per asset. `weights` are in bps and must sum to 10_000.
    ///
    /// 0. `[signer, writable]` admin (payer, receipt metadata update authority)
    /// 1. `[]` config
    /// 2. `[writable]` pool PDA
    /// 3. `[writable]` receipt mint PDA
    /// 4. `[]` system program
    /// 5. `[]` Token-2022 program
    /// 6. `[]` associated token program
    ///
    /// Per asset:
    /// - `[]` asset mint
    /// - `[writable]` vault (pool's ATA)
    /// - `[]` asset token program
    CreatePool { symbol: String, name: String, uri: String, weights: Vec<u16> },

    /// First deposit, made by an admin, which sets the starting share price.
    /// Mint `shares` equal to the basket's USD value (in 6 decimals) for $1 per share.
    ///
    /// 0. `[signer]` admin
    /// 1. `[]` config
    /// 2. `[]` pool
    /// 3. `[writable]` receipt mint
    /// 4. `[writable]` admin's receipt token account
    /// 5. `[]` Token-2022 program
    ///
    /// Per asset:
    /// - `[]` asset mint
    /// - `[writable]` admin's asset token account
    /// - `[writable]` vault
    /// - `[]` asset token program
    SeedPool { shares: u64, amounts: Vec<u64> },

    /// Mints exactly `shares` in exchange for each asset in proportion to the
    /// pool's current holdings (rounded up). `max_amounts` guards against a
    /// rebalance landing first.
    ///
    /// 0. `[signer]` user
    /// 1. `[]` pool
    /// 2. `[writable]` receipt mint
    /// 3. `[writable]` user's receipt token account
    /// 4. `[]` Token-2022 program
    ///
    /// Per asset:
    /// - `[]` asset mint
    /// - `[writable]` user's asset token account
    /// - `[writable]` vault
    /// - `[]` asset token program
    Deposit { shares: u64, max_amounts: Vec<u64> },

    /// Burns `shares` minus the withdrawal fee (sent to the treasury as
    /// receipt tokens) and pays out the pro-rata basket (rounded down).
    ///
    /// 0. `[signer]` user
    /// 1. `[]` config
    /// 2. `[]` pool
    /// 3. `[writable]` receipt mint
    /// 4. `[writable]` user's receipt token account
    /// 5. `[writable]` treasury's receipt token account
    /// 6. `[]` Token-2022 program
    ///
    /// Per asset:
    /// - `[]` asset mint
    /// - `[writable]` vault
    /// - `[writable]` user's asset token account
    /// - `[]` asset token program
    Withdraw { shares: u64, min_amounts: Vec<u64> },

    /// Admin swaps `amount_in` of one pool asset in for `amount_out` of another.
    /// The trade itself happens off-program; the admin is trusted on price.
    ///
    /// 0. `[signer]` admin
    /// 1. `[]` config
    /// 2. `[]` pool
    /// 3. `[]` asset-out mint
    /// 4. `[writable]` asset-out vault
    /// 5. `[writable]` admin's asset-out token account
    /// 6. `[]` asset-out token program
    /// 7. `[]` asset-in mint
    /// 8. `[writable]` admin's asset-in token account
    /// 9. `[writable]` asset-in vault
    /// 10. `[]` asset-in token program
    Rebalance { out_index: u8, amount_out: u64, in_index: u8, amount_in: u64 },

    /// 0. `[signer]` admin
    /// 1. `[]` config
    /// 2. `[writable]` pool
    SetWeights { weights: Vec<u16> },

    /// 0. `[signer]` admin
    /// 1. `[]` config
    /// 2. `[writable]` pool
    SetPaused { paused: bool },
}

#[derive(Clone, Copy, Debug)]
pub struct AssetKeys {
    pub mint: Pubkey,
    pub token_program: Pubkey,
}

pub fn find_config_address() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[CONFIG_SEED], &crate::ID)
}

pub fn find_pool_address(symbol: &str) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[POOL_SEED, symbol.as_bytes()], &crate::ID)
}

pub fn find_receipt_mint_address(pool: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[MINT_SEED, pool.as_ref()], &crate::ID)
}

pub fn find_program_data_address() -> Pubkey {
    Pubkey::find_program_address(&[crate::ID.as_ref()], &BPF_LOADER_UPGRADEABLE_ID).0
}

fn build(accounts: Vec<AccountMeta>, data: BeandxInstruction) -> Instruction {
    Instruction {
        program_id: crate::ID,
        accounts,
        data: borsh::to_vec(&data).unwrap(),
    }
}

pub fn initialize_config(super_admin: &Pubkey, treasury: &Pubkey, withdraw_fee_bps: u16) -> Instruction {
    build(
        vec![
            AccountMeta::new(*super_admin, true),
            AccountMeta::new(find_config_address().0, false),
            AccountMeta::new_readonly(find_program_data_address(), false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ],
        BeandxInstruction::InitializeConfig { treasury: *treasury, withdraw_fee_bps },
    )
}

pub fn update_config(
    super_admin: &Pubkey,
    new_super_admin: Option<Pubkey>,
    treasury: Option<Pubkey>,
    withdraw_fee_bps: Option<u16>,
) -> Instruction {
    build(
        super_admin_accounts(super_admin),
        BeandxInstruction::UpdateConfig { new_super_admin, treasury, withdraw_fee_bps },
    )
}

pub fn add_admin(super_admin: &Pubkey, admin: &Pubkey) -> Instruction {
    build(super_admin_accounts(super_admin), BeandxInstruction::AddAdmin { admin: *admin })
}

pub fn remove_admin(super_admin: &Pubkey, admin: &Pubkey) -> Instruction {
    build(super_admin_accounts(super_admin), BeandxInstruction::RemoveAdmin { admin: *admin })
}

fn super_admin_accounts(super_admin: &Pubkey) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new_readonly(*super_admin, true),
        AccountMeta::new(find_config_address().0, false),
    ]
}

pub fn create_pool(
    admin: &Pubkey,
    symbol: &str,
    name: &str,
    uri: &str,
    assets: &[AssetKeys],
    weights: Vec<u16>,
) -> Instruction {
    let pool = find_pool_address(symbol).0;
    let mut accounts = vec![
        AccountMeta::new(*admin, true),
        AccountMeta::new_readonly(find_config_address().0, false),
        AccountMeta::new(pool, false),
        AccountMeta::new(find_receipt_mint_address(&pool).0, false),
        AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
        AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID, false),
    ];
    for asset in assets {
        accounts.push(AccountMeta::new_readonly(asset.mint, false));
        accounts.push(AccountMeta::new(
            get_associated_token_address(&pool, &asset.mint, &asset.token_program),
            false,
        ));
        accounts.push(AccountMeta::new_readonly(asset.token_program, false));
    }
    build(
        accounts,
        BeandxInstruction::CreatePool {
            symbol: symbol.to_string(),
            name: name.to_string(),
            uri: uri.to_string(),
            weights,
        },
    )
}

/// `admin` funds the seed from its associated token accounts.
pub fn seed_pool(admin: &Pubkey, symbol: &str, assets: &[AssetKeys], shares: u64, amounts: Vec<u64>) -> Instruction {
    let pool = find_pool_address(symbol).0;
    let mint = find_receipt_mint_address(&pool).0;
    let mut accounts = vec![
        AccountMeta::new_readonly(*admin, true),
        AccountMeta::new_readonly(find_config_address().0, false),
        AccountMeta::new_readonly(pool, false),
        AccountMeta::new(mint, false),
        AccountMeta::new(get_associated_token_address(admin, &mint, &TOKEN_2022_PROGRAM_ID), false),
        AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
    ];
    push_user_to_vault_accounts(&mut accounts, admin, &pool, assets);
    build(accounts, BeandxInstruction::SeedPool { shares, amounts })
}

/// `user` deposits from, and receives shares into, its associated token accounts.
pub fn deposit(user: &Pubkey, symbol: &str, assets: &[AssetKeys], shares: u64, max_amounts: Vec<u64>) -> Instruction {
    let pool = find_pool_address(symbol).0;
    let mint = find_receipt_mint_address(&pool).0;
    let mut accounts = vec![
        AccountMeta::new_readonly(*user, true),
        AccountMeta::new_readonly(pool, false),
        AccountMeta::new(mint, false),
        AccountMeta::new(get_associated_token_address(user, &mint, &TOKEN_2022_PROGRAM_ID), false),
        AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
    ];
    push_user_to_vault_accounts(&mut accounts, user, &pool, assets);
    build(accounts, BeandxInstruction::Deposit { shares, max_amounts })
}

/// Pays out into `user`'s associated token accounts, which must already exist.
pub fn withdraw(
    user: &Pubkey,
    treasury: &Pubkey,
    symbol: &str,
    assets: &[AssetKeys],
    shares: u64,
    min_amounts: Vec<u64>,
) -> Instruction {
    let pool = find_pool_address(symbol).0;
    let mint = find_receipt_mint_address(&pool).0;
    let mut accounts = vec![
        AccountMeta::new_readonly(*user, true),
        AccountMeta::new_readonly(find_config_address().0, false),
        AccountMeta::new_readonly(pool, false),
        AccountMeta::new(mint, false),
        AccountMeta::new(get_associated_token_address(user, &mint, &TOKEN_2022_PROGRAM_ID), false),
        AccountMeta::new(get_associated_token_address(treasury, &mint, &TOKEN_2022_PROGRAM_ID), false),
        AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
    ];
    for asset in assets {
        accounts.push(AccountMeta::new_readonly(asset.mint, false));
        accounts.push(AccountMeta::new(
            get_associated_token_address(&pool, &asset.mint, &asset.token_program),
            false,
        ));
        accounts.push(AccountMeta::new(
            get_associated_token_address(user, &asset.mint, &asset.token_program),
            false,
        ));
        accounts.push(AccountMeta::new_readonly(asset.token_program, false));
    }
    build(accounts, BeandxInstruction::Withdraw { shares, min_amounts })
}

#[allow(clippy::too_many_arguments)]
pub fn rebalance(
    admin: &Pubkey,
    symbol: &str,
    out_index: u8,
    asset_out: &AssetKeys,
    amount_out: u64,
    in_index: u8,
    asset_in: &AssetKeys,
    amount_in: u64,
) -> Instruction {
    let pool = find_pool_address(symbol).0;
    build(
        vec![
            AccountMeta::new_readonly(*admin, true),
            AccountMeta::new_readonly(find_config_address().0, false),
            AccountMeta::new_readonly(pool, false),
            AccountMeta::new_readonly(asset_out.mint, false),
            AccountMeta::new(
                get_associated_token_address(&pool, &asset_out.mint, &asset_out.token_program),
                false,
            ),
            AccountMeta::new(
                get_associated_token_address(admin, &asset_out.mint, &asset_out.token_program),
                false,
            ),
            AccountMeta::new_readonly(asset_out.token_program, false),
            AccountMeta::new_readonly(asset_in.mint, false),
            AccountMeta::new(
                get_associated_token_address(admin, &asset_in.mint, &asset_in.token_program),
                false,
            ),
            AccountMeta::new(
                get_associated_token_address(&pool, &asset_in.mint, &asset_in.token_program),
                false,
            ),
            AccountMeta::new_readonly(asset_in.token_program, false),
        ],
        BeandxInstruction::Rebalance { out_index, amount_out, in_index, amount_in },
    )
}

pub fn set_weights(admin: &Pubkey, symbol: &str, weights: Vec<u16>) -> Instruction {
    build(pool_admin_accounts(admin, symbol), BeandxInstruction::SetWeights { weights })
}

pub fn set_paused(admin: &Pubkey, symbol: &str, paused: bool) -> Instruction {
    build(pool_admin_accounts(admin, symbol), BeandxInstruction::SetPaused { paused })
}

fn pool_admin_accounts(admin: &Pubkey, symbol: &str) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new_readonly(*admin, true),
        AccountMeta::new_readonly(find_config_address().0, false),
        AccountMeta::new(find_pool_address(symbol).0, false),
    ]
}

fn push_user_to_vault_accounts(accounts: &mut Vec<AccountMeta>, user: &Pubkey, pool: &Pubkey, assets: &[AssetKeys]) {
    for asset in assets {
        accounts.push(AccountMeta::new_readonly(asset.mint, false));
        accounts.push(AccountMeta::new(
            get_associated_token_address(user, &asset.mint, &asset.token_program),
            false,
        ));
        accounts.push(AccountMeta::new(
            get_associated_token_address(pool, &asset.mint, &asset.token_program),
            false,
        ));
        accounts.push(AccountMeta::new_readonly(asset.token_program, false));
    }
}
