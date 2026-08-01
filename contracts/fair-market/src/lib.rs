// Wick Fair Market — 5-minute binary price prediction market on Stellar/Soroban.
//
// Testnet oracle  (ReflectorPulse, USD-base, free, 5-min, BTC/ETH/SOL):
//   CCYOZJCOPG34LLQQ7N24YXBM7LL62R7ONMZ3G6WZAAYPB5OYKOMJRN63
//   decimals: 14   base: USD
//
// XLM SAC — TODO: run at deploy time:
//   stellar contract id asset --asset native --network testnet
//
// Assets: BTC, ETH, SOL.  XLM excluded in v1 (thin liquidity = manipulable).
//
// Round lifecycle:
//   create_round(asset)  →  calls lastprice(), anchors strike to oracle ts
//   bet(...)             →  accepted until lock_ts = strike_ts + lock_offset
//   settle(round_id)     →  calls price(asset, settle_ts), deterministic
//
// Storage:
//   Round / Position  →  TEMPORARY  (7-day TTL, bumped on write, expire after claim)
//   Config / Counters →  PERSISTENT (30-day TTL, bumped on admin ops)
//
// Unclaimed winnings expire after 7 days. Surface countdown in the UI.

#![no_std]

mod oracle;
mod types;

#[cfg(test)]
mod test;

use oracle::{OracleAsset, OracleClient};
use types::{Config, Outcome, Position, Round, Side, Status};

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short,
    token, Address, Env, Symbol, Vec,
};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Seconds between oracle updates. Reflector ReflectorPulse = 5 min.
const ORACLE_INTERVAL: u64 = 300;

/// Maximum fee the admin can set (5 %).
const MAX_FEE_BPS: u32 = 500;

/// Hard minimum for lock_offset. Must stay well below ORACLE_INTERVAL.
const MIN_LOCK_OFFSET: u64 = 90;

/// Maximum rounds in a single claim_many call.
const MAX_CLAIM_BATCH: usize = 20;

// Temporary storage TTL (rounds + positions).
const TEMP_BUMP: u32 = 7 * 24 * 3600 / 5; // 120_960 ledgers (7 days)
const TEMP_THRESHOLD: u32 = 3 * 24 * 3600 / 5; // 51_840 (3 days)

// Persistent storage TTL (config + counters).
const INST_BUMP: u32 = 30 * 24 * 3600 / 5; // 518_400 (30 days)
const INST_THRESHOLD: u32 = 15 * 24 * 3600 / 5; // 259_200 (15 days)

// ── Storage keys ─────────────────────────────────────────────────────────────

#[contracttype]
enum Key {
    Config,
    Counter,              // u64 — next round id
    Fees,                 // i128 — accumulated treasury fees
    Round(u64),
    Position(u64, Address),
    AssetStrikeTs(Symbol), // last strike_ts per asset — dedup guard
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    InvalidAsset = 3,
    RoundNotFound = 4,
    RoundLocked = 5,
    RoundNotSettled = 6,
    AlreadySettled = 7,
    TooEarly = 8,
    BetTooSmall = 9,
    AlreadyBet = 10,
    AlreadyClaimed = 11,
    NothingToClaim = 12,
    FeeTooHigh = 13,
    LockOffsetTooSmall = 14,
    Unauthorized = 15,
    DuplicateRound = 16,
    ClaimBatchTooLarge = 17,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct FairMarket;

#[contractimpl]
impl FairMarket {
    // ── Initialisation ─────────────────────────────────────────────────────

    pub fn initialize(
        e: Env,
        admin: Address,
        oracle: Address,
        token: Address,
        fee_bps: u32,
        min_bet: i128,
        lock_offset: u64,
    ) {
        if e.storage().instance().has(&Key::Config) {
            panic_with_error!(&e, Error::AlreadyInitialized);
        }
        admin.require_auth();

        if fee_bps > MAX_FEE_BPS {
            panic_with_error!(&e, Error::FeeTooHigh);
        }
        if lock_offset < MIN_LOCK_OFFSET {
            panic_with_error!(&e, Error::LockOffsetTooSmall);
        }

        // Fetch oracle decimal precision once at init; store for UI use.
        let oracle_decimals = OracleClient::new(&e, &oracle).decimals();

        let config = Config { admin, oracle, token, fee_bps, min_bet, lock_offset, oracle_decimals };
        e.storage().instance().set(&Key::Config, &config);
        e.storage().instance().set(&Key::Counter, &0u64);
        e.storage().instance().set(&Key::Fees, &0i128);
        bump_instance(&e);
    }

