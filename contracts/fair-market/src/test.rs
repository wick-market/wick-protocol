use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    token, vec, Address, Env, Symbol,
};

use crate::{
    oracle::mock::{MockOracle, MockOracleClient},
    oracle::OracleAsset,
    types::{Outcome, Side, Status},
    Error, FairMarket, FairMarketClient,
};

// ── Constants ──────────────────────────────────────────────────────────────────

const FEE_BPS: u32 = 200; // 2%
const MIN_BET: i128 = 100_000_000; // 10 XLM
const LOCK_OFFSET: u64 = 180; // 3 minutes

// Prices use 14 decimal places (Reflector standard).
const P0: i128 = 50_000_000_000_000_000_000i128; // $50,000 BTC
const P_UP: i128 = 51_000_000_000_000_000_000i128; // $51,000 — UP wins
const P_DOWN: i128 = 49_000_000_000_000_000_000i128; // $49,000 — DOWN wins
const P_SAME: i128 = P0; // exact tie

const ORACLE_INTERVAL: u64 = 300;
const T0: u64 = 1_000 * ORACLE_INTERVAL; // arbitrary epoch-aligned start

// ── Test harness ───────────────────────────────────────────────────────────────

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
        let token_admin = Address::generate(&env);
        let token = env
            .register_stellar_asset_contract_v2(token_admin.clone())
            .address();

        let contract = env.register(FairMarket, ());
        let client = FairMarketClient::new(&env, &contract);
        client.initialize(
            &admin,
            &oracle,
            &token,
            &FEE_BPS,
            &MIN_BET,
            &LOCK_OFFSET,
        );
        Ctx { env, contract, oracle, token, admin }
    }

    fn client(&self) -> FairMarketClient {
        FairMarketClient::new(&self.env, &self.contract)
    }

    fn oracle(&self) -> MockOracleClient {
        MockOracleClient::new(&self.env, &self.oracle)
    }

    fn mint(&self, to: &Address, amount: i128) {
        token::StellarAssetClient::new(&self.env, &self.token).mint(to, &amount);
    }

    fn balance(&self, addr: &Address) -> i128 {
        token::TokenClient::new(&self.env, &self.token).balance(addr)
    }

    fn set_price(&self, asset: &str, ts: u64, price: i128) {
        self.oracle().set_price(
            &OracleAsset::Other(Symbol::new(&self.env, asset)),
            &ts,
            &price,
        );
    }

    /// Seed the oracle so create_round("BTC") succeeds at the current ledger time.
    /// Returns the oracle tick timestamp used as strike_ts.
    fn seed_btc(&self, price: i128) -> u64 {
        let ts = self.env.ledger().timestamp();
        // Oracle stores prices at 5-min boundaries; we use current time as the tick.
        self.set_price("BTC", ts, price);
        ts
    }

    fn now(&self) -> u64 {
        self.env.ledger().timestamp()
    }

    fn advance(&self, secs: u64) {
        self.env.ledger().set_timestamp(self.now() + secs);
    }
}

// Error matcher — contracterror variants need .into() for comparison.
macro_rules! expect_err {
    ($res:expr, $err:expr) => {
        match $res {
            Err(Ok(e)) => assert_eq!(e, Into::<soroban_sdk::Error>::into($err)),
            other => panic!("expected {:?}, got {:?}", $err, other),
        }
    };
}

