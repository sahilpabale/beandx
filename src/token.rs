//! Minimal hand-encoded SPL Token / Token-2022 / ATA / System instructions and
//! account readers. Encodings follow `spl-token-2022-interface` 3.x; pulling the
//! full interface crates in would drag its zk dependencies onto the program.

use solana_program::{
    account_info::AccountInfo,
    instruction::{AccountMeta, Instruction},
    program_error::ProgramError,
    pubkey::Pubkey,
};

use crate::{
    constants::{
        ASSOCIATED_TOKEN_PROGRAM_ID, SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID,
    },
    error::BeandxError,
};

const MINT_BASE_LEN: usize = 82;
const TOKEN_ACCOUNT_BASE_LEN: usize = 165;
const ACCOUNT_TYPE_MINT: u8 = 1;
const TLV_HEADER_LEN: usize = 4;

const EXT_TRANSFER_FEE_CONFIG: u16 = 1;
const EXT_DEFAULT_ACCOUNT_STATE: u16 = 6;
const EXT_NON_TRANSFERABLE: u16 = 9;
const EXT_TRANSFER_HOOK: u16 = 14;
const ACCOUNT_STATE_FROZEN: u8 = 2;

/// Receipt mint with only the MetadataPointer extension, before metadata is written.
pub const RECEIPT_MINT_BASE_LEN: usize = TOKEN_ACCOUNT_BASE_LEN + 1 + TLV_HEADER_LEN + 64;

/// Final receipt mint size once Token-2022 appends the TokenMetadata TLV entry.
pub fn receipt_mint_len(name: &str, symbol: &str, uri: &str) -> usize {
    // update_authority + mint + 3 strings + empty additional_metadata vec
    let metadata_len = 32 + 32 + 4 + name.len() + 4 + symbol.len() + 4 + uri.len() + 4;
    RECEIPT_MINT_BASE_LEN + TLV_HEADER_LEN + metadata_len
}

pub fn is_token_program(key: &Pubkey) -> bool {
    *key == TOKEN_PROGRAM_ID || *key == TOKEN_2022_PROGRAM_ID
}

pub fn get_associated_token_address(wallet: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[wallet.as_ref(), token_program.as_ref(), mint.as_ref()],
        &ASSOCIATED_TOKEN_PROGRAM_ID,
    )
    .0
}

// ---------------------------------------------------------------------------
// Account readers
// ---------------------------------------------------------------------------

pub struct MintInfo {
    pub supply: u64,
    pub decimals: u8,
}

pub fn read_mint(account: &AccountInfo) -> Result<MintInfo, ProgramError> {
    if !is_token_program(account.owner) {
        return Err(BeandxError::InvalidTokenProgram.into());
    }
    let data = account.try_borrow_data()?;
    if data.len() < MINT_BASE_LEN || data[45] != 1 {
        return Err(ProgramError::UninitializedAccount);
    }
    Ok(MintInfo {
        supply: u64::from_le_bytes(data[36..44].try_into().unwrap()),
        decimals: data[44],
    })
}

pub struct TokenAccountInfo {
    pub mint: Pubkey,
    pub owner: Pubkey,
    pub amount: u64,
}

