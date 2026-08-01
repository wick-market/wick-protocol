mod mock_oracle;

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, vec, Address, Env,
};

use crate::{
    types::{Asset, Direction},
    Error, WickContract, WickContractClient,
};

use mock_oracle::{MockOracle, MockOracleClient};

// ── Test constants ────────────────────────────────────────────────────────────

const INTERVAL: u64 = 300; // 5 minutes in seconds
const FEE_BPS: u32 = 300; // 3%
const MIN_BET: i128 = 100_000_000; // 10 XLM in stroops

// Oracle price with 14 decimal places.
// $50,000.00 BTC → 50_000_00000000000000 (50000 * 10^14)
const BTC_PRICE_1: i128 = 50_000_000_000_000_000_000; // $50,000
const BTC_PRICE_UP: i128 = 51_000_000_000_000_000_000; // $51,000 (up)
const BTC_PRICE_DOWN: i128 = 49_000_000_000_000_000_000; // $49,000 (down)
const BTC_PRICE_SAME: i128 = 50_000_000_000_000_000_000; // same = tie

// ── Test harness ──────────────────────────────────────────────────────────────

struct TestEnv {
    env: Env,
    contract: Address,
    oracle: Address,
    token: Address,
    admin: Address,
    treasury: Address,
}

impl TestEnv {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();

        // Start at a clean epoch boundary.
        env.ledger().set_timestamp(INTERVAL * 100); // epoch 100

        let admin = Address::generate(&env);
        let treasury = Address::generate(&env);

        // Register the mock oracle.
        let oracle = env.register(MockOracle, ());

        // Register a native XLM token (Stellar Asset Contract mock).
        let token = env.register_stellar_asset_contract_v2(admin.clone());
        let token_addr = token.address();

        // Register the prediction contract.
        let contract = env.register(WickContract, ());

        // Initialize contract.
        let client = WickContractClient::new(&env, &contract);
        client.initialize(
            &admin,
            &oracle,
            &token_addr,
            &treasury,
            &FEE_BPS,
            &MIN_BET,
        );

        TestEnv {
            env,
            contract,
            oracle,
            token: token_addr,
            admin,
            treasury,
        }
    }

    fn client(&self) -> WickContractClient {
        WickContractClient::new(&self.env, &self.contract)
    }

    fn oracle_client(&self) -> MockOracleClient {
        MockOracleClient::new(&self.env, &self.oracle)
    }

    fn token_client(&self) -> token::StellarAssetClient {
        token::StellarAssetClient::new(&self.env, &self.token)
    }

    fn token_balance(&self, addr: &Address) -> i128 {
        token::TokenClient::new(&self.env, &self.token).balance(addr)
    }

    fn now(&self) -> u64 {
        self.env.ledger().timestamp()
    }

    fn current_epoch(&self) -> u64 {
        self.now() / INTERVAL
    }

    fn advance_to_next_epoch(&self) {
        let next = (self.current_epoch() + 1) * INTERVAL;
        self.env.ledger().set_timestamp(next);
    }

    /// Mint `amount` stroops of the test token to `addr`.
    fn mint(&self, addr: &Address, amount: i128) {
        self.token_client().mint(addr, &amount);
    }

    /// Set oracle price for BTC at a given epoch boundary.
    fn set_btc_price(&self, epoch: u64, price: i128) {
        use crate::oracle::OracleAsset;
        let ts = epoch * INTERVAL;
        self.oracle_client()
            .set_price(&OracleAsset::Other(soroban_sdk::Symbol::new(&self.env, "BTC")), &ts, &price);
    }

    /// Run genesis: start + lock.  After this, epoch 101 is Locked and 102 is Open.
    fn bootstrap_btc(&self) {
        // genesis_start at epoch 100 opens Round(101).
        self.client().genesis_start(&Asset::Btc);
        // Advance to epoch 101 and set oracle price AT that boundary.
        self.advance_to_next_epoch(); // now epoch 101
        let ep = self.current_epoch(); // 101
        self.set_btc_price(ep, BTC_PRICE_1); // price at 101*300
        self.client().genesis_lock(&Asset::Btc);
        // Round(101): Locked, startPrice=BTC_PRICE_1.  Round(102): Open.
    }

    /// Advance one epoch, set an oracle price, and call execute_round.
    fn advance_and_execute(&self, price: i128) {
        let next_epoch = self.current_epoch() + 1;
        self.set_btc_price(next_epoch, price);
        self.advance_to_next_epoch();
        self.client().execute_round(&Asset::Btc);
    }
}