// ═══════════════════════════════════════════════════════════════════════════════
// HAPPY PATH
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_happy_path_up_wins() {
    let ctx = Ctx::new();
    let strike_ts = ctx.seed_btc(P0);
    let round_id = ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

    let alice = Address::generate(&ctx.env);
    let bob = Address::generate(&ctx.env);
    ctx.mint(&alice, 1_000_000_000);
    ctx.mint(&bob, 1_000_000_000);

    ctx.client().bet(&alice, &round_id, &Side::Up, &500_000_000);
    ctx.client().bet(&bob, &round_id, &Side::Down, &300_000_000);

    // Advance to settle_ts and provide the settlement price.
    let settle_ts = strike_ts + ORACLE_INTERVAL;
    ctx.advance(ORACLE_INTERVAL);
    ctx.set_price("BTC", settle_ts, P_UP);

    ctx.client().settle(&round_id);
    let round = ctx.client().get_round(&round_id);
    assert_eq!(round.status, Status::Settled);
    assert_eq!(round.outcome, Outcome::Up);

    // total=800, fee=16, distributed=784
    // alice's payout = 500 * 784 / 500 = 784
    let alice_before = ctx.balance(&alice);
    let payout = ctx.client().claim(&alice, &round_id);
    assert_eq!(payout, 784_000_000);
    assert_eq!(ctx.balance(&alice) - alice_before, 784_000_000);

    // Bob lost — claim returns 0 but succeeds (marks claimed).
    let bob_payout = ctx.client().claim(&bob, &round_id);
    assert_eq!(bob_payout, 0);
}

#[test]
fn test_happy_path_down_wins() {
    let ctx = Ctx::new();
    let strike_ts = ctx.seed_btc(P0);
    let round_id = ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

    let alice = Address::generate(&ctx.env);
    let bob = Address::generate(&ctx.env);
    ctx.mint(&alice, 1_000_000_000);
    ctx.mint(&bob, 1_000_000_000);

    ctx.client().bet(&alice, &round_id, &Side::Up, &300_000_000);
    ctx.client().bet(&bob, &round_id, &Side::Down, &600_000_000);

    let settle_ts = strike_ts + ORACLE_INTERVAL;
    ctx.advance(ORACLE_INTERVAL);
    ctx.set_price("BTC", settle_ts, P_DOWN);
    ctx.client().settle(&round_id);

    let round = ctx.client().get_round(&round_id);
    assert_eq!(round.outcome, Outcome::Down);

    // total=900, fee=18, distributed=882
    // bob's payout = 600 * 882 / 600 = 882
    let payout = ctx.client().claim(&bob, &round_id);
    assert_eq!(payout, 882_000_000);
    assert_eq!(ctx.client().claim(&alice, &round_id), 0);
}

// ═══════════════════════════════════════════════════════════════════════════════
// VOID CONDITIONS (all three must produce GROSS refund with NO fee)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_void_empty_pool_up_only() {
    let ctx = Ctx::new();
    let strike_ts = ctx.seed_btc(P0);
    let round_id = ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

    let alice = Address::generate(&ctx.env);
    ctx.mint(&alice, 500_000_000);
    ctx.client().bet(&alice, &round_id, &Side::Up, &500_000_000);

    let settle_ts = strike_ts + ORACLE_INTERVAL;
    ctx.advance(ORACLE_INTERVAL);
    ctx.set_price("BTC", settle_ts, P_UP);
    ctx.client().settle(&round_id);

    assert_eq!(ctx.client().get_round(&round_id).outcome, Outcome::Void);

    // Gross refund — full 500_000_000 back, no fee deducted.
    let payout = ctx.client().claim(&alice, &round_id);
    assert_eq!(payout, 500_000_000);
}

#[test]
fn test_void_empty_pool_down_only() {
    let ctx = Ctx::new();
    let strike_ts = ctx.seed_btc(P0);
    let round_id = ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

    let bob = Address::generate(&ctx.env);
    ctx.mint(&bob, 400_000_000);
    ctx.client().bet(&bob, &round_id, &Side::Down, &400_000_000);

    ctx.advance(ORACLE_INTERVAL);
    ctx.set_price("BTC", strike_ts + ORACLE_INTERVAL, P_DOWN);
    ctx.client().settle(&round_id);

    assert_eq!(ctx.client().get_round(&round_id).outcome, Outcome::Void);
    assert_eq!(ctx.client().claim(&bob, &round_id), 400_000_000); // gross refund
}

