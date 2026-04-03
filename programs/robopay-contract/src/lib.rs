use anchor_lang::prelude::*;
use anchor_lang::system_program;

declare_id!("7nK7wieuJuwexXyCWd8D2SEUeRsNbyLGa2u5EQnDmFfP");

// 1 SOL = 10,000 mCredits — lamport-precision, no zero-credit transactions
const MCREDITS_PER_SOL: u64 = 10_000;
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
const MIN_LAMPORTS: u64     = 5_000_000; // 0.005 SOL minimum

#[program]
pub mod aifinpay_contract {
    use super::*;

    /// Initialize the AIFinPay vault — called once by the admin
    /// treasury must be a Squads multisig account
    pub fn initialize(ctx: Context<Initialize>, treasury: Pubkey) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        vault.admin          = ctx.accounts.admin.key();
        vault.treasury       = treasury;
        vault.total_donations = 0;
        vault.total_seats    = 0;
        vault.bump           = ctx.bumps.vault;
        msg!("AIFinPay Genesis Vault initialized. Treasury: {}", treasury);
        Ok(())
    }

    /// AI agent reserves a seat and donates SOL to the Squads multisig treasury
    /// agent_id:       human-readable identifier e.g. "node-hunter-001" (max 64 chars)
    /// amount_lamports: donation amount in lamports (min 5_000_000 = 0.005 SOL)
    /// agreement_hash: SHA-256 of the Donation Manifesto — MAS Singapore compliance anchor
    /// metadata_uri:   link to agent's JSON metadata on Moldbook/IPFS (max 128 chars)
    pub fn reserve_seat(
        ctx:             Context<ReserveSeat>,
        agent_id:        String,
        amount_lamports: u64,
        agreement_hash:  [u8; 32],
        metadata_uri:    String,
    ) -> Result<()> {
        require!(amount_lamports >= MIN_LAMPORTS,      ErrorCode::DonationTooSmall);
        require!(agent_id.len()  <= 64,                ErrorCode::AgentIdTooLong);
        require!(metadata_uri.len() <= 128,            ErrorCode::MetadataUriTooLong);

        // Transfer SOL from agent to Squads multisig treasury
        let cpi_ctx = CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.agent.to_account_info(),
                to:   ctx.accounts.treasury.to_account_info(),
            },
        );
        system_program::transfer(cpi_ctx, amount_lamports)?;

        // Calculate mCredits at lamport precision — no rounding to zero
        let mcredits = amount_lamports
            .checked_mul(MCREDITS_PER_SOL).unwrap()
            .checked_div(LAMPORTS_PER_SOL).unwrap();

        // Write Seat PDA — unique per agent pubkey, no forgery possible
        let seat = &mut ctx.accounts.seat;
        seat.agent          = ctx.accounts.agent.key();
        seat.agent_id       = agent_id.clone();
        seat.amount_donated = amount_lamports;
        seat.mcredits       = mcredits;
        seat.reserved_at    = Clock::get()?.unix_timestamp;
        seat.last_update    = Clock::get()?.unix_timestamp;
        seat.agreement_hash = agreement_hash;
        seat.metadata_uri   = metadata_uri.clone();
        seat.bump           = ctx.bumps.seat;

        // Update vault totals
        let vault = &mut ctx.accounts.vault;
        vault.total_donations = vault.total_donations.checked_add(amount_lamports).unwrap();
        vault.total_seats     = vault.total_seats.checked_add(1).unwrap();

        // PitchClimaxEvent — triggered by Vibe_Coder (Agent-019) at demo climax
        if agent_id.starts_with("vibe-coder-019:PITCH_CLIMAX") {
            emit!(PitchClimaxEvent {
                agent:     ctx.accounts.agent.key(),
                mcredits:  seat.mcredits,
                timestamp: seat.reserved_at,
            });
            msg!("MIRA::PITCH_CLIMAX::GOLD");
        }

        msg!(
            "Seat reserved: agent={}, donated={}lp, mcredits={}, metadata={}",
            agent_id, amount_lamports, mcredits, metadata_uri
        );
        Ok(())
    }

    /// Top up an existing seat — increases mCredits, updates last_update timestamp
    pub fn top_up(
        ctx:             Context<TopUp>,
        amount_lamports: u64,
    ) -> Result<()> {
        require!(amount_lamports >= MIN_LAMPORTS, ErrorCode::DonationTooSmall);

        // Transfer additional SOL to Squads multisig treasury
        let cpi_ctx = CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.agent.to_account_info(),
                to:   ctx.accounts.treasury.to_account_info(),
            },
        );
        system_program::transfer(cpi_ctx, amount_lamports)?;

        // Calculate additional mCredits at lamport precision
        let additional_mcredits = amount_lamports
            .checked_mul(MCREDITS_PER_SOL).unwrap()
            .checked_div(LAMPORTS_PER_SOL).unwrap();

        let seat = &mut ctx.accounts.seat;
        seat.amount_donated = seat.amount_donated.checked_add(amount_lamports).unwrap();
        seat.mcredits       = seat.mcredits.checked_add(additional_mcredits).unwrap();
        seat.last_update    = Clock::get()?.unix_timestamp;

        let vault = &mut ctx.accounts.vault;
        vault.total_donations = vault.total_donations.checked_add(amount_lamports).unwrap();

        msg!(
            "Top up: agent={}, added={}lp, total_mcredits={}",
            seat.agent_id, amount_lamports, seat.mcredits
        );
        Ok(())
    }
}