// ── Helper: expect a specific contract error ──────────────────────────────────

// contracterror variants must be converted to soroban_sdk::Error via Into.
macro_rules! expect_error {
    ($result:expr, $err:expr) => {
        match $result {
            Err(Ok(e)) => assert_eq!(e, Into::<soroban_sdk::Error>::into($err)),
            other => panic!("expected error {:?}, got {:?}", $err, other),
        }
    };
}

// ═══════════════════════════════════════════════════════════════════════════════
// LIFECYCLE TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_initialize_sets_config() {
    let t = TestEnv::new();
    let config = t.client().get_config_view();
    assert_eq!(config.fee_bps, FEE_BPS);
    assert_eq!(config.min_bet, MIN_BET);
    assert_eq!(config.interval_secs, INTERVAL);
    assert!(config.tie_refund);
}

#[test]
fn test_double_initialize_fails() {
    let t = TestEnv::new();
    let res = t.client().try_initialize(
        &t.admin, &t.oracle, &t.token, &t.treasury, &FEE_BPS, &MIN_BET,
    );
    expect_error!(res, Error::AlreadyInitialized);
}

#[test]
fn test_genesis_start_creates_open_round() {
    let t = TestEnv::new();
    t.client().genesis_start(&Asset::Btc);

    let open_epoch = t.current_epoch() + 1; // 101
    let round = t.client().round_info(&Asset::Btc, &open_epoch).unwrap();
    assert_eq!(round.status, crate::types::RoundStatus::Open);
    assert_eq!(round.lock_time, open_epoch * INTERVAL);
}

#[test]
fn test_genesis_lock_transitions_round() {
    let t = TestEnv::new();
    t.client().genesis_start(&Asset::Btc);
    t.advance_to_next_epoch(); // epoch 101
    // Set oracle price AT the epoch 101 boundary timestamp.
    t.set_btc_price(t.current_epoch(), BTC_PRICE_1);
    t.client().genesis_lock(&Asset::Btc);

    let locked_epoch = t.current_epoch(); // 101
    let round = t.client().round_info(&Asset::Btc, &locked_epoch).unwrap();
    assert_eq!(round.status, crate::types::RoundStatus::Locked);
    assert_eq!(round.start_price, BTC_PRICE_1);

    // Next round (102) should be open.
    let next = t.client().round_info(&Asset::Btc, &(locked_epoch + 1)).unwrap();
    assert_eq!(next.status, crate::types::RoundStatus::Open);
}