#[test]
fn test_void_exact_tie() {
    let ctx = Ctx::new();
    let strike_ts = ctx.seed_btc(P0);
    let round_id = ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

    let alice = Address::generate(&ctx.env);
    let bob = Address::generate(&ctx.env);
    ctx.mint(&alice, 500_000_000);
    ctx.mint(&bob, 500_000_000);
    ctx.client().bet(&alice, &round_id, &Side::Up, &500_000_000);
    ctx.client().bet(&bob, &round_id, &Side::Down, &500_000_000);

    ctx.advance(ORACLE_INTERVAL);
    // settle_price == strike → exact tie → void
    ctx.set_price("BTC", strike_ts + ORACLE_INTERVAL, P_SAME);
    ctx.client().settle(&round_id);

    let round = ctx.client().get_round(&round_id);
    assert_eq!(round.outcome, Outcome::Void);
    assert_eq!(round.settle_price, P_SAME);

    // Both get gross refunds, no fee.
    assert_eq!(ctx.client().claim(&alice, &round_id), 500_000_000);
    assert_eq!(ctx.client().claim(&bob, &round_id), 500_000_000);
}

#[test]
fn test_void_oracle_returns_none() {
    let ctx = Ctx::new();
    let strike_ts = ctx.seed_btc(P0);
    let round_id = ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

    let alice = Address::generate(&ctx.env);
    ctx.mint(&alice, 500_000_000);
    ctx.client().bet(&alice, &round_id, &Side::Up, &500_000_000);
    let bob = Address::generate(&ctx.env);
    ctx.mint(&bob, 300_000_000);
    ctx.client().bet(&bob, &round_id, &Side::Down, &300_000_000);

    // Do NOT set settle price → oracle returns None → void.
    ctx.advance(ORACLE_INTERVAL);
    ctx.client().settle(&round_id);

    assert_eq!(ctx.client().get_round(&round_id).outcome, Outcome::Void);
    assert_eq!(ctx.client().claim(&alice, &round_id), 500_000_000);
    assert_eq!(ctx.client().claim(&bob, &round_id), 300_000_000);
}

// ═══════════════════════════════════════════════════════════════════════════════
// BETTING GUARDS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_bet_rejected_after_lock_ts() {
    let ctx = Ctx::new();
    ctx.seed_btc(P0);
    let round_id = ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

    // Advance past lock_ts (strike_ts + 180).
    ctx.advance(LOCK_OFFSET + 1);

    let alice = Address::generate(&ctx.env);
    ctx.mint(&alice, 500_000_000);
    let res = ctx.client().try_bet(&alice, &round_id, &Side::Up, &500_000_000);
    expect_err!(res, Error::RoundLocked);
}

#[test]
fn test_bet_at_exactly_lock_ts_rejected() {
    let ctx = Ctx::new();
    let strike_ts = ctx.seed_btc(P0);
    let round_id = ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

    // Advance to exactly lock_ts.
    ctx.env.ledger().set_timestamp(strike_ts + LOCK_OFFSET);

    let alice = Address::generate(&ctx.env);
    ctx.mint(&alice, 500_000_000);
    let res = ctx.client().try_bet(&alice, &round_id, &Side::Up, &500_000_000);
    expect_err!(res, Error::RoundLocked);
}

#[test]
fn test_bet_below_minimum_rejected() {
    let ctx = Ctx::new();
    ctx.seed_btc(P0);
    let round_id = ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

    let alice = Address::generate(&ctx.env);
    ctx.mint(&alice, 500_000_000);
    let res = ctx.client().try_bet(&alice, &round_id, &Side::Up, &(MIN_BET - 1));
    expect_err!(res, Error::BetTooSmall);
}

#[test]
fn test_double_bet_rejected() {
    let ctx = Ctx::new();
    ctx.seed_btc(P0);
    let round_id = ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

    let alice = Address::generate(&ctx.env);
    ctx.mint(&alice, 1_000_000_000);
    ctx.client().bet(&alice, &round_id, &Side::Up, &MIN_BET);

    let res = ctx.client().try_bet(&alice, &round_id, &Side::Up, &MIN_BET);
    expect_err!(res, Error::AlreadyBet);
}

