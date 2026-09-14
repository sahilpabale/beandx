use solana_program::program_error::ProgramError;

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeandxError {
    Unauthorized = 0,
    AlreadyInitialized,
    InvalidPda,
    InvalidProgramAccount,
    AdminListFull,
    AdminExists,
    AdminNotFound,
    FeeTooHigh,
    InvalidSymbol,
    InvalidName,
    InvalidUri,
    InvalidAssetCount,
    DuplicateAsset,
    InvalidWeights,
    UnsupportedMint,
    InvalidTokenProgram,
    InvalidAssetAccount,
    InvalidTreasuryAccount,
    PoolPaused,
    PoolNotSeeded,
    PoolAlreadySeeded,
    ZeroAmount,
    SlippageExceeded,
    MathOverflow,
    InvalidAssetIndex,
}

impl From<BeandxError> for ProgramError {
    fn from(e: BeandxError) -> Self {
        ProgramError::Custom(e as u32)
    }
}