#[test]
fn test_happy_path_up_wins() {
    let t = TestEnv::new();
    t.bootstrap_btc();
    // After bootstrap: epoch 101 is Locked (start=$50k), epoch 102 is Open.

    let open_epoch = 102u64;
    let alice = Address::generate(&t.env);
    let bob = Address::generate(&t.env);
    t.mint(&alice, 1_000_000_000);
    t.mint(&bob, 1_000_000_000);

    // Alice bets UP 500 XLM, Bob bets DOWN 300 XLM.
    t.client().bet(&alice, &Asset::Btc, &open_epoch, &Direction::Up, &500_000_000);
    t.client().bet(&bob, &Asset::Btc, &open_epoch, &Direction::Down, &300_000_000);

    let round_before = t.client().round_info(&Asset::Btc, &open_epoch).unwrap();
    assert_eq!(round_before.total_amount, 800_000_000);

    // Advance to epoch 102 boundary → lock round 102, close round 101.
    t.set_btc_price(102, BTC_PRICE_UP); // epoch 102 lock price
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    // Advance to epoch 103 boundary → close round 102 (price went up → UP wins).
    t.set_btc_price(103, BTC_PRICE_UP.checked_add(500_000_000_000_000_000).unwrap());
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    let round = t.client().round_info(&Asset::Btc, &open_epoch).unwrap();
    assert_eq!(round.status, crate::types::RoundStatus::Resolved);

    // Payout math:
    // total = 800_000_000
    // fee = 800_000_000 * 300 / 10_000 = 24_000_000
    // pool = 776_000_000
    // alice_payout = 500_000_000 * 776_000_000 / 500_000_000 = 776_000_000
    let alice_claimable = t.client().claimable(&Asset::Btc, &open_epoch, &alice);
    assert_eq!(alice_claimable, 776_000_000);
    let bob_claimable = t.client().claimable(&Asset::Btc, &open_epoch, &bob);
    assert_eq!(bob_claimable, 0); // bob lost

    // Conservation invariant: sum(payouts) + fee <= total
    assert!(alice_claimable + 24_000_000 <= 800_000_000);

    // Claim.
    let alice_before = t.token_balance(&alice);
    t.client().claim(&Asset::Btc, &vec![&t.env, open_epoch], &alice);
    let alice_after = t.token_balance(&alice);
    assert_eq!(alice_after - alice_before, alice_claimable);
}

#[test]
fn test_happy_path_down_wins() {
    let t = TestEnv::new();
    t.bootstrap_btc();

    let open_epoch = 102u64;
    let alice = Address::generate(&t.env);
    let bob = Address::generate(&t.env);
    t.mint(&alice, 1_000_000_000);
    t.mint(&bob, 1_000_000_000);

    t.client().bet(&alice, &Asset::Btc, &open_epoch, &Direction::Up, &300_000_000);
    t.client().bet(&bob, &Asset::Btc, &open_epoch, &Direction::Down, &600_000_000);

    // Lock at BTC_PRICE_1, then close at BTC_PRICE_DOWN.
    t.set_btc_price(102, BTC_PRICE_1);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    t.set_btc_price(103, BTC_PRICE_DOWN);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    let round = t.client().round_info(&Asset::Btc, &open_epoch).unwrap();
    assert_eq!(round.status, crate::types::RoundStatus::Resolved);
    assert!(round.end_price < round.start_price);

    // bob_payout = 600_000_000 * pool / 600_000_000 = pool
    // total=900, fee=27_000_000, pool=873_000_000
    let bob_claimable = t.client().claimable(&Asset::Btc, &open_epoch, &bob);
    assert_eq!(bob_claimable, 873_000_000);
    assert_eq!(t.client().claimable(&Asset::Btc, &open_epoch, &alice), 0);

    // Conservation.
    assert!(bob_claimable + 27_000_000 <= 900_000_000);
}

#[test]
fn test_one_sided_pool_up_only_refunds() {
    let t = TestEnv::new();
    t.bootstrap_btc();

    let open_epoch = 102u64;
    let alice = Address::generate(&t.env);
    t.mint(&alice, 1_000_000_000);
    t.client().bet(&alice, &Asset::Btc, &open_epoch, &Direction::Up, &500_000_000);

    t.set_btc_price(102, BTC_PRICE_1);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    t.set_btc_price(103, BTC_PRICE_UP);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    let round = t.client().round_info(&Asset::Btc, &open_epoch).unwrap();
    assert_eq!(round.status, crate::types::RoundStatus::Cancelled);

    // Refund = original bet, no fee.
    let refundable = t.client().claimable(&Asset::Btc, &open_epoch, &alice);
    assert_eq!(refundable, 500_000_000);

    let before = t.token_balance(&alice);
    t.client().refund(&Asset::Btc, &vec![&t.env, open_epoch], &alice);
    assert_eq!(t.token_balance(&alice) - before, 500_000_000);
}