pub fn read_token_account(account: &AccountInfo) -> Result<TokenAccountInfo, ProgramError> {
    if !is_token_program(account.owner) {
        return Err(BeandxError::InvalidTokenProgram.into());
    }
    let data = account.try_borrow_data()?;
    if data.len() < TOKEN_ACCOUNT_BASE_LEN {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(TokenAccountInfo {
        mint: Pubkey::new_from_array(data[0..32].try_into().unwrap()),
        owner: Pubkey::new_from_array(data[32..64].try_into().unwrap()),
        amount: u64::from_le_bytes(data[64..72].try_into().unwrap()),
    })
}

/// Rejects Token-2022 mints whose extensions break plain `TransferChecked`
/// into and out of pool vaults:
/// - transfer fees (vaults would receive less than accounted for)
/// - an active transfer hook (needs extra accounts we don't forward)
/// - non-transferable tokens
/// - new accounts frozen by default (vaults could never receive)
///
/// Issuer controls such as permanent delegate, pausable and freeze authority
/// are accepted; they are inherent to tokenized securities like Backpack's.
pub fn check_mint_extensions(account: &AccountInfo) -> Result<(), ProgramError> {
    if *account.owner != TOKEN_2022_PROGRAM_ID {
        return Ok(());
    }
    let data = account.try_borrow_data()?;
    if data.len() <= TOKEN_ACCOUNT_BASE_LEN {
        return Ok(());
    }
    if data[TOKEN_ACCOUNT_BASE_LEN] != ACCOUNT_TYPE_MINT {
        return Err(ProgramError::InvalidAccountData);
    }

    let mut offset = TOKEN_ACCOUNT_BASE_LEN + 1;
    while offset + TLV_HEADER_LEN <= data.len() {
        let ext_type = u16::from_le_bytes([data[offset], data[offset + 1]]);
        let len = u16::from_le_bytes([data[offset + 2], data[offset + 3]]) as usize;
        let value_start = offset + TLV_HEADER_LEN;
        let value = data
            .get(value_start..value_start + len)
            .ok_or(ProgramError::InvalidAccountData)?;
        match ext_type {
            0 => break,
            EXT_TRANSFER_FEE_CONFIG | EXT_NON_TRANSFERABLE => {
                return Err(BeandxError::UnsupportedMint.into());
            }
            // value = authority (32) || program_id (32); all zeroes means no hook
            EXT_TRANSFER_HOOK if value.len() == 64 && value[32..].iter().any(|b| *b != 0) => {
                return Err(BeandxError::UnsupportedMint.into());
            }
            EXT_DEFAULT_ACCOUNT_STATE if value.first() == Some(&ACCOUNT_STATE_FROZEN) => {
                return Err(BeandxError::UnsupportedMint.into());
            }
            _ => {}
        }
        offset = value_start + len;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Token instructions
// ---------------------------------------------------------------------------

pub fn transfer_checked(
    token_program: &Pubkey,
    source: &Pubkey,
    mint: &Pubkey,
    destination: &Pubkey,
    authority: &Pubkey,
    amount: u64,
    decimals: u8,
) -> Instruction {
    let mut data = vec![12];
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);
    Instruction {
        program_id: *token_program,
        accounts: vec![
            AccountMeta::new(*source, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new(*destination, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data,
    }
}

pub fn mint_to_checked(
    token_program: &Pubkey,
    mint: &Pubkey,
    destination: &Pubkey,
    authority: &Pubkey,
    amount: u64,
    decimals: u8,
) -> Instruction {
    let mut data = vec![14];
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);
    Instruction {
        program_id: *token_program,
        accounts: vec![
            AccountMeta::new(*mint, false),
            AccountMeta::new(*destination, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data,
    }
}

pub fn burn_checked(
    token_program: &Pubkey,
    account: &Pubkey,
    mint: &Pubkey,
    authority: &Pubkey,
    amount: u64,
    decimals: u8,
) -> Instruction {
    let mut data = vec![15];
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);
    Instruction {
        program_id: *token_program,
        accounts: vec![
            AccountMeta::new(*account, false),
            AccountMeta::new(*mint, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data,
    }
}

/// InitializeMint2 with no freeze authority.
pub fn initialize_mint2(
    token_program: &Pubkey,
    mint: &Pubkey,
    mint_authority: &Pubkey,
    decimals: u8,
) -> Instruction {
    let mut data = vec![20, decimals];
    data.extend_from_slice(mint_authority.as_ref());
    data.push(0);
    Instruction {
        program_id: *token_program,
        accounts: vec![AccountMeta::new(*mint, false)],
        data,
    }
}

/// Token-2022 MetadataPointerExtension::Initialize.
pub fn initialize_metadata_pointer(mint: &Pubkey, authority: &Pubkey, metadata: &Pubkey) -> Instruction {
    let mut data = vec![39, 0];
    data.extend_from_slice(authority.as_ref());
    data.extend_from_slice(metadata.as_ref());
    Instruction {
        program_id: TOKEN_2022_PROGRAM_ID,
        accounts: vec![AccountMeta::new(*mint, false)],
        data,
    }
}

/// spl-token-metadata-interface Initialize, for metadata stored on the mint itself.
pub fn initialize_token_metadata(
    mint: &Pubkey,
    update_authority: &Pubkey,
    mint_authority: &Pubkey,
    name: &str,
    symbol: &str,
    uri: &str,
) -> Instruction {
    // sha256("spl_token_metadata_interface:initialize_account")[..8]
    let mut data = vec![210, 225, 30, 162, 88, 184, 77, 141];
    for field in [name, symbol, uri] {
        data.extend_from_slice(&(field.len() as u32).to_le_bytes());
        data.extend_from_slice(field.as_bytes());
    }
    Instruction {
        program_id: TOKEN_2022_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*mint, false),
            AccountMeta::new_readonly(*update_authority, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(*mint_authority, true),
        ],
        data,
    }
}

pub fn create_associated_token_account_idempotent(
    payer: &Pubkey,
    wallet: &Pubkey,
    mint: &Pubkey,
    token_program: &Pubkey,
) -> Instruction {
    Instruction {
        program_id: ASSOCIATED_TOKEN_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(get_associated_token_address(wallet, mint, token_program), false),
            AccountMeta::new_readonly(*wallet, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(*token_program, false),
        ],
        data: vec![1],
    }
}

// ---------------------------------------------------------------------------
// System instructions (bincode: u32 tag, then fields)
// ---------------------------------------------------------------------------

pub fn create_account(from: &Pubkey, to: &Pubkey, lamports: u64, space: u64, owner: &Pubkey) -> Instruction {
    let mut data = 0u32.to_le_bytes().to_vec();
    data.extend_from_slice(&lamports.to_le_bytes());
    data.extend_from_slice(&space.to_le_bytes());
    data.extend_from_slice(owner.as_ref());
    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![AccountMeta::new(*from, true), AccountMeta::new(*to, true)],
        data,
    }
}

pub fn assign(account: &Pubkey, owner: &Pubkey) -> Instruction {
    let mut data = 1u32.to_le_bytes().to_vec();
    data.extend_from_slice(owner.as_ref());
    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![AccountMeta::new(*account, true)],
        data,
    }
}

pub fn transfer_lamports(from: &Pubkey, to: &Pubkey, lamports: u64) -> Instruction {
    let mut data = 2u32.to_le_bytes().to_vec();
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![AccountMeta::new(*from, true), AccountMeta::new(*to, false)],
        data,
    }
}

pub fn allocate(account: &Pubkey, space: u64) -> Instruction {
    let mut data = 8u32.to_le_bytes().to_vec();
    data.extend_from_slice(&space.to_le_bytes());
    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![AccountMeta::new(*account, true)],
        data,
    }
}