    // ── Round creation (permissionless) ────────────────────────────────────
    //
    // Calls lastprice() to get the current oracle tick (P_N).
    // The round's strike is anchored to the oracle's own timestamp — not ledger
    // time — so round boundaries align to the oracle feed even if the keeper fires late.

    pub fn create_round(e: Env, asset: Symbol) -> u64 {
        let config = require_config(&e);
        require_valid_asset(&e, &asset);

        let oracle = OracleClient::new(&e, &config.oracle);
        let tick = oracle
            .lastprice(&OracleAsset::Other(asset.clone()))
            .unwrap_or_else(|| panic_with_error!(&e, Error::RoundNotFound));

        let strike_ts = tick.timestamp;
        let settle_ts = strike_ts + ORACLE_INTERVAL;
        let lock_ts = strike_ts + config.lock_offset;

        // Dedup: reject if we already have a round for this oracle tick.
        let last_strike_ts: u64 = e
            .storage()
            .instance()
            .get(&Key::AssetStrikeTs(asset.clone()))
            .unwrap_or(0);
        if last_strike_ts == strike_ts {
            panic_with_error!(&e, Error::DuplicateRound);
        }

        let id = next_round_id(&e);
        let round = Round {
            id,
            asset: asset.clone(),
            strike: tick.price,
            strike_ts,
            settle_ts,
            lock_ts,
            pool_up: 0,
            pool_down: 0,
            status: Status::Open,
            outcome: Outcome::Void,
            settle_price: 0,
        };
        save_round(&e, &round);

        e.storage().instance().set(&Key::AssetStrikeTs(asset.clone()), &strike_ts);
        bump_instance(&e);

        e.events().publish(
            (symbol_short!("created"), asset),
            (id, tick.price, strike_ts, settle_ts, lock_ts),
        );
        id
    }

    // ── Betting ────────────────────────────────────────────────────────────

    pub fn bet(e: Env, user: Address, round_id: u64, side: Side, amount: i128) {
        user.require_auth();
        let config = require_config(&e);

        if amount < config.min_bet {
            panic_with_error!(&e, Error::BetTooSmall);
        }

        let mut round = load_round(&e, round_id);
        let now = e.ledger().timestamp();

        if now >= round.lock_ts {
            panic_with_error!(&e, Error::RoundLocked);
        }
        if round.status != Status::Open {
            panic_with_error!(&e, Error::RoundLocked);
        }

        if e.storage().temporary().has(&Key::Position(round_id, user.clone())) {
            panic_with_error!(&e, Error::AlreadyBet);
        }

        // Transfer XLM from user to contract.
        token::TokenClient::new(&e, &config.token).transfer(
            &user,
            &e.current_contract_address(),
            &amount,
        );

        match side {
            Side::Up => {
                round.pool_up = round.pool_up.checked_add(amount).expect("pool overflow");
            }
            Side::Down => {
                round.pool_down = round.pool_down.checked_add(amount).expect("pool overflow");
            }
        }
        save_round(&e, &round);

        let pos = Position { round_id, side, amount, claimed: false };
        save_position(&e, round_id, &user, &pos);

        e.events().publish(
            (symbol_short!("bet"), round_id),
            (user, side, amount, round.pool_up, round.pool_down),
        );
        bump_instance(&e);
    }

    // ── Settlement (permissionless) ────────────────────────────────────────
    //
    // Reads oracle.price(asset, settle_ts) — pinned to a specific timestamp.
    // Outcome is identical regardless of who calls settle() or when they call it.
    // This is the core security property: the settler cannot pick a favourable price.

