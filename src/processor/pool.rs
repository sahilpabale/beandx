use solana_program::{
    account_info::{AccountInfo, next_account_info},
    entrypoint::ProgramResult,
    msg,
    program::{invoke, invoke_signed},
    pubkey::Pubkey,
};

use super::{
    asset_groups, create_pda_account, load_config_as_admin, rent_exempt_lamports, require_key, transfer,
    validate_weights, verify_asset,
};
use crate::{
    constants::{
        ASSOCIATED_TOKEN_PROGRAM_ID, MAX_NAME_LEN, MAX_SYMBOL_LEN, MAX_URI_LEN, MINT_SEED, POOL_SEED,
        RECEIPT_DECIMALS, TOKEN_2022_PROGRAM_ID,
    },
    error::BeandxError,
    state::{POOL_KEY, Pool, PoolAsset},
    token,
};

pub fn create_pool(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    symbol: String,
    name: String,
    uri: String,
    weights: Vec<u16>,
) -> ProgramResult {
    let accounts_iter = &mut accounts.iter();
    let admin = next_account_info(accounts_iter)?;
    let config_info = next_account_info(accounts_iter)?;
    let pool_info = next_account_info(accounts_iter)?;
    let mint_info = next_account_info(accounts_iter)?;
    let system_program = next_account_info(accounts_iter)?;
    let token_2022 = next_account_info(accounts_iter)?;
    let ata_program = next_account_info(accounts_iter)?;

    load_config_as_admin(program_id, config_info, admin)?;
    if symbol.is_empty() || symbol.len() > MAX_SYMBOL_LEN || !symbol.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(BeandxError::InvalidSymbol.into());
    }
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err(BeandxError::InvalidName.into());
    }
    if uri.len() > MAX_URI_LEN {
        return Err(BeandxError::InvalidUri.into());
    }
    validate_weights(&weights, weights.len())?;
    if weights.contains(&0) {
        return Err(BeandxError::InvalidWeights.into());
    }
    require_key(token_2022, &TOKEN_2022_PROGRAM_ID, BeandxError::InvalidTokenProgram)?;
    require_key(ata_program, &ASSOCIATED_TOKEN_PROGRAM_ID, BeandxError::InvalidProgramAccount)?;

    let (pool_address, pool_bump) = Pubkey::find_program_address(&[POOL_SEED, symbol.as_bytes()], program_id);
    require_key(pool_info, &pool_address, BeandxError::InvalidPda)?;
    let (mint_address, mint_bump) = Pubkey::find_program_address(&[MINT_SEED, pool_address.as_ref()], program_id);
    require_key(mint_info, &mint_address, BeandxError::InvalidPda)?;
    if pool_info.owner == program_id {
        return Err(BeandxError::AlreadyInitialized.into());
    }

    let mut assets = Vec::with_capacity(weights.len());
    for (group, weight) in asset_groups(accounts_iter.as_slice(), weights.len(), 3)?.zip(&weights) {
        let [asset_mint, vault, asset_token_program] = group else { unreachable!() };
        if !token::is_token_program(asset_token_program.key) || asset_mint.owner != asset_token_program.key {
            return Err(BeandxError::InvalidTokenProgram.into());
        }
        if assets.iter().any(|a: &PoolAsset| a.mint == *asset_mint.key) {
            return Err(BeandxError::DuplicateAsset.into());
        }
        let mint = token::read_mint(asset_mint)?;
        token::check_mint_extensions(asset_mint)?;
        // The ATA program re-derives and checks this address when creating it.
        assets.push(PoolAsset {
            mint: *asset_mint.key,
            token_program: *asset_token_program.key,
            vault: *vault.key,
            decimals: mint.decimals,
            weight_bps: *weight,
        });
    }

    let pool_bump_seed = [pool_bump];
    let pool_seeds: &[&[u8]] = &[POOL_SEED, symbol.as_bytes(), &pool_bump_seed];
    create_pda_account(
        admin,
        pool_info,
        system_program,
        Pool::LEN,
        rent_exempt_lamports(Pool::LEN)?,
        program_id,
        pool_seeds,
    )?;

    // Receipt mint: Token-2022 with metadata stored on the mint itself. The
    // account is created at its pre-metadata size but funded for the final
    // size, because Token-2022 reallocs when writing the metadata.
    let mint_bump_seed = [mint_bump];
    create_pda_account(
        admin,
        mint_info,
        system_program,
        token::RECEIPT_MINT_BASE_LEN,
        rent_exempt_lamports(token::receipt_mint_len(&name, &symbol, &uri))?,
        &TOKEN_2022_PROGRAM_ID,
        &[MINT_SEED, pool_address.as_ref(), &mint_bump_seed],
    )?;
    invoke(
        &token::initialize_metadata_pointer(mint_info.key, admin.key, mint_info.key),
        &[mint_info.clone(), token_2022.clone()],
    )?;
    invoke(
        &token::initialize_mint2(&TOKEN_2022_PROGRAM_ID, mint_info.key, pool_info.key, RECEIPT_DECIMALS),
        &[mint_info.clone(), token_2022.clone()],
    )?;
    invoke_signed(
        &token::initialize_token_metadata(mint_info.key, admin.key, pool_info.key, &name, &symbol, &uri),
        &[mint_info.clone(), admin.clone(), pool_info.clone(), token_2022.clone()],
        &[pool_seeds],
    )?;

    for group in accounts_iter.as_slice().chunks(3) {
        let [asset_mint, vault, asset_token_program] = group else { unreachable!() };
        invoke(
            &token::create_associated_token_account_idempotent(
                admin.key,
                pool_info.key,
                asset_mint.key,
                asset_token_program.key,
            ),
            &[
                admin.clone(),
                vault.clone(),
                pool_info.clone(),
                asset_mint.clone(),
                system_program.clone(),
                asset_token_program.clone(),
                ata_program.clone(),
            ],
        )?;
    }

    msg!("Created pool {} with {} assets", symbol, assets.len());
    Pool {
        key: POOL_KEY,
        bump: pool_bump,
        mint_bump,
        paused: false,
        symbol,
        mint: mint_address,
        assets,
    }
    .save(pool_info)
}

