use solana_program::{
    account_info::{AccountInfo, next_account_info},
    entrypoint::ProgramResult,
    msg,
    program_error::ProgramError,
    pubkey::Pubkey,
};

use super::{
    create_pda_account, load_config_as_admin, rent_exempt_lamports, require_key, require_signer,
    transfer, validate_weights, verify_asset,
};
use crate::{
    constants::{BPF_LOADER_UPGRADEABLE_ID, CONFIG_SEED, MAX_ADMINS, MAX_WITHDRAW_FEE_BPS, POOL_SEED},
    error::BeandxError,
    state::{CONFIG_KEY, Config, Pool},
};

pub fn initialize_config(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    treasury: Pubkey,
    withdraw_fee_bps: u16,
) -> ProgramResult {
    let accounts_iter = &mut accounts.iter();
    let super_admin = next_account_info(accounts_iter)?;
    let config_info = next_account_info(accounts_iter)?;
    let program_data = next_account_info(accounts_iter)?;
    let system_program = next_account_info(accounts_iter)?;

    require_signer(super_admin)?;
    // Only the upgrade authority can initialize, so nobody can front-run the
    // deploy and make themselves super admin.
    if upgrade_authority(program_id, program_data)? != Some(*super_admin.key) {
        return Err(BeandxError::Unauthorized.into());
    }
    if withdraw_fee_bps > MAX_WITHDRAW_FEE_BPS {
        return Err(BeandxError::FeeTooHigh.into());
    }

    let (config_address, bump) = Pubkey::find_program_address(&[CONFIG_SEED], program_id);
    require_key(config_info, &config_address, BeandxError::InvalidPda)?;
    if config_info.owner == program_id {
        return Err(BeandxError::AlreadyInitialized.into());
    }

    create_pda_account(
        super_admin,
        config_info,
        system_program,
        Config::LEN,
        rent_exempt_lamports(Config::LEN)?,
        program_id,
        &[CONFIG_SEED, &[bump]],
    )?;
    Config {
        key: CONFIG_KEY,
        bump,
        super_admin: *super_admin.key,
        treasury,
        withdraw_fee_bps,
        admins: Vec::new(),
    }
    .save(config_info)
}

/// Reads the upgrade authority from the program's ProgramData account
/// (bincode: u32 tag = 3, u64 slot, Option<Pubkey>).
fn upgrade_authority(program_id: &Pubkey, program_data: &AccountInfo) -> Result<Option<Pubkey>, ProgramError> {
    let expected = Pubkey::find_program_address(&[program_id.as_ref()], &BPF_LOADER_UPGRADEABLE_ID).0;
    if *program_data.key != expected || *program_data.owner != BPF_LOADER_UPGRADEABLE_ID {
        return Err(BeandxError::InvalidProgramAccount.into());
    }
    let data = program_data.try_borrow_data()?;
    if data.len() < 45 || data[0..4] != 3u32.to_le_bytes() {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(match data[12] {
        1 => Some(Pubkey::new_from_array(data[13..45].try_into().unwrap())),
        _ => None,
    })
}

fn load_config_as_super_admin<'a, 'info>(
    program_id: &Pubkey,
    accounts: &'a [AccountInfo<'info>],
) -> Result<(Config, &'a AccountInfo<'info>), ProgramError> {
    let accounts_iter = &mut accounts.iter();
    let super_admin = next_account_info(accounts_iter)?;
    let config_info = next_account_info(accounts_iter)?;

    require_signer(super_admin)?;
    let config = Config::load(config_info, program_id)?;
    if config.super_admin != *super_admin.key {
        return Err(BeandxError::Unauthorized.into());
    }
    Ok((config, config_info))
}

pub fn update_config(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    new_super_admin: Option<Pubkey>,
    treasury: Option<Pubkey>,
    withdraw_fee_bps: Option<u16>,
) -> ProgramResult {
    let (mut config, config_info) = load_config_as_super_admin(program_id, accounts)?;
    if let Some(new_super_admin) = new_super_admin {
        config.super_admin = new_super_admin;
    }
    if let Some(treasury) = treasury {
        config.treasury = treasury;
    }
    if let Some(fee) = withdraw_fee_bps {
        if fee > MAX_WITHDRAW_FEE_BPS {
            return Err(BeandxError::FeeTooHigh.into());
        }
        config.withdraw_fee_bps = fee;
    }
    config.save(config_info)
}