    pub fn settle(e: Env, round_id: u64) {
        let config = require_config(&e);
        let mut round = load_round(&e, round_id);

        if round.status == Status::Settled {
            panic_with_error!(&e, Error::AlreadySettled);
        }
        if e.ledger().timestamp() < round.settle_ts {
            panic_with_error!(&e, Error::TooEarly);
        }

        let oracle = OracleClient::new(&e, &config.oracle);

        // Pinned read — never lastprice().
        let settle_price_opt =
            oracle.price(&OracleAsset::Other(round.asset.clone()), &round.settle_ts);

        let outcome = match settle_price_opt {
            None => Outcome::Void, // oracle gap
            Some(p) => {
                let sp = p.price;
                round.settle_price = sp;
                if sp > round.strike {
                    Outcome::Up
                } else if sp < round.strike {
                    Outcome::Down
                } else {
                    Outcome::Void // exact tie — refund, never pick a winner arbitrarily
                }
            }
        };

        // Void when either pool is empty — no counterparty, no real market.
        let outcome = if round.pool_up == 0 || round.pool_down == 0 {
            Outcome::Void
        } else {
            outcome
        };

        // Fee is only collected on non-void rounds.
        if outcome != Outcome::Void {
            let total = round.pool_up + round.pool_down;
            let fee = total
                .checked_mul(config.fee_bps as i128)
                .expect("fee mul overflow")
                .checked_div(10_000)
                .expect("fee div");
            accumulate_fee(&e, fee);
        }

        round.outcome = outcome;
        round.status = Status::Settled;
        save_round(&e, &round);

        e.events().publish(
            (symbol_short!("settled"), round_id),
            (outcome, round.settle_price),
        );
        bump_instance(&e);
    }

    // ── Claiming ───────────────────────────────────────────────────────────

    pub fn claim(e: Env, user: Address, round_id: u64) -> i128 {
        user.require_auth();
        let config = require_config(&e);
        let payout = compute_claim(&e, &config, round_id, &user);
        if payout > 0 {
            token::TokenClient::new(&e, &config.token).transfer(
                &e.current_contract_address(),
                &user,
                &payout,
            );
            e.events().publish((symbol_short!("claim"), round_id), (user, payout));
        }
        bump_instance(&e);
        payout
    }

    /// Batch claim, capped at MAX_CLAIM_BATCH rounds per call.
    pub fn claim_many(e: Env, user: Address, round_ids: Vec<u64>) -> i128 {
        user.require_auth();
        let config = require_config(&e);

        if round_ids.len() > MAX_CLAIM_BATCH as u32 {
            panic_with_error!(&e, Error::ClaimBatchTooLarge);
        }

        let mut total: i128 = 0;
        for id in round_ids.iter() {
            total = total
                .checked_add(compute_claim(&e, &config, id, &user))
                .expect("claim sum overflow");
        }

        if total > 0 {
            token::TokenClient::new(&e, &config.token).transfer(
                &e.current_contract_address(),
                &user,
                &total,
            );
            e.events().publish((symbol_short!("claims"), round_ids.len()), (user, total));
        }
        bump_instance(&e);
        total
    }

    // ── View functions ─────────────────────────────────────────────────────

    pub fn get_round(e: Env, round_id: u64) -> Round {
        let mut round = load_round(&e, round_id);
        // Derive Locked status from time without requiring an extra tx.
        if round.status == Status::Open && e.ledger().timestamp() >= round.lock_ts {
            round.status = Status::Locked;
        }
        round
    }

    pub fn get_position(e: Env, round_id: u64, user: Address) -> Option<Position> {
        e.storage().temporary().get(&Key::Position(round_id, user))
    }

    pub fn get_config(e: Env) -> Config {
        require_config(&e)
    }

    // ── Admin ──────────────────────────────────────────────────────────────

    pub fn set_fee_bps(e: Env, bps: u32) {
        let mut config = require_config(&e);
        config.admin.require_auth();
        if bps > MAX_FEE_BPS {
            panic_with_error!(&e, Error::FeeTooHigh);
        }
        config.fee_bps = bps;
        e.storage().instance().set(&Key::Config, &config);
        bump_instance(&e);
    }

    pub fn set_lock_offset(e: Env, seconds: u64) {
        let mut config = require_config(&e);
        config.admin.require_auth();
        if seconds < MIN_LOCK_OFFSET {
            panic_with_error!(&e, Error::LockOffsetTooSmall);
        }
        config.lock_offset = seconds;
        e.storage().instance().set(&Key::Config, &config);
        bump_instance(&e);
    }

