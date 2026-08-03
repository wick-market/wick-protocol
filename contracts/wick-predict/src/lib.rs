// Wick Predict — XLM/USD binary prediction market on Stellar/Soroban
//
// Mechanics based on 9lives.so DPPM + Ninetails model.
// Earlier bettors earn more. Even losers get a partial refund.
//
// ── Payout at resolution (Above wins) ────────────────────────────────────────
//
//   loser_pool   = pool_below
//   distributed  = loser_pool * (10000 - fee_bps) / 10000
//
//   winner = stake_above             (1:1 base — always get your stake back)
//          + 70% * distributed * (user_above_boosted / total_above_boosted)
//          + 30% * distributed * (user_boosted / global_boosted)
//
//   loser  = 30% * distributed * (user_boosted / global_boosted)
//
//   boosted = amount * (lock_ts - bet_ts)   (time-weight: bet early → more)
//
//   Conservation: pool_above + loser_pool - fee = total paid out  ✓
//   Early-entry bonus: even losers receive proportional refund
//
// Oracle: Reflector ReflectorPulse (testnet)
//   CCYOZJCOPG34LLQQ7N24YXBM7LL62R7ONMZ3G6WZAAYPB5OYKOMJRN63
//   decimals=14, base=USD, 5-minute updates
//
// Storage TTL: Round + Position in TEMPORARY (7 days).
//              Config / Counter in PERSISTENT (30 days).

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error,
    symbol_short, token, Address, BytesN, Env, Symbol,
};

// ── Constants ─────────────────────────────────────────────────────────────────

const MAX_FEE_BPS: u32 = 500;
const MIN_LOCK_OFFSET: u64 = 10; // lowered so test oracle can use short rounds
// Ninetails split: 70% winner-side bonus, 30% early-entry refund (all participants)
const NINETAILS_WINNER_PCT: i128 = 7_000; // 70% of 10_000
const NINETAILS_REFUND_PCT: i128 = 3_000; // 30% of 10_000
const NINETAILS_DENOM: i128 = 10_000;

// Storage TTL
const TEMP_BUMP: u32 = 7 * 24 * 3600 / 5;
const TEMP_THRESHOLD: u32 = 3 * 24 * 3600 / 5;
const INST_BUMP: u32 = 30 * 24 * 3600 / 5;
const INST_THRESHOLD: u32 = 15 * 24 * 3600 / 5;

// ── Storage keys ──────────────────────────────────────────────────────────────

#[contracttype]
enum Key {
    Config,
    Counter,
    Fees,
    LastStrikeTs,
    Round(u64),
    Position(u64, Address),
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized  = 1,
    NotInitialized      = 2,
    RoundNotFound       = 3,
    RoundLocked         = 4,
    RoundNotSettled     = 5,
    AlreadySettled      = 6,
    TooEarly            = 7,
    BetTooSmall         = 8,
    AlreadyBet          = 9,
    NothingToClaim      = 10,
    FeeTooHigh          = 11,
    LockOffsetTooSmall  = 12,
    Unauthorized        = 13,
    DuplicateRound      = 14,
    OracleNoPrice       = 15,
    /// lock_offset must land strictly before settle, or betting stays open
    /// past the moment the outcome is already knowable.
    LockOffsetTooLate   = 16,
}

// ── Types ─────────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side { Above, Below }

#[contracttype]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome { Above, Below, Void }

#[contracttype]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status { Open, Locked, Settled }

