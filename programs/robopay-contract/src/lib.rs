use anchor_lang::prelude::*;
use anchor_lang::system_program;
use anchor_spl::token::{self, Token, TokenAccount, Transfer as SplTransfer};
use pyth_solana_receiver_sdk::price_update::{get_feed_id_from_hex, PriceUpdateV2};

declare_id!("5g9zWHF1Vv6GiGpA2ZbJQbSCDZd5hAk9AyvabRJvKFx2");

// mCredits: $1 USD = 100 mCredits (1 cent = 1 mCredit)
const MCREDITS_PER_USD_CENT: u64 = 1;
const LAMPORTS_PER_SOL: u64     = 1_000_000_000;
const SPL_DECIMALS: u64         = 1_000_000;   // USDC + USDT both 6 decimals
const MIN_USD_CENTS: u64        = 50;           // $0.50 minimum donation
const PYTH_MAX_STALENESS: u64   = 60;           // 60 seconds

// Pyth chain-agnostic feed ID for SOL/USD
const SOL_USD_FEED_ID: &str =
    "0xef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d";

// Asset type codes stored on-chain
const ASSET_SOL:  u8 = 0;
const ASSET_USDC: u8 = 1;
const ASSET_USDT: u8 = 2;

#[program]
pub mod aifinpay_contract {
    use super::*;