    /// Sweep accumulated treasury fees to the admin address.
    pub fn sweep_fees(e: Env) {
        let config = require_config(&e);
        config.admin.require_auth();
        let fees: i128 = e.storage().instance().get(&Key::Fees).unwrap_or(0);
        if fees > 0 {
            token::TokenClient::new(&e, &config.token).transfer(
                &e.current_contract_address(),
                &config.admin,
                &fees,
            );
            e.storage().instance().set(&Key::Fees, &0i128);
        }
        bump_instance(&e);
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn require_config(e: &Env) -> Config {
    if !e.storage().instance().has(&Key::Config) {
        panic_with_error!(e, Error::NotInitialized);
    }
    e.storage().instance().get(&Key::Config).unwrap()
}

fn require_valid_asset(e: &Env, asset: &Symbol) {
    let btc = Symbol::new(e, "BTC");
    let eth = Symbol::new(e, "ETH");
    let sol = Symbol::new(e, "SOL");
    if *asset != btc && *asset != eth && *asset != sol {
        panic_with_error!(e, Error::InvalidAsset);
    }
}

fn next_round_id(e: &Env) -> u64 {
    let id: u64 = e.storage().instance().get(&Key::Counter).unwrap_or(0);
    let next = id + 1;
    e.storage().instance().set(&Key::Counter, &next);
    next
}

fn save_round(e: &Env, round: &Round) {
    e.storage().temporary().set(&Key::Round(round.id), round);
    e.storage().temporary().extend_ttl(&Key::Round(round.id), TEMP_THRESHOLD, TEMP_BUMP);
}

fn load_round(e: &Env, round_id: u64) -> Round {
    e.storage()
        .temporary()
        .get(&Key::Round(round_id))
        .unwrap_or_else(|| panic_with_error!(e, Error::RoundNotFound))
}

fn save_position(e: &Env, round_id: u64, user: &Address, pos: &Position) {
    let key = Key::Position(round_id, user.clone());
    e.storage().temporary().set(&key, pos);
    e.storage().temporary().extend_ttl(&key, TEMP_THRESHOLD, TEMP_BUMP);
}

fn accumulate_fee(e: &Env, amount: i128) {
    let current: i128 = e.storage().instance().get(&Key::Fees).unwrap_or(0);
    e.storage()
        .instance()
        .set(&Key::Fees, &current.checked_add(amount).expect("fee accumulator overflow"));
}

fn bump_instance(e: &Env) {
    e.storage().instance().extend_ttl(INST_THRESHOLD, INST_BUMP);
}

/// Core claim logic — shared by claim() and claim_many().
/// Sets claimed=true BEFORE any transfer decision (reentrancy guard).
/// Returns 0 for losers and already-claimed positions (does not panic).
fn compute_claim(e: &Env, config: &Config, round_id: u64, user: &Address) -> i128 {
    let round = match e.storage().temporary().get::<_, Round>(&Key::Round(round_id)) {
        Some(r) if r.status == Status::Settled => r,
        _ => return 0,
    };

    let key = Key::Position(round_id, user.clone());
    let mut pos: Position = match e.storage().temporary().get::<_, Position>(&key) {
        Some(p) if !p.claimed => p,
        _ => return 0,
    };

    // Mark claimed before computing transfer amount (reentrancy guard).
    pos.claimed = true;
    e.storage().temporary().set(&key, &pos);

    if round.outcome == Outcome::Void {
        return pos.amount; // gross refund — no fee on void
    }

    let side_matches = matches!(
        (pos.side, round.outcome),
        (Side::Up, Outcome::Up) | (Side::Down, Outcome::Down)
    );
    if !side_matches {
        return 0; // loser
    }

    let total = round.pool_up + round.pool_down;
    let fee = total
        .checked_mul(config.fee_bps as i128)
        .expect("fee mul")
        .checked_div(10_000)
        .expect("fee div");
    let distributed = total.checked_sub(fee).expect("distributed sub");

    let winning_pool = match round.outcome {
        Outcome::Up => round.pool_up,
        Outcome::Down => round.pool_down,
        Outcome::Void => return pos.amount, // unreachable but safe fallback
    };

    // Multiply before divide — never (amount / pool) * distributed.
    pos.amount
        .checked_mul(distributed)
        .expect("payout mul overflow")
        .checked_div(winning_pool)
        .expect("payout div")
}