// ═══════════════════════════════════════════════════════════════════════════════
// SETTLEMENT GUARDS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_settle_too_early_rejected() {
    let ctx = Ctx::new();
    ctx.seed_btc(P0);
    let round_id = ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

    let res = ctx.client().try_settle(&round_id);
    expect_err!(res, Error::TooEarly);
}

#[test]
fn test_settle_idempotent_second_call_fails() {
    let ctx = Ctx::new();
    let strike_ts = ctx.seed_btc(P0);
    let round_id = ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

    let alice = Address::generate(&ctx.env);
    let bob = Address::generate(&ctx.env);
    ctx.mint(&alice, 500_000_000);
    ctx.mint(&bob, 500_000_000);
    ctx.client().bet(&alice, &round_id, &Side::Up, &500_000_000);
    ctx.client().bet(&bob, &round_id, &Side::Down, &500_000_000);

    ctx.advance(ORACLE_INTERVAL);
    ctx.set_price("BTC", strike_ts + ORACLE_INTERVAL, P_UP);
    ctx.client().settle(&round_id);

    let res = ctx.client().try_settle(&round_id);
    expect_err!(res, Error::AlreadySettled);
}

// ── Key security property ─────────────────────────────────────────────────────
//
// Two different callers settling at t=settle_ts and t=settle_ts+1000 must
// produce the IDENTICAL outcome.  This is what makes settlement permissionless:
// nobody can influence the result by timing their settle() call.
// The test runs two independent envs and compares outcomes.

#[test]
fn test_settle_outcome_independent_of_caller_timing() {
    let settle_at = |delay: u64| -> (Outcome, i128) {
        let ctx = Ctx::new();
        let strike_ts = ctx.seed_btc(P0);
        let round_id = ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

        let alice = Address::generate(&ctx.env);
        let bob = Address::generate(&ctx.env);
        ctx.mint(&alice, 500_000_000);
        ctx.mint(&bob, 300_000_000);
        ctx.client().bet(&alice, &round_id, &Side::Up, &500_000_000);
        ctx.client().bet(&bob, &round_id, &Side::Down, &300_000_000);

        let settle_ts = strike_ts + ORACLE_INTERVAL;
        ctx.set_price("BTC", settle_ts, P_UP);

        // Caller settles after `delay` seconds past settle_ts.
        ctx.env.ledger().set_timestamp(settle_ts + delay);
        ctx.client().settle(&round_id);
        let round = ctx.client().get_round(&round_id);
        (round.outcome, round.settle_price)
    };

    // Early settler vs late settler — must be identical.
    let (o1, p1) = settle_at(0);
    let (o2, p2) = settle_at(1000);
    assert_eq!(o1, o2, "outcome differed by caller timing");
    assert_eq!(p1, p2, "settle_price differed by caller timing");
}

// ═══════════════════════════════════════════════════════════════════════════════
// CLAIM GUARDS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_double_claim_rejected() {
    let ctx = Ctx::new();
    let strike_ts = ctx.seed_btc(P0);
    let round_id = ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

    let alice = Address::generate(&ctx.env);
    let bob = Address::generate(&ctx.env);
    ctx.mint(&alice, 500_000_000);
    ctx.mint(&bob, 300_000_000);
    ctx.client().bet(&alice, &round_id, &Side::Up, &500_000_000);
    ctx.client().bet(&bob, &round_id, &Side::Down, &300_000_000);

    ctx.advance(ORACLE_INTERVAL);
    ctx.set_price("BTC", strike_ts + ORACLE_INTERVAL, P_UP);
    ctx.client().settle(&round_id);

    ctx.client().claim(&alice, &round_id);
    // Second claim: position is marked claimed → returns 0 (no panic, just no payout).
    let second = ctx.client().claim(&alice, &round_id);
    assert_eq!(second, 0);
}