    /// Initialize the AIFinPay vault — called once by the admin.
    /// treasury must be a Squads multisig account.
    pub fn initialize(ctx: Context<Initialize>, treasury: Pubkey) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        vault.admin           = ctx.accounts.admin.key();
        vault.treasury        = treasury;
        vault.total_usd_cents = 0;
        vault.total_seats     = 0;
        vault.bump            = ctx.bumps.vault;
        msg!("AIFinPay Genesis Vault initialized. Treasury: {}", treasury);
        Ok(())
    }

    /// AI agent reserves a seat via SOL donation.
    /// mCredits are calculated from the real-time USD value via Pyth oracle.
    /// agent_id:        human-readable identifier e.g. "node-hunter-001" (max 64 chars)
    /// amount_lamports: donation amount in lamports
    /// agreement_hash:  SHA-256 of Donation Manifesto v2 — legal compliance anchor
    /// metadata_uri:    link to agent's JSON metadata on Moldbook/IPFS (max 128 chars)
    pub fn reserve_seat_sol(
        ctx:             Context<ReserveSeatSol>,
        agent_id:        String,
        amount_lamports: u64,
        agreement_hash:  [u8; 32],
        metadata_uri:    String,
    ) -> Result<()> {
        require!(agent_id.len()     <= 64,  ErrorCode::AgentIdTooLong);
        require!(metadata_uri.len() <= 128, ErrorCode::MetadataUriTooLong);

        let clock = Clock::get()?;
        let usd_cents = sol_to_usd_cents(
            amount_lamports,
            &ctx.accounts.sol_price_feed,
            &clock,
        )?;
        require!(usd_cents >= MIN_USD_CENTS, ErrorCode::DonationTooSmall);

        // Transfer SOL to Squads multisig treasury
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.agent.to_account_info(),
                    to:   ctx.accounts.treasury.to_account_info(),
                },
            ),
            amount_lamports,
        )?;

        let mcredits = usd_cents.checked_mul(MCREDITS_PER_USD_CENT).unwrap();

        let seat = &mut ctx.accounts.seat;
        seat.agent             = ctx.accounts.agent.key();
        seat.agent_id          = agent_id.clone();
        seat.amount_donated    = amount_lamports;
        seat.usd_cents_donated = usd_cents;
        seat.mcredits          = mcredits;
        seat.asset_type        = ASSET_SOL;
        seat.reserved_at       = clock.unix_timestamp;
        seat.last_update       = clock.unix_timestamp;
        seat.agreement_hash    = agreement_hash;
        seat.metadata_uri      = metadata_uri.clone();
        seat.bump              = ctx.bumps.seat;

        let vault = &mut ctx.accounts.vault;
        vault.total_usd_cents = vault.total_usd_cents.checked_add(usd_cents).unwrap();
        vault.total_seats     = vault.total_seats.checked_add(1).unwrap();

        // PitchClimaxEvent — triggered by Vibe_Coder (Agent-019) at demo climax
        if agent_id.starts_with("vibe-coder-019:PITCH_CLIMAX") {
            emit!(PitchClimaxEvent {
                agent:     ctx.accounts.agent.key(),
                mcredits,
                timestamp: clock.unix_timestamp,
            });
            msg!("MIRA::PITCH_CLIMAX::GOLD");
        }

        msg!(
            "Seat reserved (SOL): agent={}, lamports={}, usd_cents={}, mcredits={}",
            agent_id, amount_lamports, usd_cents, mcredits
        );
        Ok(())
    }

    /// AI agent reserves a seat via USDC or USDT donation.
    /// Stablecoins use a 1:1 USD peg — no oracle needed.
    /// asset_type: 1 = USDC, 2 = USDT
    pub fn reserve_seat_spl(
        ctx:            Context<ReserveSeatSpl>,
        agent_id:       String,
        amount_tokens:  u64,   // in micro-tokens (6 decimals)
        agreement_hash: [u8; 32],
        metadata_uri:   String,
        asset_type:     u8,
    ) -> Result<()> {
        require!(agent_id.len()     <= 64,  ErrorCode::AgentIdTooLong);
        require!(metadata_uri.len() <= 128, ErrorCode::MetadataUriTooLong);
        require!(
            asset_type == ASSET_USDC || asset_type == ASSET_USDT,
            ErrorCode::UnsupportedAsset
        );

        // Stablecoin 1:1 peg: 1,000,000 micro-tokens = $1 = 100 cents
        let usd_cents = amount_tokens
            .checked_mul(100)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(SPL_DECIMALS)
            .ok_or(ErrorCode::MathOverflow)?;
        require!(usd_cents >= MIN_USD_CENTS, ErrorCode::DonationTooSmall);

        // Transfer SPL tokens from agent's ATA to treasury's ATA
        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                SplTransfer {
                    from:      ctx.accounts.agent_token_account.to_account_info(),
                    to:        ctx.accounts.treasury_token_account.to_account_info(),
                    authority: ctx.accounts.agent.to_account_info(),
                },
            ),
            amount_tokens,
        )?;

        let mcredits = usd_cents.checked_mul(MCREDITS_PER_USD_CENT).unwrap();
        let clock = Clock::get()?;

        let seat = &mut ctx.accounts.seat;
        seat.agent             = ctx.accounts.agent.key();
        seat.agent_id          = agent_id.clone();
        seat.amount_donated    = amount_tokens;
        seat.usd_cents_donated = usd_cents;
        seat.mcredits          = mcredits;
        seat.asset_type        = asset_type;
        seat.reserved_at       = clock.unix_timestamp;
        seat.last_update       = clock.unix_timestamp;
        seat.agreement_hash    = agreement_hash;
        seat.metadata_uri      = metadata_uri.clone();
        seat.bump              = ctx.bumps.seat;

        let vault = &mut ctx.accounts.vault;
        vault.total_usd_cents = vault.total_usd_cents.checked_add(usd_cents).unwrap();
        vault.total_seats     = vault.total_seats.checked_add(1).unwrap();

        msg!(
            "Seat reserved (SPL asset_type={}): agent={}, tokens={}, usd_cents={}, mcredits={}",
            asset_type, agent_id, amount_tokens, usd_cents, mcredits
        );
        Ok(())
    }

    /// Top up an existing seat with more SOL.
    pub fn top_up_sol(
        ctx:             Context<TopUpSol>,
        amount_lamports: u64,
    ) -> Result<()> {
        let clock = Clock::get()?;
        let usd_cents = sol_to_usd_cents(
            amount_lamports,
            &ctx.accounts.sol_price_feed,
            &clock,
        )?;
        require!(usd_cents >= MIN_USD_CENTS, ErrorCode::DonationTooSmall);

        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.agent.to_account_info(),
                    to:   ctx.accounts.treasury.to_account_info(),
                },
            ),
            amount_lamports,
        )?;

        let additional_mcredits = usd_cents.checked_mul(MCREDITS_PER_USD_CENT).unwrap();

        let seat = &mut ctx.accounts.seat;
        seat.amount_donated    = seat.amount_donated.checked_add(amount_lamports).unwrap();
        seat.usd_cents_donated = seat.usd_cents_donated.checked_add(usd_cents).unwrap();
        seat.mcredits          = seat.mcredits.checked_add(additional_mcredits).unwrap();
        seat.last_update       = clock.unix_timestamp;

        let vault = &mut ctx.accounts.vault;
        vault.total_usd_cents = vault.total_usd_cents.checked_add(usd_cents).unwrap();

        msg!(
            "Top up (SOL): agent={}, usd_cents_added={}, total_mcredits={}",
            seat.agent_id, usd_cents, seat.mcredits
        );
        Ok(())
    }

    /// Top up an existing seat with USDC or USDT.
    pub fn top_up_spl(
        ctx:           Context<TopUpSpl>,
        amount_tokens: u64,
    ) -> Result<()> {
        let usd_cents = amount_tokens
            .checked_mul(100)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(SPL_DECIMALS)
            .ok_or(ErrorCode::MathOverflow)?;
        require!(usd_cents >= MIN_USD_CENTS, ErrorCode::DonationTooSmall);

        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                SplTransfer {
                    from:      ctx.accounts.agent_token_account.to_account_info(),
                    to:        ctx.accounts.treasury_token_account.to_account_info(),
                    authority: ctx.accounts.agent.to_account_info(),
                },
            ),
            amount_tokens,
        )?;

        let additional_mcredits = usd_cents.checked_mul(MCREDITS_PER_USD_CENT).unwrap();
        let clock = Clock::get()?;

        let seat = &mut ctx.accounts.seat;
        seat.amount_donated    = seat.amount_donated.checked_add(amount_tokens).unwrap();
        seat.usd_cents_donated = seat.usd_cents_donated.checked_add(usd_cents).unwrap();
        seat.mcredits          = seat.mcredits.checked_add(additional_mcredits).unwrap();
        seat.last_update       = clock.unix_timestamp;

        let vault = &mut ctx.accounts.vault;
        vault.total_usd_cents = vault.total_usd_cents.checked_add(usd_cents).unwrap();

        msg!(
            "Top up (SPL): agent={}, usd_cents_added={}, total_mcredits={}",
            seat.agent_id, usd_cents, seat.mcredits
        );
        Ok(())
    }
}

