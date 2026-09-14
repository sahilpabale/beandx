use solana_program::{pubkey, pubkey::Pubkey};

pub const CONFIG_SEED: &[u8] = b"config";
pub const POOL_SEED: &[u8] = b"pool";
pub const MINT_SEED: &[u8] = b"mint";

pub const MAX_ADMINS: usize = 10;
/// Keeps deposit/withdraw (4 accounts per asset) well under the 64-account tx limit.
pub const MAX_ASSETS: usize = 10;
pub const MAX_SYMBOL_LEN: usize = 10;
pub const MAX_NAME_LEN: usize = 32;
pub const MAX_URI_LEN: usize = 200;

pub const BPS_DENOMINATOR: u64 = 10_000;
pub const MAX_WITHDRAW_FEE_BPS: u16 = 500;
/// Receipt tokens use USDC's decimals so 1 share == $1 at launch reads naturally.
pub const RECEIPT_DECIMALS: u8 = 6;

pub const TOKEN_PROGRAM_ID: Pubkey = pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
pub const TOKEN_2022_PROGRAM_ID: Pubkey = pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
pub const ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey =
    pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
pub const SYSTEM_PROGRAM_ID: Pubkey = pubkey!("11111111111111111111111111111111");
pub const BPF_LOADER_UPGRADEABLE_ID: Pubkey =
    pubkey!("BPFLoaderUpgradeab1e11111111111111111111111");