#[test]
fn test_claim_many_works() {
    let ctx = Ctx::new();
    let alice = Address::generate(&ctx.env);
    ctx.mint(&alice, 10_000_000_000);

    let mut ids = [0u64; 3];
    for (i, price) in [P_UP, P_DOWN, P_UP].iter().enumerate() {
        // Each round needs a fresh oracle tick. Advance by ORACLE_INTERVAL.
        ctx.advance(ORACLE_INTERVAL);
        let strike_ts = ctx.now();
        ctx.set_price("BTC", strike_ts, P0);
        let rid = ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));
        ids[i] = rid;

        let bob = Address::generate(&ctx.env);
        ctx.mint(&bob, 300_000_000);
        ctx.client().bet(&alice, &rid, &Side::Up, &500_000_000);
        ctx.client().bet(&bob, &rid, &Side::Down, &300_000_000);

        let settle_ts = strike_ts + ORACLE_INTERVAL;
        ctx.advance(ORACLE_INTERVAL);
        ctx.set_price("BTC", settle_ts, *price);
        ctx.client().settle(&rid);
    }

    let total = ctx.client().claim_many(&alice, &vec![&ctx.env, ids[0], ids[1], ids[2]]);
    assert!(total > 0, "claim_many returned 0");
}

#[test]
fn test_claim_many_batch_too_large_rejected() {
    let ctx = Ctx::new();
    let alice = Address::generate(&ctx.env);
    // 21 ids — one over the cap of 20
    // Build a Vec<u64> of 21 IDs using soroban_sdk::Vec (no_std crate).
    let mut sdk_vec = soroban_sdk::Vec::new(&ctx.env);
    for i in 1u64..=21 {
        sdk_vec.push_back(i);
    }
    let res = ctx.client().try_claim_many(&alice, &sdk_vec);
    expect_err!(res, Error::ClaimBatchTooLarge);
}

// ═══════════════════════════════════════════════════════════════════════════════
// POOL CONSERVATION INVARIANT
// sum(all_payouts) + fee <= total_staked   (never over-pays)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_pool_conservation() {
    let ctx = Ctx::new();
    let strike_ts = ctx.seed_btc(P0);
    let round_id = ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

    let bettors = [
        (Address::generate(&ctx.env), Side::Up, 100_000_000i128),
        (Address::generate(&ctx.env), Side::Up, 200_000_000i128),
        (Address::generate(&ctx.env), Side::Up, 300_000_000i128),
        (Address::generate(&ctx.env), Side::Down, 150_000_000i128),
        (Address::generate(&ctx.env), Side::Down, 250_000_000i128),
    ];

    let total_staked: i128 = bettors.iter().map(|(_, _, a)| *a).sum();
    for (addr, side, amount) in &bettors {
        ctx.mint(addr, *amount);
        ctx.client().bet(addr, &round_id, side, amount);
    }

    // UP wins.
    ctx.advance(ORACLE_INTERVAL);
    ctx.set_price("BTC", strike_ts + ORACLE_INTERVAL, P_UP);
    ctx.client().settle(&round_id);

    let contract_before = ctx.balance(&ctx.contract);

    let mut total_paid: i128 = 0;
    for (addr, _, _) in &bettors {
        total_paid += ctx.client().claim(addr, &round_id);
    }

    let contract_after = ctx.balance(&ctx.contract);

    // Contract must never go below zero.
    assert!(contract_after >= 0, "contract balance went negative");

    // Total paid out must not exceed what was staked.
    assert!(
        total_paid <= total_staked,
        "overpaid: paid={total_paid} staked={total_staked}"
    );

    // Remainder in contract = fee + dust.
    let remainder = contract_before - (contract_before - contract_after);
    let fee = total_staked * FEE_BPS as i128 / 10_000;
    assert!(
        contract_after >= fee,
        "contract holds less than fee: contract_after={contract_after} fee={fee}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// ADMIN GUARDS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_set_fee_bps_max_enforced() {
    let ctx = Ctx::new();
    let res = ctx.client().try_set_fee_bps(&501);
    expect_err!(res, Error::FeeTooHigh);
}

