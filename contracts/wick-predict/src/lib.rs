// Wick Predict — XLM/USD binary prediction market on Stellar/Soroban
//
// Inspired by 9lives' DPPM (Dynamic Pari-Mutuel Prediction Market) model.
// Earlier bettors receive more shares per XLM than late bettors, rewarding
// those who commit capital early with less information.
//
// Oracle: Reflector ReflectorPulse (testnet)
//   CCYOZJCOPG34LLQQ7N24YXBM7LL62R7ONMZ3G6WZAAYPB5OYKOMJRN63
//   decimals=14, base=USD, updates every 300s
//
// Round lifecycle (5 minutes total):
//   t=0:00   create_round() — reads lastprice() → strike
//   t=0:00→3:00  bet_above/bet_below open — shares decrease with time
//   t=3:00   lock — no new bets
//   t=5:00   settle() — reads price(XLM, settle_ts) → outcome
//
// Share formula (Ninetails-style):
//   scale   = 1000 − (elapsed_secs × 500 / window_secs)   [1000 → 500]
//   shares  = amount × scale / 1000
//
// Payout (winner):
//   distributed = losing_pool × (10000 − fee_bps) / 10000
//   payout_i    = shares_i × distributed / total_winning_shares
//
// Void (empty pool, exact tie, oracle gap): gross refund, no fee.

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error,
    symbol_short, token, Address, Env, Symbol,
};

// ── Constants ─────────────────────────────────────────────────────────────────

const ORACLE_INTERVAL: u64 = 300;   // Reflector updates every 5 minutes
const SCALE_BASE: i128 = 1000;      // share scale denominator
const SCALE_MIN: i128 = 500;        // scale at lock_ts (50% of SCALE_BASE)
const MAX_FEE_BPS: u32 = 500;       // 5% hard cap
const MIN_LOCK_OFFSET: u64 = 90;    // minimum betting window (seconds)