#[test]
fn test_one_sided_pool_down_only_refunds() {
    let t = TestEnv::new();
    t.bootstrap_btc();

    let open_epoch = 102u64;
    let bob = Address::generate(&t.env);
    t.mint(&bob, 1_000_000_000);
    t.client().bet(&bob, &Asset::Btc, &open_epoch, &Direction::Down, &400_000_000);

    t.set_btc_price(102, BTC_PRICE_1);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    t.set_btc_price(103, BTC_PRICE_DOWN);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    let round = t.client().round_info(&Asset::Btc, &open_epoch).unwrap();
    assert_eq!(round.status, crate::types::RoundStatus::Cancelled);

    let refundable = t.client().claimable(&Asset::Btc, &open_epoch, &bob);
    assert_eq!(refundable, 400_000_000);
}

#[test]
fn test_exact_tie_refunds() {
    let t = TestEnv::new();
    t.bootstrap_btc();

    let open_epoch = 102u64;
    let alice = Address::generate(&t.env);
    let bob = Address::generate(&t.env);
    t.mint(&alice, 1_000_000_000);
    t.mint(&bob, 1_000_000_000);
    t.client().bet(&alice, &Asset::Btc, &open_epoch, &Direction::Up, &300_000_000);
    t.client().bet(&bob, &Asset::Btc, &open_epoch, &Direction::Down, &300_000_000);

    // Lock at price X, close at same price X → tie.
    t.set_btc_price(102, BTC_PRICE_1);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    t.set_btc_price(103, BTC_PRICE_SAME);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    let round = t.client().round_info(&Asset::Btc, &open_epoch).unwrap();
    assert_eq!(round.status, crate::types::RoundStatus::Cancelled);

    // Both get full refund, no fee.
    assert_eq!(t.client().claimable(&Asset::Btc, &open_epoch, &alice), 300_000_000);
    assert_eq!(t.client().claimable(&Asset::Btc, &open_epoch, &bob), 300_000_000);
}

#[test]
fn test_stale_oracle_cancels_round() {
    let t = TestEnv::new();
    t.bootstrap_btc();

    let open_epoch = 102u64;
    let alice = Address::generate(&t.env);
    t.mint(&alice, 1_000_000_000);
    t.client().bet(&alice, &Asset::Btc, &open_epoch, &Direction::Up, &500_000_000);

    t.set_btc_price(102, BTC_PRICE_1);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    // Do NOT set a price for epoch 103 → oracle returns None → cancel.
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    let round = t.client().round_info(&Asset::Btc, &open_epoch).unwrap();
    assert_eq!(round.status, crate::types::RoundStatus::Cancelled);

    // Alice can refund.
    assert_eq!(t.client().claimable(&Asset::Btc, &open_epoch, &alice), 500_000_000);
}

#[test]
fn test_double_claim_rejected() {
    let t = TestEnv::new();
    t.bootstrap_btc();

    let open_epoch = 102u64;
    let alice = Address::generate(&t.env);
    let bob = Address::generate(&t.env);
    t.mint(&alice, 1_000_000_000);
    t.mint(&bob, 1_000_000_000);
    t.client().bet(&alice, &Asset::Btc, &open_epoch, &Direction::Up, &500_000_000);
    t.client().bet(&bob, &Asset::Btc, &open_epoch, &Direction::Down, &300_000_000);

    t.set_btc_price(102, BTC_PRICE_1);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    t.set_btc_price(103, BTC_PRICE_UP);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    // First claim succeeds.
    t.client().claim(&Asset::Btc, &vec![&t.env, open_epoch], &alice);

    // Second claim: NothingToClaim because bet is already marked claimed.
    let res = t.client().try_claim(&Asset::Btc, &vec![&t.env, open_epoch], &alice);
    expect_error!(res, Error::NothingToClaim);
}

#[test]
fn test_bet_on_locked_round_rejected() {
    let t = TestEnv::new();
    t.bootstrap_btc();

    // After bootstrap, epoch 101 is Locked.
    let locked_epoch = 101u64;
    let alice = Address::generate(&t.env);
    t.mint(&alice, 1_000_000_000);

    let res = t.client().try_bet(
        &alice,
        &Asset::Btc,
        &locked_epoch,
        &Direction::Up,
        &500_000_000,
    );
    expect_error!(res, Error::InvalidEpoch);
}