pub fn seed_pool(program_id: &Pubkey, accounts: &[AccountInfo], shares: u64, amounts: Vec<u64>) -> ProgramResult {
    let accounts_iter = &mut accounts.iter();
    let admin = next_account_info(accounts_iter)?;
    let config_info = next_account_info(accounts_iter)?;
    let pool_info = next_account_info(accounts_iter)?;
    let mint_info = next_account_info(accounts_iter)?;
    let admin_receipt = next_account_info(accounts_iter)?;
    let token_2022 = next_account_info(accounts_iter)?;

    load_config_as_admin(program_id, config_info, admin)?;
    let pool = Pool::load(pool_info, program_id)?;
    require_key(mint_info, &pool.mint, BeandxError::InvalidAssetAccount)?;
    require_key(token_2022, &TOKEN_2022_PROGRAM_ID, BeandxError::InvalidTokenProgram)?;
    // Allowed again if every holder has exited; leftover dust goes to the new seed.
    if token::read_mint(mint_info)?.supply != 0 {
        return Err(BeandxError::PoolAlreadySeeded.into());
    }
    if amounts.len() != pool.assets.len() {
        return Err(BeandxError::InvalidAssetCount.into());
    }
    // A zero balance could never grow again, since deposits are proportional.
    if shares == 0 || amounts.contains(&0) {
        return Err(BeandxError::ZeroAmount.into());
    }

    let groups = asset_groups(accounts_iter.as_slice(), pool.assets.len(), 4)?;
    for ((group, asset), amount) in groups.zip(&pool.assets).zip(&amounts) {
        let [asset_mint, source, vault, asset_token_program] = group else { unreachable!() };
        verify_asset(asset, asset_mint, vault, asset_token_program)?;
        transfer(asset_token_program, source, asset_mint, vault, admin, *amount, asset.decimals, &[])?;
    }

    let bump = [pool.bump];
    invoke_signed(
        &token::mint_to_checked(
            &TOKEN_2022_PROGRAM_ID,
            mint_info.key,
            admin_receipt.key,
            pool_info.key,
            shares,
            RECEIPT_DECIMALS,
        ),
        &[mint_info.clone(), admin_receipt.clone(), pool_info.clone(), token_2022.clone()],
        &[&[POOL_SEED, pool.symbol.as_bytes(), &bump]],
    )?;
    msg!("Seeded pool {} with {} shares", pool.symbol, shares);
    Ok(())
}
