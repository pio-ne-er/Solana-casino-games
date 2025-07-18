use anchor_lang::{prelude::*, AnchorDeserialize};

use solana_program::{
    pubkey::Pubkey,
    sysvar,
    sysvar::{instructions::load_current_index_checked},
};

pub mod account;
pub mod constants;
pub mod error;
pub mod utils;

use account::*;
use constants::*;
use error::*;
use utils::*;

declare_id!("3Sh6Wrj72E7CUJ9EV5g4KPLEo69sxDs6gPZXZd2Tawx1");

#[program]
pub mod coinflip {
    use solana_program::native_token::LAMPORTS_PER_SOL;
    use super::*;
    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let global_authority = &mut ctx.accounts.global_authority;

        sol_transfer_user(
            ctx.accounts.admin.to_account_info().clone(),
            ctx.accounts.reward_vault.to_account_info().clone(),
            ctx.accounts.system_program.to_account_info().clone(),
            ctx.accounts.rent.minimum_balance(0),
        )?;

        global_authority.super_admin = ctx.accounts.admin.key();
        global_authority.loyalty_wallet = LOYALTY_WALLET.parse::<Pubkey>().unwrap();
        global_authority.loyalty_fee = LOYALTY_FEE;
        
        Ok(())
    }

    pub fn initialize_player_pool(ctx: Context<InitializePlayerPool>) -> Result<()> {
        let mut player_pool = ctx.accounts.player_pool.load_init()?;
        player_pool.player = ctx.accounts.owner.key();
        msg!("Owner: {:?}", player_pool.player.to_string());

        Ok(())
    }

    pub fn update(ctx: Context<Update>, loyalty_fee: u64) -> Result<()> {
        let global_authority = &mut ctx.accounts.global_authority;

        require!(
            ctx.accounts.admin.key() == global_authority.super_admin,
            GameError::InvalidAdmin
        );

        global_authority.loyalty_wallet = ctx.accounts.loyalty_wallet.key();
        global_authority.loyalty_fee = loyalty_fee;
        Ok(())
    }

    /**
    The main function to play dice.
    Input Args:
    set_number: The number is set by a player to play : 0: Tail, 1: Head
    deposit:    The SOL amount to deposit
    */
    #[access_control(user(&ctx.accounts.player_pool, &ctx.accounts.owner))]
    pub fn play_game(
        ctx: Context<PlayRound>,
        set_number: u64,
        deposit: u64,
    ) -> Result<()> {
        let mut player_pool = ctx.accounts.player_pool.load_mut()?;
        let global_authority = &mut ctx.accounts.global_authority;

        msg!("Deposit: {}", deposit);
        msg!(
            "Vault: {}",
            ctx.accounts.reward_vault.to_account_info().key()
        );
        msg!(
            "Lamports: {}",
            ctx.accounts.reward_vault.to_account_info().lamports()
        );
        msg!(
            "Owner Lamports: {}",
            ctx.accounts.owner.to_account_info().lamports()
        );
        require!(
            ctx.accounts.owner.to_account_info().lamports() > deposit,
            GameError::InsufficientUserBalance
        );

        require!(
            ctx.accounts.reward_vault.to_account_info().lamports() > 2 * deposit,
            GameError::InsufficientRewardVault
        );

        require!(
            ctx.accounts.loyalty_wallet.to_account_info().key() == global_authority.loyalty_wallet,
            GameError::InvalidRewardVault
        );
        
        // 3% of deposit Sol
        let fee_price = deposit * global_authority.loyalty_fee / PERMILLE;
        
        // Transfer deposit Sol to this PDA
        sol_transfer_user(
            ctx.accounts.owner.to_account_info(),
            ctx.accounts.reward_vault.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
            deposit - fee_price,
        )?;

        // Transfer SOL to the loyalty_wallet
        sol_transfer_user(
            ctx.accounts.owner.to_account_info(),
            ctx.accounts.loyalty_wallet.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
            fee_price,
        )?;
    
        // Generate random number
        let mut reward: u64 = 0;
        let timestamp = Clock::get()?.unix_timestamp;
        let slot = Clock::get()?.slot;
        msg!("Slot number: {}", slot);

        // Compare random number and set_number
        // 0.05, 0.1 Sol: 47.5%
        if slot as u64 % 2 == set_number && (deposit == BET_AMOUNT_FIRST || deposit == BET_AMOUNT_SECOND) && (slot as u64 / 2) % 20 != 9 {
            reward = 2 * deposit;
        }

        // 0.25, 0.5 Sol: 40%
        if slot as u64 % 2 == set_number && (deposit == BET_AMOUNT_THIRD || deposit == BET_AMOUNT_FOURTH) && (slot as u64 / 2) % 5 != 4 {
            reward = 2 * deposit;
        }

        // 1 Sol: 25%
        if slot as u64 % 2 == set_number && (deposit == BET_AMOUNT_FIFTH) && (slot as u64 / 2) % 2 == set_number {
            reward = 2 * deposit;
        }

        // 2 Sol: 16.67%
        if slot as u64 % 2 == set_number && (deposit == BET_AMOUNT_SIXTH) && (slot as u64 / 2) % 3 == set_number {
            reward = 2 * deposit;
        }

        msg!("Chosen Number: {}", set_number);
        msg!("Reward: {}", reward);
        msg!("CF Version: {}", CF_VERSION);

        // Add game data to the blockchain
        player_pool.add_game_data(timestamp, deposit, reward, set_number, slot);

        global_authority.total_round += 1;

        Ok(())
    }

    /**
    The claim Reward function after playing
    */
    #[access_control(user(&ctx.accounts.player_pool, &ctx.accounts.owner))]
    pub fn claim_reward(ctx: Context<ClaimReward>) -> Result<()>
    {
        let instruction_sysvar_account = ctx.accounts.instruction_sysvar_account.to_account_info();
        let index = load_current_index_checked(&instruction_sysvar_account).unwrap();
        require!(index == 0, GameError::InvalidClaim);

        let _vault_bump = ctx.bumps.reward_vault;

        let mut player_pool = ctx.accounts.player_pool.load_mut()?;
        let reward = player_pool.claimable_reward;
        require!(
            ctx.accounts.reward_vault.to_account_info().lamports() > reward,
            GameError::InsufficientRewardVault
        );

        msg!("Withdrawer: {}", ctx.accounts.owner.to_account_info().key());
        msg!("Asking to Withdraw: {} SOL", player_pool.claimable_reward as u64 / LAMPORTS_PER_SOL);
        msg!("Current Game Reward: {} SOL", player_pool.game_data.reward_amount as u64 / LAMPORTS_PER_SOL);
        //msg!(player_pool.game_data.into());

        if reward > 0
        {
            // Transfer SOL to the winner from the PDA
            sol_transfer_with_signer(
                ctx.accounts.reward_vault.to_account_info(),
                ctx.accounts.owner.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
                &[&[VAULT_AUTHORITY_SEED.as_ref(), &[_vault_bump]]],
                reward,
            )?;
            player_pool.game_data.reward_amount = 0;
            player_pool.claimable_reward = 0;
        }
        Ok(())
    }

    /**
    Withdraw function to withdraw SOL from the PDA with amount
    Args:
    amount: The sol amount to withdraw from this PDA
    Only Admin can withdraw SOL from this PDA
    */
    pub fn withdraw(
        ctx: Context<Withdraw>,
        amount: u64,
    ) -> Result<()> {
        let global_authority = &mut ctx.accounts.global_authority;
        require!(
            ctx.accounts.admin.key() == global_authority.super_admin 
             || ctx.accounts.admin.key() == global_authority.loyalty_wallet, 
            GameError::InvalidAdmin
        );
                
        let _vault_bump = ctx.bumps.reward_vault;

        sol_transfer_with_signer(
            ctx.accounts.reward_vault.to_account_info(),
            ctx.accounts.admin.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
            &[&[VAULT_AUTHORITY_SEED.as_ref(), &[_vault_bump]]],
            amount,
        )?;
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        init,
        space = 88,
        seeds = [GLOBAL_AUTHORITY_SEED.as_ref()],
        bump,
        payer = admin
    )]
    pub global_authority: Box<Account<'info, GlobalPool>>,

    #[account(
        mut,
        seeds = [VAULT_AUTHORITY_SEED.as_ref()],
        bump,
    )]
    /// CHECK: This is not dangerous because we don't read or write from this account
    pub reward_vault: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct InitializePlayerPool<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,

    #[account(zero)]
    pub player_pool: AccountLoader<'info, PlayerPool>,
}