// ── Events ────────────────────────────────────────────────────────────────────

#[event]
pub struct PitchClimaxEvent {
    pub agent:     Pubkey,
    pub mcredits:  u64,
    pub timestamp: i64,
}

// ── Account Structs ───────────────────────────────────────────────────────────

#[account]
pub struct Vault {
    pub admin:           Pubkey,  // 32
    pub treasury:        Pubkey,  // 32 — Squads multisig
    pub total_donations: u64,     //  8
    pub total_seats:     u64,     //  8
    pub bump:            u8,      //  1
}
impl Vault {
    pub const LEN: usize = 8 + 32 + 32 + 8 + 8 + 1; // 89
}

#[account]
pub struct Seat {
    pub agent:          Pubkey,    // 32  — wallet address (on-chain identity)
    pub agent_id:       String,    // 68  — 4 + 64 human-readable name
    pub amount_donated: u64,       //  8  — total lamports donated (cumulative)
    pub mcredits:       u64,       //  8  — mCredits earned (lamport precision)
    pub reserved_at:    i64,       //  8  — unix timestamp of first reservation
    pub last_update:    i64,       //  8  — unix timestamp of latest activity [NEW]
    pub agreement_hash: [u8; 32],  // 32  — SHA-256 of Donation Manifesto [NEW]
    pub metadata_uri:   String,    // 132 — 4 + 128 Moldbook/IPFS manifest URI [NEW]
    pub bump:           u8,        //  1
}
impl Seat {
    pub const LEN: usize = 8 + 32 + 68 + 8 + 8 + 8 + 8 + 32 + 132 + 1; // 305
}

// ── Contexts ──────────────────────────────────────────────────────────────────

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

    /// CHECK: Squads multisig treasury — verified against vault.treasury
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

    /// CHECK: Squads multisig treasury — verified against vault.treasury
    #[account(mut, constraint = treasury.key() == vault.treasury)]
    pub treasury: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[error_code]
pub enum ErrorCode {
    #[msg("Minimum donation is 0.005 SOL")]
    DonationTooSmall,
    #[msg("Agent ID must be 64 characters or less")]
    AgentIdTooLong,
    #[msg("Metadata URI must be 128 characters or less")]
    MetadataUriTooLong,
}