#[test]
fn test_set_fee_bps_at_max_succeeds() {
    let ctx = Ctx::new();
    ctx.client().set_fee_bps(&500);
    assert_eq!(ctx.client().get_config().fee_bps, 500);
}

#[test]
fn test_set_lock_offset_min_enforced() {
    let ctx = Ctx::new();
    let res = ctx.client().try_set_lock_offset(&89);
    expect_err!(res, Error::LockOffsetTooSmall);
}

#[test]
fn test_set_lock_offset_at_min_succeeds() {
    let ctx = Ctx::new();
    ctx.client().set_lock_offset(&90);
    assert_eq!(ctx.client().get_config().lock_offset, 90);
}

// ═══════════════════════════════════════════════════════════════════════════════
// DEDUP & ASSET VALIDATION
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_duplicate_round_same_oracle_tick_rejected() {
    let ctx = Ctx::new();
    ctx.seed_btc(P0);
    ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

    // Same oracle tick (timestamp unchanged) — second create must fail.
    let res = ctx.client().try_create_round(&Symbol::new(&ctx.env, "BTC"));
    expect_err!(res, Error::DuplicateRound);
}

#[test]
fn test_invalid_asset_rejected() {
    let ctx = Ctx::new();
    // Unlisted assets are rejected regardless of oracle data.
    let res = ctx.client().try_create_round(&Symbol::new(&ctx.env, "DOGE"));
    expect_err!(res, Error::InvalidAsset);
}

#[test]
fn test_all_four_assets_work() {
    let ctx = Ctx::new();

    // BTC
    ctx.set_price("BTC", ctx.now(), P0);
    ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

    // ETH
    ctx.advance(ORACLE_INTERVAL);
    ctx.set_price("ETH", ctx.now(), 3_000_000_000_000_000_000i128);
    ctx.client().create_round(&Symbol::new(&ctx.env, "ETH"));

    // SOL
    ctx.advance(ORACLE_INTERVAL);
    ctx.set_price("SOL", ctx.now(), 100_000_000_000_000_000i128);
    ctx.client().create_round(&Symbol::new(&ctx.env, "SOL"));

    // XLM — included because we're building on Stellar and users expect it.
    // Pool cap to guard against thin-book manipulation is a v2 concern.
    ctx.advance(ORACLE_INTERVAL);
    ctx.set_price("XLM", ctx.now(), 130_000_000_000_000i128); // ~$0.13 with 14 decimals
    ctx.client().create_round(&Symbol::new(&ctx.env, "XLM"));
}

// ═══════════════════════════════════════════════════════════════════════════════
// ROUND STATUS DERIVATION
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_get_round_shows_locked_after_lock_ts() {
    let ctx = Ctx::new();
    let strike_ts = ctx.seed_btc(P0);
    let round_id = ctx.client().create_round(&Symbol::new(&ctx.env, "BTC"));

    let r_open = ctx.client().get_round(&round_id);
    assert_eq!(r_open.status, Status::Open);

    ctx.env.ledger().set_timestamp(strike_ts + LOCK_OFFSET);
    let r_locked = ctx.client().get_round(&round_id);
    assert_eq!(r_locked.status, Status::Locked);
}

// ═══════════════════════════════════════════════════════════════════════════════
// INITIALISATION GUARDS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_double_initialize_rejected() {
    let ctx = Ctx::new();
    let res = ctx.client().try_initialize(
        &ctx.admin,
        &ctx.oracle,
        &ctx.token,
        &FEE_BPS,
        &MIN_BET,
        &LOCK_OFFSET,
    );
    expect_err!(res, Error::AlreadyInitialized);
}

#[test]
fn test_initialize_lock_offset_below_min_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let oracle = env.register(MockOracle, ());
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let contract = env.register(FairMarket, ());
    let client = FairMarketClient::new(&env, &contract);

    let res = client.try_initialize(&admin, &oracle, &token, &FEE_BPS, &MIN_BET, &89);
    expect_err!(res, Error::LockOffsetTooSmall);
}