#[derive(Accounts)]
pub struct Update<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [GLOBAL_AUTHORITY_SEED.as_ref()],
        bump,
    )]
    pub global_authority: Box<Account<'info, GlobalPool>>,

    #[account(mut)]
    /// CHECK: This is not dangerous because we don't read or write from this account
    pub loyalty_wallet: AccountInfo<'info>,
}

#[derive(Accounts)]
pub struct PlayRound<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,

    #[account(mut)]
    pub player_pool: AccountLoader<'info, PlayerPool>,

    #[account(
        mut,
        seeds = [GLOBAL_AUTHORITY_SEED.as_ref()],
        bump,
    )]
    pub global_authority: Box<Account<'info, GlobalPool>>,

    #[account(
        mut,
        seeds = [VAULT_AUTHORITY_SEED.as_ref()],
        bump,
    )]
    /// CHECK: This is not dangerous because we don't read or write from this account
    pub reward_vault: AccountInfo<'info>,

    #[account(mut)]
    pub loyalty_wallet: SystemAccount<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ClaimReward<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,

    #[account(mut)]
    pub player_pool: AccountLoader<'info, PlayerPool>,

    #[account(
        mut,
        seeds = [GLOBAL_AUTHORITY_SEED.as_ref()],
        bump,
    )]
    pub global_authority: Box<Account<'info, GlobalPool>>,

    #[account(
        mut,
        seeds = [VAULT_AUTHORITY_SEED.as_ref()],
        bump,
    )]
    /// CHECK: This is not dangerous because we don't read or write from this account
    pub reward_vault: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
    
    /// CHECK: instruction_sysvar_account cross checking 
    #[account(address = sysvar::instructions::ID)]
    instruction_sysvar_account: AccountInfo<'info>,
}

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [GLOBAL_AUTHORITY_SEED.as_ref()],
        bump,
    )]
    pub global_authority: Box<Account<'info, GlobalPool>>,

    #[account(
        mut,
        seeds = [VAULT_AUTHORITY_SEED.as_ref()],
        bump,
    )]
    /// CHECK: This is not dangerous because we don't read or write from this account
    pub reward_vault: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}
// Access control modifiers
fn user(pool_loader: &AccountLoader<PlayerPool>, user: &AccountInfo) -> Result<()> {
    let user_pool = pool_loader.load()?;
    require!(user_pool.player == *user.key, GameError::InvalidPlayerPool);
    Ok(())
}