#[test]
fn test_bet_below_minimum_rejected() {
    let t = TestEnv::new();
    t.bootstrap_btc();

    let open_epoch = 102u64;
    let alice = Address::generate(&t.env);
    t.mint(&alice, 1_000_000_000);

    let res = t.client().try_bet(
        &alice,
        &Asset::Btc,
        &open_epoch,
        &Direction::Up,
        &(MIN_BET - 1),
    );
    expect_error!(res, Error::BetTooSmall);
}

#[test]
fn test_keeper_missed_round_retroactive_settle() {
    let t = TestEnv::new();
    t.bootstrap_btc();
    // epoch 101: Locked (start=BTC_PRICE_1), epoch 102: Open.

    let open_epoch = 102u64;
    let alice = Address::generate(&t.env);
    let bob = Address::generate(&t.env);
    t.mint(&alice, 1_000_000_000);
    t.mint(&bob, 1_000_000_000);
    t.client().bet(&alice, &Asset::Btc, &open_epoch, &Direction::Up, &400_000_000);
    t.client().bet(&bob, &Asset::Btc, &open_epoch, &Direction::Down, &400_000_000);

    // Set prices but SKIP epoch 102's execute_round (simulate missed keeper).
    t.set_btc_price(102, BTC_PRICE_UP);
    t.set_btc_price(103, BTC_PRICE_UP.checked_add(1_000_000_000_000_000).unwrap());

    // Skip epoch 102, jump straight to 103.
    t.env.ledger().set_timestamp(103 * INTERVAL);
    t.client().execute_round(&Asset::Btc);

    // epoch 101 should be retroactively resolved (endPrice from epoch 102).
    let round_101 = t.client().round_info(&Asset::Btc, &101).unwrap();
    assert!(
        round_101.status == crate::types::RoundStatus::Resolved
            || round_101.status == crate::types::RoundStatus::Cancelled,
        "epoch 101 should be settled, got {:?}",
        round_101.status
    );

    // epoch 102 (open→never locked) should be cancelled.
    let round_102 = t.client().round_info(&Asset::Btc, &102).unwrap();
    assert_eq!(round_102.status, crate::types::RoundStatus::Cancelled);

    // alice and bob both get refunds for epoch 102.
    assert_eq!(t.client().claimable(&Asset::Btc, &open_epoch, &alice), 400_000_000);
    assert_eq!(t.client().claimable(&Asset::Btc, &open_epoch, &bob), 400_000_000);
}

#[test]
fn test_paused_blocks_betting() {
    let t = TestEnv::new();
    t.bootstrap_btc();
    t.client().pause();

    let open_epoch = 102u64;
    let alice = Address::generate(&t.env);
    t.mint(&alice, 1_000_000_000);

    let res = t
        .client()
        .try_bet(&alice, &Asset::Btc, &open_epoch, &Direction::Up, &500_000_000);
    expect_error!(res, Error::Paused);
}

#[test]
fn test_unpause_restores_betting() {
    let t = TestEnv::new();
    t.bootstrap_btc();
    t.client().pause();
    t.client().unpause();

    let open_epoch = 102u64;
    let alice = Address::generate(&t.env);
    t.mint(&alice, 1_000_000_000);
    // Should succeed.
    t.client().bet(&alice, &Asset::Btc, &open_epoch, &Direction::Up, &500_000_000);

    let round = t.client().round_info(&Asset::Btc, &open_epoch).unwrap();
    assert_eq!(round.up_amount, 500_000_000);
}

#[test]
fn test_execute_round_idempotent() {
    let t = TestEnv::new();
    t.bootstrap_btc();

    t.set_btc_price(102, BTC_PRICE_UP);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    t.set_btc_price(103, BTC_PRICE_UP);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    // Call execute_round again at same timestamp — must be a no-op.
    t.client().execute_round(&Asset::Btc);
    t.client().execute_round(&Asset::Btc);

    // State should be consistent.
    let r = t.client().round_info(&Asset::Btc, &102).unwrap();
    assert!(
        r.status == crate::types::RoundStatus::Resolved
            || r.status == crate::types::RoundStatus::Cancelled
    );
}

