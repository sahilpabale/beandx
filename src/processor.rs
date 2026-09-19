mod admin;
mod pool;
mod shares;

use borsh::BorshDeserialize;
use solana_program::{
    account_info::AccountInfo,
    entrypoint::ProgramResult,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    pubkey::Pubkey,
    rent::Rent,
    sysvar::Sysvar,
};

use crate::{
    constants::{BPS_DENOMINATOR, MAX_ASSETS, SYSTEM_PROGRAM_ID},
    error::BeandxError,
    instruction::BeandxInstruction,
    state::{Config, PoolAsset},
    token,
};

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    let instruction = BeandxInstruction::try_from_slice(instruction_data)
        .map_err(|_| ProgramError::InvalidInstructionData)?;

    match instruction {
        BeandxInstruction::InitializeConfig { treasury, withdraw_fee_bps } => {
            admin::initialize_config(program_id, accounts, treasury, withdraw_fee_bps)
        }
        BeandxInstruction::UpdateConfig { new_super_admin, treasury, withdraw_fee_bps } => {
            admin::update_config(program_id, accounts, new_super_admin, treasury, withdraw_fee_bps)
        }
        BeandxInstruction::AddAdmin { admin } => admin::add_admin(program_id, accounts, admin),
        BeandxInstruction::RemoveAdmin { admin } => admin::remove_admin(program_id, accounts, admin),
        BeandxInstruction::CreatePool { symbol, name, uri, weights } => {
            pool::create_pool(program_id, accounts, symbol, name, uri, weights)
        }
        BeandxInstruction::SeedPool { shares, amounts } => {
            pool::seed_pool(program_id, accounts, shares, amounts)
        }
        BeandxInstruction::Deposit { shares, max_amounts } => {
            shares::deposit(program_id, accounts, shares, max_amounts)
        }
        BeandxInstruction::Withdraw { shares, min_amounts } => {
            shares::withdraw(program_id, accounts, shares, min_amounts)
        }
        BeandxInstruction::Rebalance { out_index, amount_out, in_index, amount_in } => {
            admin::rebalance(program_id, accounts, out_index, amount_out, in_index, amount_in)
        }
        BeandxInstruction::SetWeights { weights } => admin::set_weights(program_id, accounts, weights),
        BeandxInstruction::SetPaused { paused } => admin::set_paused(program_id, accounts, paused),
    }
}

fn require_signer(account: &AccountInfo) -> ProgramResult {
    if !account.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    Ok(())
}

fn require_key(account: &AccountInfo, expected: &Pubkey, err: BeandxError) -> ProgramResult {
    if account.key != expected {
        return Err(err.into());
    }
    Ok(())
}

/// Loads the config and checks `admin` signed and is a pool admin.
fn load_config_as_admin(
    program_id: &Pubkey,
    config_info: &AccountInfo,
    admin: &AccountInfo,
) -> Result<Config, ProgramError> {
    require_signer(admin)?;
    let config = Config::load(config_info, program_id)?;
    if !config.is_admin(admin.key) {
        return Err(BeandxError::Unauthorized.into());
    }
    Ok(config)
}

fn validate_weights(weights: &[u16], asset_count: usize) -> ProgramResult {
    if weights.is_empty() || weights.len() > MAX_ASSETS {
        return Err(BeandxError::InvalidAssetCount.into());
    }
    if weights.len() != asset_count {
        return Err(BeandxError::InvalidWeights.into());
    }
    let total: u64 = weights.iter().map(|w| *w as u64).sum();
    if total != BPS_DENOMINATOR {
        return Err(BeandxError::InvalidWeights.into());
    }
    Ok(())
}

/// Checks an asset's account group against what the pool stored at creation.
fn verify_asset(
    asset: &PoolAsset,
    mint: &AccountInfo,
    vault: &AccountInfo,
    token_program: &AccountInfo,
) -> ProgramResult {
    require_key(mint, &asset.mint, BeandxError::InvalidAssetAccount)?;
    require_key(vault, &asset.vault, BeandxError::InvalidAssetAccount)?;
    require_key(token_program, &asset.token_program, BeandxError::InvalidTokenProgram)
}

/// Splits the trailing per-asset accounts into groups of `group_len`.
fn asset_groups<'a, 'info>(
    remaining: &'a [AccountInfo<'info>],
    asset_count: usize,
    group_len: usize,
) -> Result<std::slice::Chunks<'a, AccountInfo<'info>>, ProgramError> {
    if remaining.len() != asset_count * group_len {
        return Err(ProgramError::NotEnoughAccountKeys);
    }
    Ok(remaining.chunks(group_len))
}

#[allow(clippy::too_many_arguments)]
fn transfer<'info>(
    token_program: &AccountInfo<'info>,
    source: &AccountInfo<'info>,
    mint: &AccountInfo<'info>,
    destination: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    amount: u64,
    decimals: u8,
    signer_seeds: &[&[u8]],
) -> ProgramResult {
    let ix = token::transfer_checked(
        token_program.key,
        source.key,
        mint.key,
        destination.key,
        authority.key,
        amount,
        decimals,
    );
    let infos = [source.clone(), mint.clone(), destination.clone(), authority.clone(), token_program.clone()];
    if signer_seeds.is_empty() {
        invoke(&ix, &infos)
    } else {
        invoke_signed(&ix, &infos, &[signer_seeds])
    }
}

/// Creates a PDA account, also working when someone pre-funded the address
/// (which would make a plain `create_account` fail).
fn create_pda_account<'info>(
    payer: &AccountInfo<'info>,
    target: &AccountInfo<'info>,
    system_program: &AccountInfo<'info>,
    space: usize,
    lamports: u64,
    owner: &Pubkey,
    signer_seeds: &[&[u8]],
) -> ProgramResult {
    require_key(system_program, &SYSTEM_PROGRAM_ID, BeandxError::InvalidProgramAccount)?;
    let current = target.lamports();
    if current == 0 {
        return invoke_signed(
            &token::create_account(payer.key, target.key, lamports, space as u64, owner),
            &[payer.clone(), target.clone(), system_program.clone()],
            &[signer_seeds],
        );
    }
    if lamports > current {
        invoke(
            &token::transfer_lamports(payer.key, target.key, lamports - current),
            &[payer.clone(), target.clone(), system_program.clone()],
        )?;
    }
    invoke_signed(
        &token::allocate(target.key, space as u64),
        &[target.clone(), system_program.clone()],
        &[signer_seeds],
    )?;
    invoke_signed(
        &token::assign(target.key, owner),
        &[target.clone(), system_program.clone()],
        &[signer_seeds],
    )
}

fn rent_exempt_lamports(space: usize) -> Result<u64, ProgramError> {
    Ok(Rent::get()?.minimum_balance(space))
}