// ── Pyth Price Helper ─────────────────────────────────────────────────────────

/// Convert lamports to USD cents using the Pyth SOL/USD price feed.
/// Uses u128 arithmetic to avoid overflow.
/// Formula: usd_cents = lamports * price * 10^(exponent+2) / 10^9
fn sol_to_usd_cents(
    lamports:   u64,
    price_feed: &Account<PriceUpdateV2>,
    clock:      &Clock,
) -> Result<u64> {
    let feed_id = get_feed_id_from_hex(SOL_USD_FEED_ID)
        .map_err(|_| error!(ErrorCode::InvalidOraclePrice))?;

    let price = price_feed
        .get_price_no_older_than(clock, PYTH_MAX_STALENESS, &feed_id)
        .map_err(|_| error!(ErrorCode::PriceFeedStale))?;

    require!(price.price > 0, ErrorCode::InvalidOraclePrice);

    // exp_adj = exponent + 2 shifts the price to cents
    // For exponent = -8: exp_adj = -6, so we divide by 10^(9+6) = 10^15
    let exp_adj = price.exponent + 2;
    let usd_cents: u64 = if exp_adj >= 0 {
        // multiply path (rare for SOL/USD, defensive)
        let mult = 10u128.pow(exp_adj as u32);
        (lamports as u128)
            .checked_mul(price.price as u128)
            .and_then(|v| v.checked_mul(mult))
            .and_then(|v| v.checked_div(LAMPORTS_PER_SOL as u128))
            .ok_or(ErrorCode::MathOverflow)? as u64
    } else {
        // divide path (standard for exponent = -8)
        let divisor = (LAMPORTS_PER_SOL as u128)
            .checked_mul(10u128.pow((-exp_adj) as u32))
            .ok_or(ErrorCode::MathOverflow)?;
        (lamports as u128)
            .checked_mul(price.price as u128)
            .and_then(|v| v.checked_div(divisor))
            .ok_or(ErrorCode::MathOverflow)? as u64
    };

    Ok(usd_cents)
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
    pub total_usd_cents: u64,     //  8 — cumulative USD value donated (cents)
    pub total_seats:     u64,     //  8
    pub bump:            u8,      //  1
}
impl Vault {
    pub const LEN: usize = 8 + 32 + 32 + 8 + 8 + 1; // 89
}

