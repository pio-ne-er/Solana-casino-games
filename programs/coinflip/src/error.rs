use anchor_lang::prelude::*;

#[error_code]
pub enum GameError {
    #[msg("Invalid Player Pool Owner")] // 6000
    InvalidPlayerPool,
    #[msg("Invalid Admin to Withdraw")] // 6001
    InvalidAdmin,
    #[msg("Invalid Claim to Withdraw Reward")] // 6002
    InvalidClaim,
    #[msg("Invalid Reward Vault to receive")] // 6003
    InvalidRewardVault,
    #[msg("Insufficient Reward SOL Balance")] // 6004
    InsufficientRewardVault,
    #[msg("Insufficient User SOL Balance")] // 6005
    InsufficientUserBalance,
}