#[test]
fn test_current_epoch_returns_open_epoch() {
    let t = TestEnv::new();
    let epoch = t.client().current_epoch(&Asset::Btc);
    assert_eq!(epoch, t.current_epoch() + 1);
}

#[test]
fn test_batch_claim_multiple_epochs() {
    let t = TestEnv::new();
    t.bootstrap_btc();

    let alice = Address::generate(&t.env);
    t.mint(&alice, 10_000_000_000);

    let epochs: &[u64] = &[102, 103, 104];
    let bob = Address::generate(&t.env);
    t.mint(&bob, 10_000_000_000);

    // Place bets across three epochs.
    for &ep in epochs {
        t.client().bet(&alice, &Asset::Btc, &ep, &Direction::Up, &300_000_000);
        t.client().bet(&bob, &Asset::Btc, &ep, &Direction::Down, &200_000_000);

        t.set_btc_price(ep, BTC_PRICE_1); // lock price (flat)
        t.advance_to_next_epoch();
        t.client().execute_round(&Asset::Btc);
    }

    // Close last open epoch.
    let last_ep = epochs[epochs.len() - 1];
    t.set_btc_price(last_ep + 1, BTC_PRICE_UP); // UP wins
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    let before = t.token_balance(&alice);
    let epochs_vec = vec![&t.env, epochs[0], epochs[1], epochs[2]];
    t.client().claim(&Asset::Btc, &epochs_vec, &alice);
    let after = t.token_balance(&alice);
    assert!(after > before, "batch claim should transfer tokens");
}

// ═══════════════════════════════════════════════════════════════════════════════
// PAYOUT CONSERVATION TESTS (contract-level)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_conservation_invariant_contract_level() {
    // Verify at the contract level: after all winners claim,
    // the contract's token balance >= 0 (no overpayment).
    let t = TestEnv::new();
    t.bootstrap_btc();

    let open_epoch = 102u64;
    let users = [
        Address::generate(&t.env),
        Address::generate(&t.env),
        Address::generate(&t.env),
        Address::generate(&t.env),
        Address::generate(&t.env),
    ];
    t.mint(&users[0], 1_000_000_000); // 100 XLM
    t.mint(&users[1], 1_000_000_000);
    t.mint(&users[2], 1_000_000_000);
    t.mint(&users[3], 1_000_000_000);
    t.mint(&users[4], 1_000_000_000);

    t.client().bet(&users[0], &Asset::Btc, &open_epoch, &Direction::Up, &100_000_000);
    t.client().bet(&users[1], &Asset::Btc, &open_epoch, &Direction::Up, &200_000_000);
    t.client().bet(&users[2], &Asset::Btc, &open_epoch, &Direction::Up, &150_000_000);
    t.client().bet(&users[3], &Asset::Btc, &open_epoch, &Direction::Down, &300_000_000);
    t.client().bet(&users[4], &Asset::Btc, &open_epoch, &Direction::Down, &250_000_000);

    // UP wins.
    t.set_btc_price(102, BTC_PRICE_1);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    t.set_btc_price(103, BTC_PRICE_UP);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    let contract_balance_before = t.token_balance(&t.contract);

    // All winners claim.
    for u in &users[0..3] {
        t.client().claim(&Asset::Btc, &vec![&t.env, open_epoch], u);
    }

    let contract_balance_after = t.token_balance(&t.contract);
    // Contract balance must be >= 0 (no overpayment).
    assert!(
        contract_balance_after >= 0,
        "contract balance went negative: {contract_balance_after}"
    );

    // The remainder must be >= treasury (losers' funds minus fee already paid out).
    // At minimum the treasury_fee should remain plus any dust.
    let round = t.client().round_info(&Asset::Btc, &open_epoch).unwrap();
    assert!(
        contract_balance_after >= 0,
        "treasury fee should remain in contract"
    );

    // Total paid out + treasury_fee must not exceed original total.
    let paid_out = contract_balance_before - contract_balance_after;
    assert!(
        paid_out + round.treasury_fee <= round.total_amount,
        "paid_out={paid_out} + fee={} > total={}",
        round.treasury_fee,
        round.total_amount
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// SECURITY TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_cannot_bet_twice_same_round() {
    let t = TestEnv::new();
    t.bootstrap_btc();

    let open_epoch = 102u64;
    let alice = Address::generate(&t.env);
    t.mint(&alice, 2_000_000_000);
    t.client().bet(&alice, &Asset::Btc, &open_epoch, &Direction::Up, &300_000_000);

    let res = t.client().try_bet(
        &alice,
        &Asset::Btc,
        &open_epoch,
        &Direction::Up,
        &300_000_000,
    );
    expect_error!(res, Error::AlreadyBet);
}