#[contracttype]
#[derive(Clone, Debug)]
pub struct Round {
    pub id: u64,
    /// XLM/USD oracle price at open — the reference to predict against.
    pub strike: i128,
    pub strike_ts: u64,
    pub lock_ts: u64,
    pub settle_ts: u64,
    /// Total XLM staked on Above (stroops).
    pub pool_above: i128,
    /// Total XLM staked on Below (stroops).
    pub pool_below: i128,
    /// Sum of (amount × time_remaining) for all Above bettors.
    pub boosted_above: i128,
    /// Sum of (amount × time_remaining) for all Below bettors.
    pub boosted_below: i128,
    /// boosted_above + boosted_below — global denominator for loser refunds.
    pub global_boosted: i128,
    pub status: Status,
    pub outcome: Outcome,
    /// Settlement price from oracle. Zero until settled.
    pub settle_price: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Position {
    pub round_id: u64,
    pub side: Side,
    /// Stroops staked.
    pub amount: i128,
    /// Time-weighted shares: amount × (lock_ts − bet_ts).
    /// Larger for early bettors — determines Ninetails bonus share.
    pub boosted: i128,
    pub claimed: bool,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Config {
    pub admin: Address,
    pub oracle: Address,
    pub token: Address,
    pub asset: Symbol,
    pub fee_bps: u32,
    pub min_bet: i128,
    pub lock_offset: u64,
    pub oracle_decimals: u32,
    /// Seconds between oracle updates. Reflector=300, test oracle=60.
    pub oracle_interval: u64,
}

// ── Oracle client ─────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone)]
pub enum OracleAsset {
    Stellar(Address),
    Other(Symbol),
}

#[contracttype]
#[derive(Clone)]
pub struct PriceData {
    pub price: i128,
    pub timestamp: u64,
}

#[soroban_sdk::contractclient(name = "OracleClient")]
pub trait OracleTrait {
    fn lastprice(env: Env, asset: OracleAsset) -> Option<PriceData>;
    fn price(env: Env, asset: OracleAsset, timestamp: u64) -> Option<PriceData>;
    fn decimals(env: Env) -> u32;
}

// ── Pure math ─────────────────────────────────────────────────────────────────

/// Time-weighted boosted shares.
/// boosted = amount × (lock_ts − bet_ts)
/// Max at t=0 (bet at open), zero at t=lock_ts.
pub fn boosted_shares(amount: i128, bet_ts: u64, lock_ts: u64) -> i128 {
    let remaining = lock_ts.saturating_sub(bet_ts) as i128;
    amount.checked_mul(remaining).expect("boosted overflow")
}

/// Ninetails payout for a winner.
///
///   base          = staked amount (1:1 guarantee)
///   winner_bonus  = 70% of distributed × user_above_boosted / total_above_boosted
///   early_bonus   = 30% of distributed × user_boosted / global_boosted
///   total         = base + winner_bonus + early_bonus
pub fn winner_payout(
    staked: i128,
    distributed: i128,
    user_boosted: i128,
    side_boosted: i128,
    global_boosted: i128,
) -> i128 {
    let base = staked;

    let winner_share = if side_boosted > 0 {
        distributed
            .checked_mul(NINETAILS_WINNER_PCT).expect("w mul1")
            .checked_div(NINETAILS_DENOM).expect("w div1")
            .checked_mul(user_boosted).expect("w mul2")
            .checked_div(side_boosted).expect("w div2")
    } else {
        0
    };

    let refund_share = if global_boosted > 0 {
        distributed
            .checked_mul(NINETAILS_REFUND_PCT).expect("r mul1")
            .checked_div(NINETAILS_DENOM).expect("r div1")
            .checked_mul(user_boosted).expect("r mul2")
            .checked_div(global_boosted).expect("r div2")
    } else {
        0
    };

    base.checked_add(winner_share).expect("wp add1")
        .checked_add(refund_share).expect("wp add2")
}

/// Ninetails payout for a loser.
///
///   payout = 30% of distributed × user_boosted / global_boosted
pub fn loser_payout(
    distributed: i128,
    user_boosted: i128,
    global_boosted: i128,
) -> i128 {
    if global_boosted == 0 { return 0; }
    distributed
        .checked_mul(NINETAILS_REFUND_PCT).expect("lp mul1")
        .checked_div(NINETAILS_DENOM).expect("lp div1")
        .checked_mul(user_boosted).expect("lp mul2")
        .checked_div(global_boosted).expect("lp div2")
}

/// Fee on the losing pool, taken at resolution.
pub fn fee_amount(losing_pool: i128, fee_bps: u32) -> i128 {
    losing_pool
        .checked_mul(fee_bps as i128).expect("fee mul")
        .checked_div(10_000).expect("fee div")
}

// ── Storage ───────────────────────────────────────────────────────────────────

fn bump_instance(e: &Env) {
    e.storage().instance().extend_ttl(INST_THRESHOLD, INST_BUMP);
}

fn save_round(e: &Env, r: &Round) {
    let key = Key::Round(r.id);
    e.storage().temporary().set(&key, r);
    e.storage().temporary().extend_ttl(&key, TEMP_THRESHOLD, TEMP_BUMP);
}

fn load_round(e: &Env, id: u64) -> Round {
    e.storage()
        .temporary()
        .get(&Key::Round(id))
        .unwrap_or_else(|| panic_with_error!(e, Error::RoundNotFound))
}

fn save_pos(e: &Env, round_id: u64, user: &Address, pos: &Position) {
    let key = Key::Position(round_id, user.clone());
    e.storage().temporary().set(&key, pos);
    e.storage().temporary().extend_ttl(&key, TEMP_THRESHOLD, TEMP_BUMP);
}

fn next_id(e: &Env) -> u64 {
    let n: u64 = e.storage().instance().get(&Key::Counter).unwrap_or(0);
    e.storage().instance().set(&Key::Counter, &(n + 1));
    n + 1
}

fn accrue_fee(e: &Env, amt: i128) {
    let cur: i128 = e.storage().instance().get(&Key::Fees).unwrap_or(0);
    e.storage().instance().set(&Key::Fees, &(cur + amt));
}

fn require_config(e: &Env) -> Config {
    e.storage()
        .instance()
        .get(&Key::Config)
        .unwrap_or_else(|| panic_with_error!(e, Error::NotInitialized))
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct WickPredict;

#[contractimpl]
impl WickPredict {

    // ── Init ──────────────────────────────────────────────────────────────────

    pub fn initialize(
        e: Env,
        admin: Address,
        oracle: Address,
        token: Address,
        asset: Symbol,
        fee_bps: u32,
        min_bet: i128,
        lock_offset: u64,
        oracle_interval: u64,
    ) {
        if e.storage().instance().has(&Key::Config) {
            panic_with_error!(&e, Error::AlreadyInitialized);
        }
        admin.require_auth();
        if fee_bps > MAX_FEE_BPS { panic_with_error!(&e, Error::FeeTooHigh); }
        if lock_offset < MIN_LOCK_OFFSET { panic_with_error!(&e, Error::LockOffsetTooSmall); }

        let decimals = OracleClient::new(&e, &oracle).decimals();
        e.storage().instance().set(&Key::Config, &Config {
            admin, oracle, token, asset, fee_bps, min_bet, lock_offset,
            oracle_decimals: decimals,
            oracle_interval,
        });
        e.storage().instance().set(&Key::Counter, &0u64);
        e.storage().instance().set(&Key::Fees, &0i128);
        bump_instance(&e);
    }

    // ── Round creation (permissionless) ──────────────────────────────────────

    pub fn create_round(e: Env) -> u64 {
        let config = require_config(&e);
        let oracle = OracleClient::new(&e, &config.oracle);
        let tick = oracle
            .lastprice(&OracleAsset::Other(config.asset.clone()))
            .unwrap_or_else(|| panic_with_error!(&e, Error::OracleNoPrice));

        let strike_ts = tick.timestamp;
        let last: u64 = e.storage().instance().get(&Key::LastStrikeTs).unwrap_or(0);
        if last == strike_ts { panic_with_error!(&e, Error::DuplicateRound); }

        let id = next_id(&e);
        let round = Round {
            id,
            strike: tick.price,
            strike_ts,
            lock_ts: strike_ts + config.lock_offset,
            settle_ts: strike_ts + config.oracle_interval,
            pool_above: 0,
            pool_below: 0,
            boosted_above: 0,
            boosted_below: 0,
            global_boosted: 0,
            status: Status::Open,
            outcome: Outcome::Void,
            settle_price: 0,
        };
        save_round(&e, &round);
        e.storage().instance().set(&Key::LastStrikeTs, &strike_ts);
        bump_instance(&e);

        e.events().publish(
            (symbol_short!("created"), id),
            (tick.price, strike_ts, round.lock_ts, round.settle_ts),
        );
        id
    }

    // ── Betting ───────────────────────────────────────────────────────────────

    pub fn bet_above(e: Env, user: Address, round_id: u64, amount: i128) {
        Self::_bet(&e, user, round_id, Side::Above, amount);
    }

    pub fn bet_below(e: Env, user: Address, round_id: u64, amount: i128) {
        Self::_bet(&e, user, round_id, Side::Below, amount);
    }

    fn _bet(e: &Env, user: Address, round_id: u64, side: Side, amount: i128) {
        user.require_auth();
        let config = require_config(e);
        if amount < config.min_bet { panic_with_error!(e, Error::BetTooSmall); }

        let now = e.ledger().timestamp();
        let mut round = load_round(e, round_id);
        if now >= round.lock_ts { panic_with_error!(e, Error::RoundLocked); }
        if round.status != Status::Open { panic_with_error!(e, Error::RoundLocked); }

        if e.storage().temporary().has(&Key::Position(round_id, user.clone())) {
            panic_with_error!(e, Error::AlreadyBet);
        }

        // Ninetails: time-weighted boosted shares
        let boosted = boosted_shares(amount, now, round.lock_ts);

        // Transfer XLM to contract
        token::TokenClient::new(e, &config.token)
            .transfer(&user, &e.current_contract_address(), &amount);

        match side {
            Side::Above => {
                round.pool_above += amount;
                round.boosted_above += boosted;
            }
            Side::Below => {
                round.pool_below += amount;
                round.boosted_below += boosted;
            }
        }
        round.global_boosted += boosted;
        save_round(e, &round);

        let pos = Position { round_id, side, amount, boosted, claimed: false };
        save_pos(e, round_id, &user, &pos);

        e.events().publish(
            (symbol_short!("bet"), round_id),
            (user, side, amount, boosted),
        );
        bump_instance(e);
    }

    // ── Settlement (permissionless) ───────────────────────────────────────────

    pub fn settle(e: Env, round_id: u64) {
        let config = require_config(&e);
        let mut round = load_round(&e, round_id);

        if round.status == Status::Settled { panic_with_error!(&e, Error::AlreadySettled); }
        if e.ledger().timestamp() < round.settle_ts { panic_with_error!(&e, Error::TooEarly); }

        let oracle = OracleClient::new(&e, &config.oracle);
        let price_data = oracle.price(&OracleAsset::Other(config.asset.clone()), &round.settle_ts);

        let outcome = match price_data {
            None => Outcome::Void,
            Some(p) => {
                round.settle_price = p.price;
                if p.price > round.strike { Outcome::Above }
                else if p.price < round.strike { Outcome::Below }
                else { Outcome::Void }
            }
        };

        // Empty pool on either side → void
        let outcome = if round.pool_above == 0 || round.pool_below == 0 {
            Outcome::Void
        } else {
            outcome
        };

        // Collect fee from losing pool on resolved (non-void) rounds
        if outcome != Outcome::Void {
            let losing_pool = if outcome == Outcome::Above {
                round.pool_below
            } else {
                round.pool_above
            };
            accrue_fee(&e, fee_amount(losing_pool, config.fee_bps));
        }

        round.outcome = outcome;
        round.status = Status::Settled;
        save_round(&e, &round);
        bump_instance(&e);

        e.events().publish(
            (symbol_short!("settled"), round_id),
            (outcome, round.settle_price),
        );
    }

    // ── Claim ─────────────────────────────────────────────────────────────────

    pub fn claim(e: Env, user: Address, round_id: u64) -> i128 {
        user.require_auth();
        let config = require_config(&e);
        let round = load_round(&e, round_id);

        if round.status != Status::Settled { panic_with_error!(&e, Error::RoundNotSettled); }

        let key = Key::Position(round_id, user.clone());
        let mut pos: Position = e
            .storage()
            .temporary()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&e, Error::NothingToClaim));

        if pos.claimed { panic_with_error!(&e, Error::NothingToClaim); }

        let payout = if round.outcome == Outcome::Void {
            pos.amount // full refund, no fee on void
        } else {
            let losing_pool = if round.outcome == Outcome::Above {
                round.pool_below
            } else {
                round.pool_above
            };
            let fee = fee_amount(losing_pool, config.fee_bps);
            let distributed = losing_pool - fee;

            let on_winning_side = matches!(
                (pos.side, round.outcome),
                (Side::Above, Outcome::Above) | (Side::Below, Outcome::Below)
            );

            if on_winning_side {
                let side_boosted = if round.outcome == Outcome::Above {
                    round.boosted_above
                } else {
                    round.boosted_below
                };
                winner_payout(
                    pos.amount,
                    distributed,
                    pos.boosted,
                    side_boosted,
                    round.global_boosted,
                )
            } else {
                // Loser gets 30% of distributed proportional to early-entry boost
                loser_payout(distributed, pos.boosted, round.global_boosted)
            }
        };

        // Reentrancy guard: mark before transfer
        pos.claimed = true;
        e.storage().temporary().set(&key, &pos);

        if payout > 0 {
            token::TokenClient::new(&e, &config.token)
                .transfer(&e.current_contract_address(), &user, &payout);
        }

        e.events().publish((symbol_short!("claim"), round_id), (user, payout));
        bump_instance(&e);
        payout
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    pub fn get_round(e: Env, round_id: u64) -> Round {
        let mut r = load_round(&e, round_id);
        if r.status == Status::Open && e.ledger().timestamp() >= r.lock_ts {
            r.status = Status::Locked;
        }
        r
    }

    pub fn get_position(e: Env, round_id: u64, user: Address) -> Option<Position> {
        e.storage().temporary().get(&Key::Position(round_id, user))
    }

    pub fn get_config(e: Env) -> Config { require_config(&e) }

    pub fn current_round_id(e: Env) -> u64 {
        e.storage().instance().get(&Key::Counter).unwrap_or(0)
    }

    // ── Admin ─────────────────────────────────────────────────────────────────

    pub fn sweep_fees(e: Env) {
        let config = require_config(&e);
        config.admin.require_auth();
        let fees: i128 = e.storage().instance().get(&Key::Fees).unwrap_or(0);
        if fees > 0 {
            token::TokenClient::new(&e, &config.token)
                .transfer(&e.current_contract_address(), &config.admin, &fees);
            e.storage().instance().set(&Key::Fees, &0i128);
        }
        bump_instance(&e);
    }

    pub fn set_fee_bps(e: Env, bps: u32) {
        let mut config = require_config(&e);
        config.admin.require_auth();
        if bps > MAX_FEE_BPS { panic_with_error!(&e, Error::FeeTooHigh); }
        config.fee_bps = bps;
        e.storage().instance().set(&Key::Config, &config);
        bump_instance(&e);
    }

    pub fn set_lock_offset(e: Env, seconds: u64) {
        let mut config = require_config(&e);
        config.admin.require_auth();
        if seconds < MIN_LOCK_OFFSET { panic_with_error!(&e, Error::LockOffsetTooSmall); }
        // Betting must close before settle. Equal is not enough: at lock_ts ==
        // settle_ts the settle price is already determined, so a bet landing in
        // that same second would be a free win.
        if seconds >= config.oracle_interval { panic_with_error!(&e, Error::LockOffsetTooLate); }
        config.lock_offset = seconds;
        e.storage().instance().set(&Key::Config, &config);
        bump_instance(&e);
    }

    /// Round length: settle_ts = strike_ts + oracle_interval.
    ///
    /// Must stay strictly above lock_offset, so shortening a round cannot
    /// silently swallow the lock window that set_lock_offset guards.
    pub fn set_oracle_interval(e: Env, seconds: u64) {
        let mut config = require_config(&e);
        config.admin.require_auth();
        if seconds <= config.lock_offset { panic_with_error!(&e, Error::LockOffsetTooLate); }
        config.oracle_interval = seconds;
        e.storage().instance().set(&Key::Config, &config);
        bump_instance(&e);
    }

    /// Hand the admin role to a new address.
    ///
    /// Without this a leaked admin key is permanent: the key signs create_round,
    /// settle, sweep_fees and the fee/offset setters, and the only remedy would
    /// be redeploying and abandoning every open round. Requires auth from both
    /// sides so a typo cannot strand the contract on an address nobody holds.
    pub fn set_admin(e: Env, new_admin: Address) {
        let mut config = require_config(&e);
        config.admin.require_auth();
        new_admin.require_auth();
        config.admin = new_admin;
        e.storage().instance().set(&Key::Config, &config);
        bump_instance(&e);
    }

    /// Swap the contract's own code, preserving all storage.
    ///
    /// The counterpart to set_admin: it lets a fix ship without new contract
    /// IDs, so the frontend and round history survive an upgrade.
    pub fn upgrade(e: Env, new_wasm_hash: BytesN<32>) {
        let config = require_config(&e);
        config.admin.require_auth();
        e.deployer().update_current_contract_wasm(new_wasm_hash);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token, Env,
    };

    // ── Mock oracle ───────────────────────────────────────────────────────────

    #[contracttype]
    enum MK { Prices }

    #[soroban_sdk::contract]
    struct MockOracle;

    #[soroban_sdk::contractimpl]
    impl MockOracle {
        pub fn set_price(e: Env, ts: u64, price: i128) {
            let mut m: soroban_sdk::Map<u64, i128> = e
                .storage().instance().get(&MK::Prices)
                .unwrap_or_else(|| soroban_sdk::map![&e]);
            m.set(ts, price);
            e.storage().instance().set(&MK::Prices, &m);
        }
        pub fn lastprice(e: Env, _asset: OracleAsset) -> Option<PriceData> {
            let m: soroban_sdk::Map<u64, i128> = e
                .storage().instance().get(&MK::Prices)
                .unwrap_or_else(|| soroban_sdk::map![&e]);
            let mut best: Option<PriceData> = None;
            for (ts, price) in m.iter() {
                match &best {
                    None => best = Some(PriceData { price, timestamp: ts }),
                    Some(b) if ts > b.timestamp => best = Some(PriceData { price, timestamp: ts }),
                    _ => {}
                }
            }
            best
        }
        pub fn price(e: Env, _asset: OracleAsset, timestamp: u64) -> Option<PriceData> {
            let m: soroban_sdk::Map<u64, i128> = e
                .storage().instance().get(&MK::Prices)
                .unwrap_or_else(|| soroban_sdk::map![&e]);
            m.get(timestamp).map(|p| PriceData { price: p, timestamp })
        }
        pub fn decimals(_: Env) -> u32 { 14 }
    }

    // ── Harness ───────────────────────────────────────────────────────────────

    const LOCK: u64 = 180;
    const FEE: u32 = 200; // 2%
    const MIN: i128 = 100_000_000; // 10 XLM
    const T0: u64 = 1_000_000;
    const STRIKE: i128 = 17_000_000_000_000; // ~$0.17
    const PRICE_UP: i128 = 18_000_000_000_000;
    const PRICE_DN: i128 = 16_000_000_000_000;

    struct Ctx { env: Env, c: Address, oracle: Address, token: Address, admin: Address }

    impl Ctx {
        fn new() -> Self {
            let env = Env::default();
            env.mock_all_auths();
            env.ledger().set_timestamp(T0);
            let admin = Address::generate(&env);
            let oracle = env.register(MockOracle, ());
            let token = env.register_stellar_asset_contract_v2(admin.clone()).address();
            let c = env.register(WickPredict, ());
            let asset = soroban_sdk::Symbol::new(&env, "XLM");
            WickPredictClient::new(&env, &c).initialize(&admin, &oracle, &token, &asset, &FEE, &MIN, &LOCK, &300u64);
            Ctx { env, c, oracle, token, admin }
        }
        fn client(&self) -> WickPredictClient { WickPredictClient::new(&self.env, &self.c) }
        fn oracle(&self) -> MockOracleClient { MockOracleClient::new(&self.env, &self.oracle) }
        fn set_price(&self, ts: u64, price: i128) { self.oracle().set_price(&ts, &price); }
        fn mint(&self, to: &Address, n: i128) {
            token::StellarAssetClient::new(&self.env, &self.token).mint(to, &n);
        }
        fn balance(&self, a: &Address) -> i128 {
            token::TokenClient::new(&self.env, &self.token).balance(a)
        }
        fn now(&self) -> u64 { self.env.ledger().timestamp() }
        fn set_ts(&self, ts: u64) { self.env.ledger().set_timestamp(ts); }
        fn open(&self) -> u64 {
            self.set_price(T0, STRIKE);
            self.client().create_round()
        }
        fn settle(&self, id: u64, price: i128) {
            let r = self.client().get_round(&id);
            self.set_price(r.settle_ts, price);
            self.set_ts(r.settle_ts);
            self.client().settle(&id);
        }
    }

    macro_rules! err {
        ($r:expr, $e:expr) => {
            match $r {
                Err(Ok(e)) => assert_eq!(e, Into::<soroban_sdk::Error>::into($e)),
                o => panic!("expected {:?}, got {:?}", $e, o),
            }
        };
    }

    // ── Pure math ─────────────────────────────────────────────────────────────

    #[test]
    fn boosted_early_beats_late() {
        let early = boosted_shares(1_000_000_000, T0,       T0 + LOCK);
        let late  = boosted_shares(1_000_000_000, T0 + 170, T0 + LOCK);
        assert!(early > late, "early bettor should have more boosted shares");
    }

    #[test]
    fn winner_payout_includes_base() {
        // winner always gets at least their stake back
        let payout = winner_payout(500_000_000, 200_000_000, 1000, 2000, 3000);
        assert!(payout >= 500_000_000, "winner should receive at least their stake");
    }

    #[test]
    fn loser_gets_partial_refund() {
        // loser receives 30% of distributed proportional to early-entry boost
        let payout = loser_payout(300_000_000, 500, 1000);
        // = 300M * 30% * 500/1000 = 300M * 0.15 = 45M
        assert_eq!(payout, 45_000_000);
    }

    #[test]
    fn conservation_winner_plus_losers_eq_total() {
        // One winner (500 XLM), one loser (300 XLM), both bet at t=0
        let total = 800_000_000i128;
        let losing_pool = 300_000_000i128;
        let fee = fee_amount(losing_pool, FEE);
        let distributed = losing_pool - fee;

        let w_boosted = 500_000_000i128 * LOCK as i128;
        let l_boosted = 300_000_000i128 * LOCK as i128;
        let global = w_boosted + l_boosted;

        let w_pay = winner_payout(500_000_000, distributed, w_boosted, w_boosted, global);
        let l_pay = loser_payout(distributed, l_boosted, global);

        let sum = w_pay + l_pay;
        assert!(sum + fee <= total,
            "conservation violated: sum={sum} fee={fee} total={total}");
        // Dust should be small
        let dust = total - sum - fee;
        assert!(dust < 1000, "unexpected dust: {dust}");
    }

    #[test]
    fn conservation_multiple_bettors() {
        let winning = [(500_000_000i128, T0), (300_000_000i128, T0+60), (200_000_000i128, T0+120)];
        let losing  = [(400_000_000i128, T0+30), (150_000_000i128, T0+90)];

        let pool_win: i128  = winning.iter().map(|(a,_)| a).sum();
        let pool_lose: i128 = losing.iter().map(|(a,_)| a).sum();
        let total = pool_win + pool_lose;

        let fee = fee_amount(pool_lose, FEE);
        let distributed = pool_lose - fee;

        let win_b  = [boosted_shares(winning[0].0, winning[0].1, T0+LOCK),
                      boosted_shares(winning[1].0, winning[1].1, T0+LOCK),
                      boosted_shares(winning[2].0, winning[2].1, T0+LOCK)];
        let lose_b = [boosted_shares(losing[0].0, losing[0].1, T0+LOCK),
                      boosted_shares(losing[1].0, losing[1].1, T0+LOCK)];

        let total_win_b: i128  = win_b.iter().sum();
        let total_lose_b: i128 = lose_b.iter().sum();
        let global = total_win_b + total_lose_b;

        let mut total_out = fee;
        for (i, (amt, _)) in winning.iter().enumerate() {
            total_out += winner_payout(*amt, distributed, win_b[i], total_win_b, global);
        }
        for (i, _) in losing.iter().enumerate() {
            total_out += loser_payout(distributed, lose_b[i], global);
        }

        assert!(total_out <= total,
            "overpaid: total_out={total_out} total={total}");
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────────

    #[test]
    fn happy_path_above_wins_winner_beats_loser() {
        let ctx = Ctx::new();
        let id = ctx.open();

        let alice = Address::generate(&ctx.env); // bets Above early
        let bob   = Address::generate(&ctx.env); // bets Above late
        let carol = Address::generate(&ctx.env); // bets Below (loser)

        ctx.mint(&alice, 5_000_000_000);
        ctx.mint(&bob,   3_000_000_000);
        ctx.mint(&carol, 2_000_000_000);

        ctx.client().bet_above(&alice, &id, &5_000_000_000); // t=T0, max boost
        ctx.set_ts(T0 + 120);
        ctx.client().bet_above(&bob, &id, &3_000_000_000);   // t=T0+120, less boost
        ctx.set_ts(T0 + 30);
        ctx.client().bet_below(&carol, &id, &2_000_000_000); // t=T0+30

        ctx.settle(id, PRICE_UP); // Above wins

        let alice_bal = ctx.balance(&alice);
        let alice_pay = ctx.client().claim(&alice, &id);
        assert!(alice_pay > 5_000_000_000, "alice (early winner) should profit");
        assert_eq!(ctx.balance(&alice) - alice_bal, alice_pay);

        let bob_pay = ctx.client().claim(&bob, &id);
        assert!(bob_pay > 3_000_000_000, "bob (late winner) should still profit");

        // Alice bet early → more boosted → bigger winner bonus than Bob
        assert!(alice_pay > bob_pay,
            "early bettor alice ({alice_pay}) should outperform late bettor bob ({bob_pay})");

        // Carol loses but gets a partial refund (30% of distributed * carol_boost/global)
        let carol_pay = ctx.client().claim(&carol, &id);
        assert!(carol_pay > 0, "loser carol should receive partial ninetails refund");
        assert!(carol_pay < 2_000_000_000, "loser should not get full stake back");
    }

    #[test]
    fn happy_path_below_wins() {
        let ctx = Ctx::new();
        let id = ctx.open();

        let alice = Address::generate(&ctx.env);
        let bob   = Address::generate(&ctx.env);
        ctx.mint(&alice, 3_000_000_000);
        ctx.mint(&bob,   6_000_000_000);

        ctx.client().bet_above(&alice, &id, &3_000_000_000);
        ctx.set_ts(T0 + 60);
        ctx.client().bet_below(&bob, &id, &6_000_000_000);

        ctx.settle(id, PRICE_DN);

        let bob_pay = ctx.client().claim(&bob, &id);
        assert!(bob_pay > 6_000_000_000, "bob (below winner) should profit");

        let alice_pay = ctx.client().claim(&alice, &id);
        assert!(alice_pay > 0, "alice (loser) gets ninetails refund");
        assert!(alice_pay < 3_000_000_000);
    }

    #[test]
    fn void_empty_pool_full_refund() {
        let ctx = Ctx::new();
        let id = ctx.open();
        let alice = Address::generate(&ctx.env);
        ctx.mint(&alice, 5_000_000_000);
        ctx.client().bet_above(&alice, &id, &5_000_000_000);
        ctx.settle(id, PRICE_UP); // void — only above bets
        assert_eq!(ctx.client().claim(&alice, &id), 5_000_000_000);
    }

    #[test]
    fn void_exact_tie_full_refund() {
        let ctx = Ctx::new();
        let id = ctx.open();
        let alice = Address::generate(&ctx.env);
        let bob   = Address::generate(&ctx.env);
        ctx.mint(&alice, 3_000_000_000);
        ctx.mint(&bob,   3_000_000_000);
        ctx.client().bet_above(&alice, &id, &3_000_000_000);
        ctx.client().bet_below(&bob,   &id, &3_000_000_000);
        ctx.settle(id, STRIKE); // exact tie
        assert_eq!(ctx.client().claim(&alice, &id), 3_000_000_000);
        assert_eq!(ctx.client().claim(&bob,   &id), 3_000_000_000);
    }

    #[test]
    fn void_oracle_gap_full_refund() {
        let ctx = Ctx::new();
        let id = ctx.open();
        let alice = Address::generate(&ctx.env);
        ctx.mint(&alice, 2_000_000_000);
        ctx.client().bet_above(&alice, &id, &2_000_000_000);
        let r = ctx.client().get_round(&id);
        ctx.set_ts(r.settle_ts); // no settle price set → oracle returns None
        ctx.client().settle(&id);
        assert_eq!(ctx.client().claim(&alice, &id), 2_000_000_000);
    }

    #[test]
    fn bet_after_lock_rejected() {
        let ctx = Ctx::new();
        let id = ctx.open();
        ctx.set_ts(T0 + LOCK);
        let alice = Address::generate(&ctx.env);
        ctx.mint(&alice, MIN);
        err!(ctx.client().try_bet_above(&alice, &id, &MIN), Error::RoundLocked);
    }

    #[test]
    fn bet_below_minimum_rejected() {
        let ctx = Ctx::new();
        let id = ctx.open();
        let alice = Address::generate(&ctx.env);
        ctx.mint(&alice, MIN);
        err!(ctx.client().try_bet_above(&alice, &id, &(MIN - 1)), Error::BetTooSmall);
    }

    #[test]
    fn double_bet_rejected() {
        let ctx = Ctx::new();
        let id = ctx.open();
        let alice = Address::generate(&ctx.env);
        ctx.mint(&alice, MIN * 2);
        ctx.client().bet_above(&alice, &id, &MIN);
        err!(ctx.client().try_bet_above(&alice, &id, &MIN), Error::AlreadyBet);
    }

    #[test]
    fn double_claim_rejected() {
        let ctx = Ctx::new();
        let id = ctx.open();
        let alice = Address::generate(&ctx.env);
        let bob   = Address::generate(&ctx.env);
        ctx.mint(&alice, 3_000_000_000);
        ctx.mint(&bob,   2_000_000_000);
        ctx.client().bet_above(&alice, &id, &3_000_000_000);
        ctx.client().bet_below(&bob,   &id, &2_000_000_000);
        ctx.settle(id, PRICE_UP);
        ctx.client().claim(&alice, &id);
        err!(ctx.client().try_claim(&alice, &id), Error::NothingToClaim);
    }

    #[test]
    fn settle_too_early_rejected() {
        let ctx = Ctx::new();
        let id = ctx.open();
        err!(ctx.client().try_settle(&id), Error::TooEarly);
    }

    #[test]
    fn settle_idempotent() {
        let ctx = Ctx::new();
        let id = ctx.open();
        let alice = Address::generate(&ctx.env);
        let bob   = Address::generate(&ctx.env);
        ctx.mint(&alice, 2_000_000_000);
        ctx.mint(&bob,   2_000_000_000);
        ctx.client().bet_above(&alice, &id, &2_000_000_000);
        ctx.client().bet_below(&bob,   &id, &2_000_000_000);
        ctx.settle(id, PRICE_UP);
        err!(ctx.client().try_settle(&id), Error::AlreadySettled);
    }

    #[test]
    fn duplicate_round_rejected() {
        let ctx = Ctx::new();
        ctx.open();
        err!(ctx.client().try_create_round(), Error::DuplicateRound);
    }

    // ── Lock window ───────────────────────────────────────────────────────────
    // Betting must close strictly before settle. If lock_ts reaches settle_ts a
    // bet could land in the same second the settle price is read, which is a
    // free win — so both setters guard the gap from their own side.

    #[test]
    fn lock_offset_at_or_past_settle_rejected() {
        let ctx = Ctx::new();
        // Harness initializes with oracle_interval = 300.
        err!(ctx.client().try_set_lock_offset(&300), Error::LockOffsetTooLate);
        err!(ctx.client().try_set_lock_offset(&301), Error::LockOffsetTooLate);
        // Just inside the boundary is fine.
        ctx.client().set_lock_offset(&299);
        assert_eq!(ctx.client().get_config().lock_offset, 299);
    }

    #[test]
    fn interval_cannot_shrink_below_lock() {
        let ctx = Ctx::new();
        ctx.client().set_lock_offset(&120);
        // Shrinking the round to the lock point (or past it) would leave no
        // window, so it is refused rather than silently clamped.
        err!(ctx.client().try_set_oracle_interval(&120), Error::LockOffsetTooLate);
        err!(ctx.client().try_set_oracle_interval(&90), Error::LockOffsetTooLate);
        ctx.client().set_oracle_interval(&121);
        assert_eq!(ctx.client().get_config().oracle_interval, 121);
    }

    #[test]
    fn widened_betting_window_still_locks_before_settle() {
        let ctx = Ctx::new();
        // The change this exists for: 45s betting -> 60s, round 60s -> 75s.
        // Order matters — the new interval must clear the *current* lock, so
        // narrow the lock first. Doing it the other way is what the guard
        // refuses, which is the point.
        ctx.client().set_lock_offset(&60);
        ctx.client().set_oracle_interval(&75);

        let id = ctx.open();
        let r = ctx.client().get_round(&id);
        assert_eq!(r.lock_ts - r.strike_ts, 60, "betting window should be 60s");
        assert_eq!(r.settle_ts - r.strike_ts, 75, "round should be 75s");
        assert!(r.lock_ts < r.settle_ts, "lock must precede settle");

        // A bet at 59s lands; at 60s it does not.
        let alice = Address::generate(&ctx.env);
        let bob = Address::generate(&ctx.env);
        ctx.mint(&alice, 2_000_000_000);
        ctx.mint(&bob, 2_000_000_000);

        ctx.set_ts(r.strike_ts + 59);
        ctx.client().bet_above(&alice, &id, &1_000_000_000);

        ctx.set_ts(r.lock_ts);
        err!(
            ctx.client().try_bet_below(&bob, &id, &1_000_000_000),
            Error::RoundLocked
        );
    }

    // ── The key invariant ─────────────────────────────────────────────────────
    // Contract balance never goes negative regardless of bet timing.

    #[test]
    fn contract_never_overpays() {
        let ctx = Ctx::new();
        let id = ctx.open();

        let bettors = [
            (Address::generate(&ctx.env), Side::Above, 2_000_000_000i128, 0u64),
            (Address::generate(&ctx.env), Side::Above, 1_500_000_000,     60),
            (Address::generate(&ctx.env), Side::Above,   800_000_000,    120),
            (Address::generate(&ctx.env), Side::Below, 3_000_000_000,     30),
            (Address::generate(&ctx.env), Side::Below, 1_200_000_000,     90),
        ];

        let total_staked: i128 = bettors.iter().map(|(_, _, a, _)| *a).sum();

        for (addr, side, amt, offset) in &bettors {
            ctx.mint(addr, *amt);
            ctx.set_ts(T0 + offset);
            match side {
                Side::Above => ctx.client().bet_above(addr, &id, amt),
                Side::Below => ctx.client().bet_below(addr, &id, amt),
            }
        }

        ctx.settle(id, PRICE_UP); // Above wins

        let _contract_before = ctx.balance(&ctx.c);
        let mut total_paid: i128 = 0;
        for (addr, _, _, _) in &bettors {
            total_paid += ctx.client().claim(addr, &id);
        }

        assert!(ctx.balance(&ctx.c) >= 0, "contract went negative");
        assert!(total_paid <= total_staked,
            "overpaid: {total_paid} > total_staked {total_staked}");
    }
}