pub fn add_admin(program_id: &Pubkey, accounts: &[AccountInfo], admin: Pubkey) -> ProgramResult {
    let (mut config, config_info) = load_config_as_super_admin(program_id, accounts)?;
    if config.admins.contains(&admin) {
        return Err(BeandxError::AdminExists.into());
    }
    if config.admins.len() >= MAX_ADMINS {
        return Err(BeandxError::AdminListFull.into());
    }
    config.admins.push(admin);
    config.save(config_info)
}

pub fn remove_admin(program_id: &Pubkey, accounts: &[AccountInfo], admin: Pubkey) -> ProgramResult {
    let (mut config, config_info) = load_config_as_super_admin(program_id, accounts)?;
    let index = config
        .admins
        .iter()
        .position(|a| *a == admin)
        .ok_or(BeandxError::AdminNotFound)?;
    config.admins.remove(index);
    // The list shrinks, so zero the stale tail before writing.
    config_info.try_borrow_mut_data()?.fill(0);
    config.save(config_info)
}

pub fn set_weights(program_id: &Pubkey, accounts: &[AccountInfo], weights: Vec<u16>) -> ProgramResult {
    let (mut pool, pool_info) = load_pool_as_admin(program_id, accounts)?;
    validate_weights(&weights, pool.assets.len())?;
    for (asset, weight) in pool.assets.iter_mut().zip(weights) {
        asset.weight_bps = weight;
    }
    pool.save(pool_info)
}

pub fn set_paused(program_id: &Pubkey, accounts: &[AccountInfo], paused: bool) -> ProgramResult {
    let (mut pool, pool_info) = load_pool_as_admin(program_id, accounts)?;
    pool.paused = paused;
    pool.save(pool_info)
}

fn load_pool_as_admin<'a, 'info>(
    program_id: &Pubkey,
    accounts: &'a [AccountInfo<'info>],
) -> Result<(Pool, &'a AccountInfo<'info>), ProgramError> {
    let accounts_iter = &mut accounts.iter();
    let admin = next_account_info(accounts_iter)?;
    let config_info = next_account_info(accounts_iter)?;
    let pool_info = next_account_info(accounts_iter)?;

    load_config_as_admin(program_id, config_info, admin)?;
    Ok((Pool::load(pool_info, program_id)?, pool_info))
}

pub fn rebalance(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    out_index: u8,
    amount_out: u64,
    in_index: u8,
    amount_in: u64,
) -> ProgramResult {
    let accounts_iter = &mut accounts.iter();
    let admin = next_account_info(accounts_iter)?;
    let config_info = next_account_info(accounts_iter)?;
    let pool_info = next_account_info(accounts_iter)?;
    let out_mint = next_account_info(accounts_iter)?;
    let out_vault = next_account_info(accounts_iter)?;
    let admin_out = next_account_info(accounts_iter)?;
    let out_token_program = next_account_info(accounts_iter)?;
    let in_mint = next_account_info(accounts_iter)?;
    let admin_in = next_account_info(accounts_iter)?;
    let in_vault = next_account_info(accounts_iter)?;
    let in_token_program = next_account_info(accounts_iter)?;

    load_config_as_admin(program_id, config_info, admin)?;
    let pool = Pool::load(pool_info, program_id)?;
    if out_index == in_index {
        return Err(BeandxError::InvalidAssetIndex.into());
    }
    let asset_out = pool.assets.get(out_index as usize).ok_or(BeandxError::InvalidAssetIndex)?;
    let asset_in = pool.assets.get(in_index as usize).ok_or(BeandxError::InvalidAssetIndex)?;
    if amount_out == 0 || amount_in == 0 {
        return Err(BeandxError::ZeroAmount.into());
    }
    verify_asset(asset_out, out_mint, out_vault, out_token_program)?;
    verify_asset(asset_in, in_mint, in_vault, in_token_program)?;

    transfer(in_token_program, admin_in, in_mint, in_vault, admin, amount_in, asset_in.decimals, &[])?;
    let bump = [pool.bump];
    let pool_seeds: &[&[u8]] = &[POOL_SEED, pool.symbol.as_bytes(), &bump];
    transfer(
        out_token_program,
        out_vault,
        out_mint,
        admin_out,
        pool_info,
        amount_out,
        asset_out.decimals,
        pool_seeds,
    )?;
    msg!("Rebalance {}: out #{} {} / in #{} {}", pool.symbol, out_index, amount_out, in_index, amount_in);
    Ok(())
}
