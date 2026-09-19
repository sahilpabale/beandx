use solana_program::{
    account_info::{AccountInfo, next_account_info},
    entrypoint::ProgramResult,
    msg,
    program::{invoke, invoke_signed},
    pubkey::Pubkey,
};

use super::{asset_groups, require_key, require_signer, transfer, verify_asset};
use crate::{
    constants::{POOL_SEED, RECEIPT_DECIMALS, TOKEN_2022_PROGRAM_ID},
    error::BeandxError,
    math,
    state::{Config, Pool},
    token,
};

pub fn deposit(program_id: &Pubkey, accounts: &[AccountInfo], shares: u64, max_amounts: Vec<u64>) -> ProgramResult {
    let accounts_iter = &mut accounts.iter();
    let user = next_account_info(accounts_iter)?;
    let pool_info = next_account_info(accounts_iter)?;
    let mint_info = next_account_info(accounts_iter)?;
    let user_receipt = next_account_info(accounts_iter)?;
    let token_2022 = next_account_info(accounts_iter)?;

    require_signer(user)?;
    let pool = Pool::load(pool_info, program_id)?;
    if pool.paused {
        return Err(BeandxError::PoolPaused.into());
    }
    require_key(mint_info, &pool.mint, BeandxError::InvalidAssetAccount)?;
    require_key(token_2022, &TOKEN_2022_PROGRAM_ID, BeandxError::InvalidTokenProgram)?;
    if max_amounts.len() != pool.assets.len() {
        return Err(BeandxError::InvalidAssetCount.into());
    }
    if shares == 0 {
        return Err(BeandxError::ZeroAmount.into());
    }
    let supply = token::read_mint(mint_info)?.supply;
    if supply == 0 {
        return Err(BeandxError::PoolNotSeeded.into());
    }

    let groups = asset_groups(accounts_iter.as_slice(), pool.assets.len(), 4)?;
    for ((group, asset), max_amount) in groups.zip(&pool.assets).zip(&max_amounts) {
        let [asset_mint, source, vault, asset_token_program] = group else { unreachable!() };
        verify_asset(asset, asset_mint, vault, asset_token_program)?;
        let vault_amount = token::read_token_account(vault)?.amount;
        let amount = math::deposit_amount(vault_amount, shares, supply).ok_or(BeandxError::MathOverflow)?;
        if amount > *max_amount {
            return Err(BeandxError::SlippageExceeded.into());
        }
        if amount > 0 {
            transfer(asset_token_program, source, asset_mint, vault, user, amount, asset.decimals, &[])?;
        }
    }

    let bump = [pool.bump];
    invoke_signed(
        &token::mint_to_checked(
            &TOKEN_2022_PROGRAM_ID,
            mint_info.key,
            user_receipt.key,
            pool_info.key,
            shares,
            RECEIPT_DECIMALS,
        ),
        &[mint_info.clone(), user_receipt.clone(), pool_info.clone(), token_2022.clone()],
        &[&[POOL_SEED, pool.symbol.as_bytes(), &bump]],
    )?;
    msg!("Deposit {}: minted {} shares", pool.symbol, shares);
    Ok(())
}

pub fn withdraw(program_id: &Pubkey, accounts: &[AccountInfo], shares: u64, min_amounts: Vec<u64>) -> ProgramResult {
    let accounts_iter = &mut accounts.iter();
    let user = next_account_info(accounts_iter)?;
    let config_info = next_account_info(accounts_iter)?;
    let pool_info = next_account_info(accounts_iter)?;
    let mint_info = next_account_info(accounts_iter)?;
    let user_receipt = next_account_info(accounts_iter)?;
    let treasury_receipt = next_account_info(accounts_iter)?;
    let token_2022 = next_account_info(accounts_iter)?;

    require_signer(user)?;
    let config = Config::load(config_info, program_id)?;
    let pool = Pool::load(pool_info, program_id)?;
    require_key(mint_info, &pool.mint, BeandxError::InvalidAssetAccount)?;
    require_key(token_2022, &TOKEN_2022_PROGRAM_ID, BeandxError::InvalidTokenProgram)?;
    let treasury = token::read_token_account(treasury_receipt)?;
    if treasury.mint != pool.mint || treasury.owner != config.treasury {
        return Err(BeandxError::InvalidTreasuryAccount.into());
    }
    if min_amounts.len() != pool.assets.len() {
        return Err(BeandxError::InvalidAssetCount.into());
    }

    let supply = token::read_mint(mint_info)?.supply;
    let fee = math::withdraw_fee(shares, config.withdraw_fee_bps).ok_or(BeandxError::MathOverflow)?;
    let burn = shares.saturating_sub(fee);
    if burn == 0 {
        return Err(BeandxError::ZeroAmount.into());
    }

    // Payouts use the pre-burn supply. The fee stays in circulation (held by
    // the treasury), so it keeps its claim on the basket.
    let groups = asset_groups(accounts_iter.as_slice(), pool.assets.len(), 4)?;
    let mut payouts = Vec::with_capacity(pool.assets.len());
    for ((group, asset), min_amount) in groups.clone().zip(&pool.assets).zip(&min_amounts) {
        let [asset_mint, vault, _destination, asset_token_program] = group else { unreachable!() };
        verify_asset(asset, asset_mint, vault, asset_token_program)?;
        let vault_amount = token::read_token_account(vault)?.amount;
        let amount = math::withdraw_amount(vault_amount, burn, supply).ok_or(BeandxError::MathOverflow)?;
        if amount < *min_amount {
            return Err(BeandxError::SlippageExceeded.into());
        }
        payouts.push(amount);
    }

    if fee > 0 {
        transfer(token_2022, user_receipt, mint_info, treasury_receipt, user, fee, RECEIPT_DECIMALS, &[])?;
    }
    invoke(
        &token::burn_checked(&TOKEN_2022_PROGRAM_ID, user_receipt.key, mint_info.key, user.key, burn, RECEIPT_DECIMALS),
        &[user_receipt.clone(), mint_info.clone(), user.clone(), token_2022.clone()],
    )?;

    let bump = [pool.bump];
    let pool_seeds: &[&[u8]] = &[POOL_SEED, pool.symbol.as_bytes(), &bump];
    for ((group, asset), amount) in groups.zip(&pool.assets).zip(payouts) {
        let [asset_mint, vault, destination, asset_token_program] = group else { unreachable!() };
        if amount > 0 {
            transfer(asset_token_program, vault, asset_mint, destination, pool_info, amount, asset.decimals, pool_seeds)?;
        }
    }
    msg!("Withdraw {}: burned {} shares, fee {}", pool.symbol, burn, fee);
    Ok(())
}
