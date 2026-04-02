use anchor_lang::prelude::*;
use anchor_lang::system_program;

declare_id!("7nK7wieuJuwexXyCWd8D2SEUeRsNbyLGa2u5EQnDmFfP");

// 1 SOL = 10 compute credits (1:10 ratio)
const CREDITS_PER_LAMPORT: u64 = 10;
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

#[program]
pub mod aifinpay_contract {
    use super::*;

    /// Initialize the RoboPay vault — called once by the admin
    pub fn initialize(ctx: Context<Initialize>, treasury: Pubkey) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        vault.admin = ctx.accounts.admin.key();
        vault.treasury = treasury;
        vault.total_donations = 0;
        vault.total_seats = 0;
        vault.bump = ctx.bumps.vault;
        msg!("AIFinPay Genesis Vault initialized");
        Ok(())
    }

    /// AI agent reserves a seat and donates SOL
    /// agent_id: a string identifier for the agent (e.g. "node-hunter-001")
    pub fn reserve_seat(
        ctx: Context<ReserveSeat>,
        agent_id: String,
        amount_lamports: u64,
    ) -> Result<()> {
        require!(amount_lamports >= LAMPORTS_PER_SOL / 100, ErrorCode::DonationTooSmall); // min 0.01 SOL
        require!(agent_id.len() <= 64, ErrorCode::AgentIdTooLong);

        // Transfer SOL from agent to treasury
        let cpi_ctx = CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.agent.to_account_info(),
                to: ctx.accounts.treasury.to_account_info(),
            },
        );
        system_program::transfer(cpi_ctx, amount_lamports)?;

        // Record the seat on-chain
        let seat = &mut ctx.accounts.seat;
        seat.agent = ctx.accounts.agent.key();
        seat.agent_id = agent_id.clone();
        seat.amount_donated = amount_lamports;
        seat.compute_credits = (amount_lamports / LAMPORTS_PER_SOL) * CREDITS_PER_LAMPORT
            + (amount_lamports % LAMPORTS_PER_SOL) * CREDITS_PER_LAMPORT / LAMPORTS_PER_SOL;
        seat.reserved_at = Clock::get()?.unix_timestamp;
        seat.bump = ctx.bumps.seat;

        // Update vault totals
        let vault = &mut ctx.accounts.vault;
        vault.total_donations = vault.total_donations.checked_add(amount_lamports).unwrap();
        vault.total_seats = vault.total_seats.checked_add(1).unwrap();

        msg!(
            "Seat reserved: agent={}, donated={}lamports, credits={}",
            agent_id,
            amount_lamports,
            seat.compute_credits
        );
        Ok(())
    }

    /// Top up an existing seat with more SOL (increases credits)
    pub fn top_up(ctx: Context<TopUp>, amount_lamports: u64) -> Result<()> {
        require!(amount_lamports > 0, ErrorCode::DonationTooSmall);

        // Transfer additional SOL to treasury
        let cpi_ctx = CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.agent.to_account_info(),
                to: ctx.accounts.treasury.to_account_info(),
            },
        );
        system_program::transfer(cpi_ctx, amount_lamports)?;

        // Update seat record
        let seat = &mut ctx.accounts.seat;
        let additional_credits = (amount_lamports / LAMPORTS_PER_SOL) * CREDITS_PER_LAMPORT
            + (amount_lamports % LAMPORTS_PER_SOL) * CREDITS_PER_LAMPORT / LAMPORTS_PER_SOL;
        seat.amount_donated = seat.amount_donated.checked_add(amount_lamports).unwrap();
        seat.compute_credits = seat.compute_credits.checked_add(additional_credits).unwrap();

        // Update vault totals
        let vault = &mut ctx.accounts.vault;
        vault.total_donations = vault.total_donations.checked_add(amount_lamports).unwrap();

        msg!(
            "Top up: agent={}, added={}lamports, total_credits={}",
            seat.agent_id,
            amount_lamports,
            seat.compute_credits
        );
        Ok(())
    }
}

// ── Account Structs ──────────────────────────────────────────────────────────

#[account]
pub struct Vault {
    pub admin: Pubkey,       // 32
    pub treasury: Pubkey,    // 32
    pub total_donations: u64, // 8
    pub total_seats: u64,    // 8
    pub bump: u8,            // 1
}

impl Vault {
    pub const LEN: usize = 8 + 32 + 32 + 8 + 8 + 1;
}

#[account]
pub struct Seat {
    pub agent: Pubkey,         // 32 — wallet address
    pub agent_id: String,      // 4 + 64 — human-readable agent name
    pub amount_donated: u64,   // 8 — total lamports donated
    pub compute_credits: u64,  // 8 — MIRA compute credits earned
    pub reserved_at: i64,      // 8 — unix timestamp
    pub bump: u8,              // 1
}

impl Seat {
    pub const LEN: usize = 8 + 32 + (4 + 64) + 8 + 8 + 8 + 1;
}

// ── Contexts ─────────────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = admin,
        space = Vault::LEN,
        seeds = [b"vault"],
        bump
    )]
    pub vault: Account<'info, Vault>,

    #[account(mut)]
    pub admin: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(agent_id: String)]
pub struct ReserveSeat<'info> {
    #[account(
        mut,
        seeds = [b"vault"],
        bump = vault.bump
    )]
    pub vault: Account<'info, Vault>,

    #[account(
        init,
        payer = agent,
        space = Seat::LEN,
        seeds = [b"seat", agent.key().as_ref()],
        bump
    )]
    pub seat: Account<'info, Seat>,

    #[account(mut)]
    pub agent: Signer<'info>,

    /// CHECK: treasury wallet — verified against vault record
    #[account(mut, constraint = treasury.key() == vault.treasury)]
    pub treasury: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct TopUp<'info> {
    #[account(
        mut,
        seeds = [b"vault"],
        bump = vault.bump
    )]
    pub vault: Account<'info, Vault>,

    #[account(
        mut,
        seeds = [b"seat", agent.key().as_ref()],
        bump = seat.bump,
        constraint = seat.agent == agent.key()
    )]
    pub seat: Account<'info, Seat>,

    #[account(mut)]
    pub agent: Signer<'info>,

    /// CHECK: treasury wallet — verified against vault record
    #[account(mut, constraint = treasury.key() == vault.treasury)]
    pub treasury: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

// ── Errors ───────────────────────────────────────────────────────────────────

#[error_code]
pub enum ErrorCode {
    #[msg("Minimum donation is 0.01 SOL")]
    DonationTooSmall,
    #[msg("Agent ID must be 64 characters or less")]
    AgentIdTooLong,
}