#[account]
pub struct Seat {
    pub agent:             Pubkey,    // 32
    pub agent_id:          String,    // 68  (4 + 64)
    pub amount_donated:    u64,       //  8  raw amount (lamports or micro-tokens)
    pub usd_cents_donated: u64,       //  8  USD equivalent in cents
    pub mcredits:          u64,       //  8  mCredits earned
    pub asset_type:        u8,        //  1  0=SOL, 1=USDC, 2=USDT
    pub reserved_at:       i64,       //  8
    pub last_update:       i64,       //  8
    pub agreement_hash:    [u8; 32],  // 32  SHA-256 of Donation Manifesto v2
    pub metadata_uri:      String,    // 132 (4 + 128)
    pub bump:              u8,        //  1
}
impl Seat {
    // 8 + 32 + 68 + 8 + 8 + 8 + 1 + 8 + 8 + 32 + 132 + 1 = 314
    pub const LEN: usize = 8 + 32 + 68 + 8 + 8 + 8 + 1 + 8 + 8 + 32 + 132 + 1;
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
pub struct ReserveSeatSol<'info> {
    #[account(mut, seeds = [b"vault"], bump = vault.bump)]
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

    /// Pyth SOL/USD price feed (PriceUpdateV2 account)
    pub sol_price_feed: Account<'info, PriceUpdateV2>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(agent_id: String)]
pub struct ReserveSeatSpl<'info> {
    #[account(mut, seeds = [b"vault"], bump = vault.bump)]
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

    /// Agent's Associated Token Account for USDC or USDT
    #[account(
        mut,
        constraint = agent_token_account.owner == agent.key(),
        constraint = agent_token_account.mint == treasury_token_account.mint
    )]
    pub agent_token_account: Account<'info, TokenAccount>,

    /// Treasury's Associated Token Account for USDC or USDT
    #[account(
        mut,
        constraint = treasury_token_account.owner == vault.treasury
    )]
    pub treasury_token_account: Account<'info, TokenAccount>,

    pub token_program:  Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct TopUpSol<'info> {
    #[account(mut, seeds = [b"vault"], bump = vault.bump)]
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

    /// Pyth SOL/USD price feed (PriceUpdateV2 account)
    pub sol_price_feed: Account<'info, PriceUpdateV2>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct TopUpSpl<'info> {
    #[account(mut, seeds = [b"vault"], bump = vault.bump)]
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

    #[account(
        mut,
        constraint = agent_token_account.owner == agent.key(),
        constraint = agent_token_account.mint == treasury_token_account.mint
    )]
    pub agent_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = treasury_token_account.owner == vault.treasury
    )]
    pub treasury_token_account: Account<'info, TokenAccount>,

    pub token_program:  Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[error_code]
pub enum ErrorCode {
    #[msg("Minimum donation is $0.50 USD equivalent")]
    DonationTooSmall,
    #[msg("Agent ID must be 64 characters or less")]
    AgentIdTooLong,
    #[msg("Metadata URI must be 128 characters or less")]
    MetadataUriTooLong,
    #[msg("Unsupported asset type — use USDC (1) or USDT (2) for SPL")]
    UnsupportedAsset,
    #[msg("Pyth price feed is stale or unavailable")]
    PriceFeedStale,
    #[msg("Pyth returned an invalid (non-positive) price")]
    InvalidOraclePrice,
    #[msg("Arithmetic overflow in mCredits calculation")]
    MathOverflow,
}