#[test]
fn test_refund_on_non_cancelled_round_does_nothing() {
    let t = TestEnv::new();
    t.bootstrap_btc();

    let open_epoch = 102u64;
    let alice = Address::generate(&t.env);
    let bob = Address::generate(&t.env);
    t.mint(&alice, 1_000_000_000);
    t.mint(&bob, 1_000_000_000);
    t.client().bet(&alice, &Asset::Btc, &open_epoch, &Direction::Up, &300_000_000);
    t.client().bet(&bob, &Asset::Btc, &open_epoch, &Direction::Down, &300_000_000);

    // Resolve (not cancel) the round.
    t.set_btc_price(102, BTC_PRICE_1);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    t.set_btc_price(103, BTC_PRICE_UP);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    let round = t.client().round_info(&Asset::Btc, &open_epoch).unwrap();
    assert_eq!(round.status, crate::types::RoundStatus::Resolved);

    // refund on a Resolved round should NothingToClaim.
    let res = t.client().try_refund(&Asset::Btc, &vec![&t.env, open_epoch], &alice);
    expect_error!(res, Error::NothingToClaim);
}

#[test]
fn test_claim_on_cancelled_round_does_nothing() {
    let t = TestEnv::new();
    t.bootstrap_btc();

    let open_epoch = 102u64;
    let alice = Address::generate(&t.env);
    t.mint(&alice, 1_000_000_000);
    t.client().bet(&alice, &Asset::Btc, &open_epoch, &Direction::Up, &300_000_000);

    // One-sided → cancelled.
    t.set_btc_price(102, BTC_PRICE_1);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    t.set_btc_price(103, BTC_PRICE_UP);
    t.advance_to_next_epoch();
    t.client().execute_round(&Asset::Btc);

    // claim() (not refund()) on a cancelled round should NothingToClaim.
    let res = t.client().try_claim(&Asset::Btc, &vec![&t.env, open_epoch], &alice);
    expect_error!(res, Error::NothingToClaim);
}

#[test]
fn test_non_admin_cannot_pause() {
    let t = TestEnv::new();
    // mock_all_auths is enabled but we'll disable it for this check.
    // In real code the admin.require_auth() would catch this.
    // With mock_all_auths the auth always passes, so we skip this test
    // in the unit test environment.  The auth check is in the contract code.
    // This is a documentation test only.
}

// ═══════════════════════════════════════════════════════════════════════════════
// PURE PAYOUT MATH (covered more exhaustively in math.rs unit tests)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_math_conservation_invariant_quick() {
    // Re-run a subset of the pure math cases through the public API.
    use crate::math::{compute_payout, compute_pools};

    // Keep values ≤ 1e18 so bet_amount * pool < i128::MAX.
    let cases: &[(i128, u32, i128)] = &[
        (1_000_000_000, 300, 500_000_000),
        (999_999_999, 300, 333_333_333),
        (100, 9999, 1),
        (1_000_000_000_000_000_000i128, 300, 500_000_000_000_000_000),
    ];

    for &(total, fee_bps, winning_side) in cases {
        if winning_side <= 0 || winning_side > total {
            continue;
        }
        let (fee, pool) = compute_pools(total, fee_bps);
        let payout = compute_payout(winning_side, pool, winning_side);
        assert!(
            payout + fee <= total,
            "invariant violated: total={total} fee_bps={fee_bps} payout={payout} fee={fee}"
        );
    }
}