// Temporary storage TTL (7 days in ledgers, ~5s each)
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
    LastStrikeTs,       // dedup: reject create_round if same oracle tick
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
    /// XLM/USD oracle price at open (14 decimals). Strike bettors predict against.
    pub strike: i128,
    /// Oracle's own timestamp for the strike price — NOT ledger time.
    pub strike_ts: u64,
    /// strike_ts + lock_offset. No bets after this.
    pub lock_ts: u64,
    /// strike_ts + ORACLE_INTERVAL (300s). Settlement reads price at this ts.
    pub settle_ts: u64,
    /// Total XLM staked on Above (stroops).
    pub pool_above: i128,
    /// Total XLM staked on Below (stroops).
    pub pool_below: i128,
    /// Time-weighted shares issued on Above side.
    pub shares_above: i128,
    /// Time-weighted shares issued on Below side.
    pub shares_below: i128,
    pub status: Status,
    pub outcome: Outcome,
    /// Settlement price. Zero until settled.
    pub settle_price: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Position {
    pub round_id: u64,
    pub side: Side,
    /// XLM staked in stroops.
    pub amount: i128,
    /// Time-weighted shares received. Earlier = more shares per XLM.
    pub shares: i128,
    pub claimed: bool,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Config {
    pub admin: Address,
    /// Reflector oracle contract.
    pub oracle: Address,
    /// XLM Stellar Asset Contract.
    pub token: Address,
    /// Fee in basis points (e.g. 200 = 2%). Applied to losing pool on payout.
    pub fee_bps: u32,
    /// Minimum bet in stroops (default 100_000_000 = 10 XLM).
    pub min_bet: i128,
    /// Seconds after strike_ts before betting closes (default 180, min 90).
    pub lock_offset: u64,
    /// oracle.decimals() — for frontend display normalisation.
    pub oracle_decimals: u32,
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

fn xlm_asset(e: &Env) -> OracleAsset {
    OracleAsset::Other(Symbol::new(e, "XLM"))
}

// ── Pure math ─────────────────────────────────────────────────────────────────

/// Time-weighted share amount.
/// scale ∈ [SCALE_MIN, SCALE_BASE] → shares ∈ [amount/2, amount].
pub fn compute_shares(amount: i128, bet_ts: u64, strike_ts: u64, lock_ts: u64) -> i128 {
    let window = lock_ts.saturating_sub(strike_ts) as i128;
    if window <= 0 {
        return amount; // degenerate case — no time weighting
    }
    let elapsed = bet_ts.saturating_sub(strike_ts) as i128;
    let elapsed = elapsed.min(window);
    // scale = 1000 at t=0, 500 at t=lock_ts
    let scale = SCALE_BASE - (elapsed * (SCALE_BASE - SCALE_MIN) / window);
    amount
        .checked_mul(scale).expect("shares mul overflow")
        .checked_div(SCALE_BASE).expect("shares div overflow")
}

/// Winner's payout from the losing pool.
/// payout_i = shares_i × distributed / total_winning_shares
pub fn compute_payout(
    shares: i128,
    total_winning_shares: i128,
    losing_pool: i128,
    fee_bps: u32,
) -> i128 {
    if shares == 0 || total_winning_shares == 0 || losing_pool == 0 {
        return 0;
    }
    let distributed = losing_pool
        .checked_mul((10_000 - fee_bps as i128)).expect("dist mul overflow")
        .checked_div(10_000).expect("dist div overflow");
    shares
        .checked_mul(distributed).expect("payout mul overflow")
        .checked_div(total_winning_shares).expect("payout div overflow")
}

// ── Storage helpers ───────────────────────────────────────────────────────────

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

fn save_position(e: &Env, round_id: u64, user: &Address, pos: &Position) {
    let key = Key::Position(round_id, user.clone());
    e.storage().temporary().set(&key, pos);
    e.storage().temporary().extend_ttl(&key, TEMP_THRESHOLD, TEMP_BUMP);
}

fn next_id(e: &Env) -> u64 {
    let n: u64 = e.storage().instance().get(&Key::Counter).unwrap_or(0);
    let next = n + 1;
    e.storage().instance().set(&Key::Counter, &next);
    next
}

fn add_fees(e: &Env, amount: i128) {
    let cur: i128 = e.storage().instance().get(&Key::Fees).unwrap_or(0);
    e.storage().instance().set(&Key::Fees, &(cur + amount));
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
        fee_bps: u32,
        min_bet: i128,
        lock_offset: u64,
    ) {
        if e.storage().instance().has(&Key::Config) {
            panic_with_error!(&e, Error::AlreadyInitialized);
        }
        admin.require_auth();
        if fee_bps > MAX_FEE_BPS { panic_with_error!(&e, Error::FeeTooHigh); }
        if lock_offset < MIN_LOCK_OFFSET { panic_with_error!(&e, Error::LockOffsetTooSmall); }

        let decimals = OracleClient::new(&e, &oracle).decimals();
        e.storage().instance().set(&Key::Config, &Config {
            admin, oracle, token, fee_bps, min_bet, lock_offset,
            oracle_decimals: decimals,
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
            .lastprice(&xlm_asset(&e))
            .unwrap_or_else(|| panic_with_error!(&e, Error::OracleNoPrice));

        let strike_ts = tick.timestamp;

        // Dedup: one round per oracle tick
        let last_ts: u64 = e.storage().instance().get(&Key::LastStrikeTs).unwrap_or(0);
        if last_ts == strike_ts { panic_with_error!(&e, Error::DuplicateRound); }

        let id = next_id(&e);
        let round = Round {
            id,
            strike: tick.price,
            strike_ts,
            lock_ts: strike_ts + config.lock_offset,
            settle_ts: strike_ts + ORACLE_INTERVAL,
            pool_above: 0,
            pool_below: 0,
            shares_above: 0,
            shares_below: 0,
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

        // Time-weighted shares: bet early → more shares → larger payout share
        let shares = compute_shares(amount, now, round.strike_ts, round.lock_ts);

        // Transfer XLM from user to contract
        token::TokenClient::new(e, &config.token)
            .transfer(&user, &e.current_contract_address(), &amount);

        match side {
            Side::Above => {
                round.pool_above = round.pool_above.checked_add(amount).expect("pool overflow");
                round.shares_above = round.shares_above.checked_add(shares).expect("shares overflow");
            }
            Side::Below => {
                round.pool_below = round.pool_below.checked_add(amount).expect("pool overflow");
                round.shares_below = round.shares_below.checked_add(shares).expect("shares overflow");
            }
        }
        save_round(e, &round);

        let pos = Position { round_id, side, amount, shares, claimed: false };
        save_position(e, round_id, &user, &pos);

        e.events().publish(
            (symbol_short!("bet"), round_id),
            (user, side, amount, shares, round.pool_above, round.pool_below),
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

        // Pinned to settle_ts — outcome is identical regardless of who calls settle() or when.
        let price_data = oracle.price(&xlm_asset(&e), &round.settle_ts);

        let outcome = match price_data {
            None => Outcome::Void,
            Some(p) => {
                round.settle_price = p.price;
                if p.price > round.strike { Outcome::Above }
                else if p.price < round.strike { Outcome::Below }
                else { Outcome::Void }
            }
        };

        // One-sided pool → Void (no counterparty)
        let outcome = if round.pool_above == 0 || round.pool_below == 0 {
            Outcome::Void
        } else {
            outcome
        };

        // Collect fee from losing pool on resolved rounds
        if outcome != Outcome::Void {
            let losing_pool = if outcome == Outcome::Above {
                round.pool_below
            } else {
                round.pool_above
            };
            let fee = losing_pool
                .checked_mul(config.fee_bps as i128).expect("fee mul")
                .checked_div(10_000).expect("fee div");
            add_fees(&e, fee);
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
            pos.amount // gross refund — no fee on void
        } else {
            let on_winning_side = matches!(
                (pos.side, round.outcome),
                (Side::Above, Outcome::Above) | (Side::Below, Outcome::Below)
            );
            if !on_winning_side {
                pos.claimed = true;
                e.storage().temporary().set(&key, &pos);
                return 0; // loser — mark claimed so they can't retry
            }
            let (winning_shares, losing_pool) = if round.outcome == Outcome::Above {
                (round.shares_above, round.pool_below)
            } else {
                (round.shares_below, round.pool_above)
            };
            compute_payout(pos.shares, winning_shares, losing_pool, config.fee_bps)
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
        // Derive Locked status at read time (no extra tx needed)
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
        config.lock_offset = seconds;
        e.storage().instance().set(&Key::Config, &config);
        bump_instance(&e);
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
    enum MockKey { Prices }

    #[soroban_sdk::contract]
    struct MockOracle;

    #[soroban_sdk::contractimpl]
    impl MockOracle {
        pub fn set_price(e: Env, ts: u64, price: i128) {
            let mut m: soroban_sdk::Map<u64, i128> = e
                .storage().instance().get(&MockKey::Prices)
                .unwrap_or_else(|| soroban_sdk::map![&e]);
            m.set(ts, price);
            e.storage().instance().set(&MockKey::Prices, &m);
        }

        pub fn lastprice(e: Env, _asset: OracleAsset) -> Option<PriceData> {
            let m: soroban_sdk::Map<u64, i128> = e
                .storage().instance().get(&MockKey::Prices)
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
                .storage().instance().get(&MockKey::Prices)
                .unwrap_or_else(|| soroban_sdk::map![&e]);
            m.get(timestamp).map(|p| PriceData { price: p, timestamp })
        }

        pub fn decimals(_e: Env) -> u32 { 14 }
    }

    // ── Test harness ──────────────────────────────────────────────────────────

    const LOCK_OFFSET: u64 = 180;
    const FEE_BPS: u32 = 200;
    const MIN_BET: i128 = 100_000_000; // 10 XLM
    const T0: u64 = 1_000_000; // base timestamp (epoch-aligned)
    const STRIKE: i128 = 13_000_000_000_000; // $0.13 with 14 decimals
    const PRICE_UP: i128 = 14_000_000_000_000;   // $0.14 — above
    const PRICE_DOWN: i128 = 12_000_000_000_000; // $0.12 — below

    struct Ctx {
        env: Env,
        contract: Address,
        oracle: Address,
        token: Address,
        admin: Address,
    }

    impl Ctx {
        fn new() -> Self {
            let env = Env::default();
            env.mock_all_auths();
            env.ledger().set_timestamp(T0);

            let admin = Address::generate(&env);
            let oracle = env.register(MockOracle, ());
            let token = env.register_stellar_asset_contract_v2(admin.clone()).address();
            let contract = env.register(WickPredict, ());

            WickPredictClient::new(&env, &contract).initialize(
                &admin, &oracle, &token, &FEE_BPS, &MIN_BET, &LOCK_OFFSET,
            );
            Ctx { env, contract, oracle, token, admin }
        }

        fn client(&self) -> WickPredictClient { WickPredictClient::new(&self.env, &self.contract) }
        fn oracle_client(&self) -> MockOracleClient { MockOracleClient::new(&self.env, &self.oracle) }
        fn mint(&self, to: &Address, amt: i128) {
            token::StellarAssetClient::new(&self.env, &self.token).mint(to, &amt);
        }
        fn balance(&self, addr: &Address) -> i128 {
            token::TokenClient::new(&self.env, &self.token).balance(addr)
        }
        fn now(&self) -> u64 { self.env.ledger().timestamp() }
        fn advance(&self, secs: u64) {
            self.env.ledger().set_timestamp(self.now() + secs);
        }
        /// Seed oracle with a strike, create a round, return round_id.
        fn open_round(&self, strike: i128) -> u64 {
            self.oracle_client().set_price(&T0, &strike);
            self.client().create_round()
        }
        /// Advance to settle_ts, set settle price, call settle().
        fn settle_round(&self, id: u64, settle_price: i128) {
            let round = self.client().get_round(&id);
            self.oracle_client().set_price(&round.settle_ts, &settle_price);
            self.env.ledger().set_timestamp(round.settle_ts);
            self.client().settle(&id);
        }
    }

    macro_rules! expect_err {
        ($res:expr, $err:expr) => {
            match $res {
                Err(Ok(e)) => assert_eq!(e, Into::<soroban_sdk::Error>::into($err)),
                other => panic!("expected {:?}, got {:?}", $err, other),
            }
        };
    }

    // ── Pure math tests ───────────────────────────────────────────────────────

    #[test]
    fn test_shares_early_bet() {
        // Bet at t=0 → full shares (1000/1000 = 1.0x)
        let shares = compute_shares(1_000_000_000, T0, T0, T0 + LOCK_OFFSET);
        assert_eq!(shares, 1_000_000_000);
    }

    #[test]
    fn test_shares_late_bet() {
        // Bet at t=lock_ts → half shares (500/1000 = 0.5x)
        let bet_ts = T0 + LOCK_OFFSET;
        let shares = compute_shares(1_000_000_000, bet_ts, T0, bet_ts);
        assert_eq!(shares, 500_000_000);
    }

    #[test]
    fn test_shares_midpoint() {
        // Bet at t=lock/2 → 750/1000 = 0.75x
        let shares = compute_shares(1_000_000_000, T0 + LOCK_OFFSET / 2, T0, T0 + LOCK_OFFSET);
        assert_eq!(shares, 750_000_000);
    }

    #[test]
    fn test_early_bettor_earns_more_than_late() {
        // Both bet same amount; early bet should produce higher payout share.
        let amount = 1_000_000_000i128;
        let shares_early = compute_shares(amount, T0, T0, T0 + LOCK_OFFSET);
        let shares_late = compute_shares(amount, T0 + LOCK_OFFSET - 1, T0, T0 + LOCK_OFFSET);
        assert!(shares_early > shares_late, "early should get more shares");

        // Total winning shares = shares_early + shares_late.
        // Losing pool = 1000 XLM (some bettor on opposite side).
        let losing_pool = 1_000_000_000i128;
        let total = shares_early + shares_late;
        let p_early = compute_payout(shares_early, total, losing_pool, FEE_BPS);
        let p_late  = compute_payout(shares_late,  total, losing_pool, FEE_BPS);
        assert!(p_early > p_late, "early bettor should earn more");
    }

    #[test]
    fn test_payout_conservation() {
        // sum(payouts) + fee ≤ losing_pool  (no over-payment)
        let losing_pool = 5_000_000_000i128;
        let s1 = compute_shares(1_000_000_000, T0,              T0, T0 + LOCK_OFFSET);
        let s2 = compute_shares(2_000_000_000, T0 + 60,         T0, T0 + LOCK_OFFSET);
        let s3 = compute_shares(1_500_000_000, T0 + LOCK_OFFSET / 2, T0, T0 + LOCK_OFFSET);
        let total_shares = s1 + s2 + s3;

        let p1 = compute_payout(s1, total_shares, losing_pool, FEE_BPS);
        let p2 = compute_payout(s2, total_shares, losing_pool, FEE_BPS);
        let p3 = compute_payout(s3, total_shares, losing_pool, FEE_BPS);
        let fee = losing_pool * FEE_BPS as i128 / 10_000;

        let sum = p1 + p2 + p3;
        assert!(sum + fee <= losing_pool,
            "conservation violated: sum={sum} fee={fee} losing={losing_pool}");
        // Dust should be tiny
        let dust = losing_pool - sum - fee;
        assert!(dust < total_shares, "unexpected dust: {dust}");
    }

    // ── Lifecycle tests ───────────────────────────────────────────────────────

    #[test]
    fn test_happy_path_above_wins() {
        let ctx = Ctx::new();
        let id = ctx.open_round(STRIKE);

        let alice = Address::generate(&ctx.env);
        let bob = Address::generate(&ctx.env);
        ctx.mint(&alice, 5_000_000_000);
        ctx.mint(&bob,   2_000_000_000);

        // Alice bets above at t=0 (max shares)
        ctx.client().bet_above(&alice, &id, &5_000_000_000);
        // Bob bets below at t=60s (slightly reduced shares)
        ctx.advance(60);
        ctx.client().bet_below(&bob, &id, &2_000_000_000);

        ctx.settle_round(id, PRICE_UP); // above wins

        let round = ctx.client().get_round(&id);
        assert_eq!(round.outcome, Outcome::Above);
        assert_eq!(round.status, Status::Settled);

        // Alice (above) can claim; Bob (below) gets 0
        let alice_bal_before = ctx.balance(&alice);
        let payout = ctx.client().claim(&alice, &id);
        assert!(payout > 0, "alice should win");
        assert_eq!(ctx.balance(&alice) - alice_bal_before, payout);

        let bob_payout = ctx.client().claim(&bob, &id);
        assert_eq!(bob_payout, 0, "bob lost");
    }

    #[test]
    fn test_happy_path_below_wins() {
        let ctx = Ctx::new();
        let id = ctx.open_round(STRIKE);

        let alice = Address::generate(&ctx.env);
        let bob = Address::generate(&ctx.env);
        ctx.mint(&alice, 3_000_000_000);
        ctx.mint(&bob,   6_000_000_000);

        ctx.client().bet_above(&alice, &id, &3_000_000_000);
        ctx.advance(30);
        ctx.client().bet_below(&bob, &id, &6_000_000_000);

        ctx.settle_round(id, PRICE_DOWN); // below wins

        assert_eq!(ctx.client().get_round(&id).outcome, Outcome::Below);
        let payout = ctx.client().claim(&bob, &id);
        assert!(payout > 0);
        assert_eq!(ctx.client().claim(&alice, &id), 0);
    }

    #[test]
    fn test_void_empty_pool() {
        let ctx = Ctx::new();
        let id = ctx.open_round(STRIKE);
        let alice = Address::generate(&ctx.env);
        ctx.mint(&alice, 5_000_000_000);
        ctx.client().bet_above(&alice, &id, &5_000_000_000);
        // No one bets below → void
        ctx.settle_round(id, PRICE_UP);
        assert_eq!(ctx.client().get_round(&id).outcome, Outcome::Void);
        // Full gross refund
        let refund = ctx.client().claim(&alice, &id);
        assert_eq!(refund, 5_000_000_000);
    }

    #[test]
    fn test_void_exact_tie() {
        let ctx = Ctx::new();
        let id = ctx.open_round(STRIKE);
        let alice = Address::generate(&ctx.env);
        let bob = Address::generate(&ctx.env);
        ctx.mint(&alice, 3_000_000_000);
        ctx.mint(&bob, 3_000_000_000);
        ctx.client().bet_above(&alice, &id, &3_000_000_000);
        ctx.client().bet_below(&bob, &id, &3_000_000_000);
        ctx.settle_round(id, STRIKE); // settle price == strike → tie
        assert_eq!(ctx.client().get_round(&id).outcome, Outcome::Void);
        assert_eq!(ctx.client().claim(&alice, &id), 3_000_000_000);
        assert_eq!(ctx.client().claim(&bob, &id), 3_000_000_000);
    }

    #[test]
    fn test_void_oracle_gap() {
        let ctx = Ctx::new();
        let id = ctx.open_round(STRIKE);
        let alice = Address::generate(&ctx.env);
        let bob = Address::generate(&ctx.env);
        ctx.mint(&alice, 2_000_000_000);
        ctx.mint(&bob, 2_000_000_000);
        ctx.client().bet_above(&alice, &id, &2_000_000_000);
        ctx.client().bet_below(&bob, &id, &2_000_000_000);
        // Do NOT set settle price → oracle returns None → void
        let round = ctx.client().get_round(&id);
        ctx.env.ledger().set_timestamp(round.settle_ts);
        ctx.client().settle(&id);
        assert_eq!(ctx.client().get_round(&id).outcome, Outcome::Void);
        assert_eq!(ctx.client().claim(&alice, &id), 2_000_000_000);
        assert_eq!(ctx.client().claim(&bob, &id), 2_000_000_000);
    }

    #[test]
    fn test_bet_after_lock_rejected() {
        let ctx = Ctx::new();
        let id = ctx.open_round(STRIKE);
        ctx.advance(LOCK_OFFSET); // exactly at lock_ts → rejected
        let alice = Address::generate(&ctx.env);
        ctx.mint(&alice, 1_000_000_000);
        expect_err!(ctx.client().try_bet_above(&alice, &id, &1_000_000_000), Error::RoundLocked);
    }

    #[test]
    fn test_bet_below_minimum_rejected() {
        let ctx = Ctx::new();
        let id = ctx.open_round(STRIKE);
        let alice = Address::generate(&ctx.env);
        ctx.mint(&alice, 1_000_000_000);
        expect_err!(
            ctx.client().try_bet_above(&alice, &id, &(MIN_BET - 1)),
            Error::BetTooSmall
        );
    }

    #[test]
    fn test_double_bet_rejected() {
        let ctx = Ctx::new();
        let id = ctx.open_round(STRIKE);
        let alice = Address::generate(&ctx.env);
        ctx.mint(&alice, 3_000_000_000);
        ctx.client().bet_above(&alice, &id, &1_000_000_000);
        expect_err!(ctx.client().try_bet_above(&alice, &id, &1_000_000_000), Error::AlreadyBet);
    }

    #[test]
    fn test_double_claim_rejected() {
        let ctx = Ctx::new();
        let id = ctx.open_round(STRIKE);
        let alice = Address::generate(&ctx.env);
        let bob = Address::generate(&ctx.env);
        ctx.mint(&alice, 3_000_000_000);
        ctx.mint(&bob, 2_000_000_000);
        ctx.client().bet_above(&alice, &id, &3_000_000_000);
        ctx.client().bet_below(&bob, &id, &2_000_000_000);
        ctx.settle_round(id, PRICE_UP);
        ctx.client().claim(&alice, &id);
        expect_err!(ctx.client().try_claim(&alice, &id), Error::NothingToClaim);
    }

    #[test]
    fn test_settle_too_early_rejected() {
        let ctx = Ctx::new();
        let id = ctx.open_round(STRIKE);
        expect_err!(ctx.client().try_settle(&id), Error::TooEarly);
    }

    #[test]
    fn test_settle_idempotent() {
        let ctx = Ctx::new();
        let id = ctx.open_round(STRIKE);
        let alice = Address::generate(&ctx.env);
        let bob = Address::generate(&ctx.env);
        ctx.mint(&alice, 2_000_000_000);
        ctx.mint(&bob, 2_000_000_000);
        ctx.client().bet_above(&alice, &id, &2_000_000_000);
        ctx.client().bet_below(&bob, &id, &2_000_000_000);
        ctx.settle_round(id, PRICE_UP);
        expect_err!(ctx.client().try_settle(&id), Error::AlreadySettled);
    }

    #[test]
    fn test_duplicate_round_rejected() {
        let ctx = Ctx::new();
        ctx.open_round(STRIKE); // first call sets LastStrikeTs = T0
        // Same oracle tick → DuplicateRound
        expect_err!(ctx.client().try_create_round(), Error::DuplicateRound);
    }

    #[test]
    fn test_contract_level_conservation() {
        // sum(all payouts) + fee ≤ total staked — contract must never overpay
        let ctx = Ctx::new();
        let id = ctx.open_round(STRIKE);

        // (address, side, amount, absolute_offset_from_T0)
        let users = [
            (Address::generate(&ctx.env), Side::Above, 2_000_000_000i128, 0u64),
            (Address::generate(&ctx.env), Side::Above, 1_500_000_000i128, 60u64),
            (Address::generate(&ctx.env), Side::Above,   800_000_000i128, 120u64),
            (Address::generate(&ctx.env), Side::Below, 3_000_000_000i128, 30u64),
            (Address::generate(&ctx.env), Side::Below, 1_200_000_000i128, 90u64),
        ];

        let total_staked: i128 = users.iter().map(|(_, _, a, _)| *a).sum();
        for (addr, side, amt, offset) in &users {
            ctx.mint(addr, *amt);
            // Use absolute offset from T0 so no user exceeds lock_ts (180s)
            ctx.env.ledger().set_timestamp(T0 + offset);
            match side {
                Side::Above => ctx.client().bet_above(addr, &id, amt),
                Side::Below => ctx.client().bet_below(addr, &id, amt),
            }
        }

        ctx.settle_round(id, PRICE_UP); // Above wins

        let contract_before = ctx.balance(&ctx.contract);
        let mut total_paid: i128 = 0;
        for (addr, _, _, _) in &users {
            total_paid += ctx.client().claim(addr, &id);
        }
        let contract_after = ctx.balance(&ctx.contract);

        assert!(contract_after >= 0, "contract went negative");
        assert!(total_paid <= total_staked, "overpaid: {total_paid} > {total_staked}");

        // Fee should remain in contract
        let round = ctx.client().get_round(&id);
        let losing_pool = round.pool_above; // below won, above loses... wait
        // Above won, so below pool is the losing pool
        let losing_pool = round.pool_below;
        let fee = losing_pool * FEE_BPS as i128 / 10_000;
        assert!(contract_after >= fee - 1, "fee not retained"); // -1 for dust
    }
}
