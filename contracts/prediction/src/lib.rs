// Wick — 5-minute parimutuel binary prediction market on Stellar/Soroban.
//
// Testnet Reflector ReflectorPulse oracle (USD-base, free, 5-min):
//   CCYOZJCOPG34LLQQ7N24YXBM7LL62R7ONMZ3G6WZAAYPB5OYKOMJRN63
//   Decimals: 14   Assets: BTC, ETH, SOL, XLM   Base: USD
//
// XLM Stellar Asset Contract: TODO — run `stellar contract id asset
//   --asset native --network testnet` at deploy time and populate .env.
//
// Unclaimed winnings: no sweep. Winners' Bet entries are kept alive by
// persistent-storage TTL bumps on every state-changing call (target 60 days).
// After 60 days with no contract interaction the entries may be archived;
// at that point the funds are stranded.  This is a Soroban protocol constraint,
// not a deliberate application decision.  Document prominently in the UI.

#![no_std]

mod events;
mod math;
mod oracle;
mod storage;
mod types;

use events::{
    emit_bet, emit_cancel, emit_claim, emit_lock, emit_resolve, BetEvent, CancelEvent,
    ClaimEvent, LockEvent, ResolveEvent,
};
use math::{compute_payout, compute_pools};
use oracle::{fetch_price, to_oracle_asset};
use storage::{
    add_treasury, bump_instance, clear_treasury, get_bet, get_config, get_genesis_status,
    get_round, get_treasury, get_user_epochs, has_config, has_round, push_user_epoch, set_bet,
    set_config, set_genesis_status, set_round,
};
use types::{Asset, Bet, Config, Direction, GenesisStatus, Round, RoundStatus};

use soroban_sdk::{
    contract, contracterror, contractimpl, panic_with_error, symbol_short, token, Address,
    Env, Vec,
};

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    Paused = 4,
    RoundNotOpen = 5,
    BetTooSmall = 6,
    InvalidEpoch = 7,
    GenesisNotStarted = 9,
    GenesisAlreadyRunning = 10,
    GenesisAlreadyStarted = 11,
    EpochNotReady = 12,
    AlreadyBet = 13,
    RoundNotFound = 14,
    NothingToClaim = 15,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct WickContract;

#[contractimpl]
impl WickContract {
    // ── Initialisation ────────────────────────────────────────────────────────

    pub fn initialize(
        e: Env,
        admin: Address,
        oracle: Address,
        token: Address,
        treasury: Address,
        fee_bps: u32,
        min_bet: i128,
    ) {
        if has_config(&e) {
            panic_with_error!(&e, Error::AlreadyInitialized);
        }
        admin.require_auth();
        let config = Config {
            admin,
            oracle,
            token,
            treasury,
            fee_bps,
            interval_secs: 300,
            min_bet,
            oracle_staleness_tolerance: 60,
            paused: false,
            tie_refund: true,
        };
        set_config(&e, &config);
        bump_instance(&e);
    }

    // ── Genesis ───────────────────────────────────────────────────────────────
    //
    // genesis_start: opens the first betting round.  Call once after deploy.
    // genesis_lock:  locks the first round exactly one 5-min epoch later.
    //                After this, execute_round takes over every epoch.

    pub fn genesis_start(e: Env, asset: Asset) {
        let config = require_config(&e);
        config.admin.require_auth();
        require_not_paused(&e, &config);

        if get_genesis_status(&e, &asset) != GenesisStatus::None {
            panic_with_error!(&e, Error::GenesisAlreadyStarted);
        }

        let now = e.ledger().timestamp();
        let interval = config.interval_secs;
        let current_epoch = now / interval;
        let open_epoch = current_epoch + 1;

        let round = Round::new(now, open_epoch * interval, (open_epoch + 1) * interval);
        set_round(&e, &asset, open_epoch, &round);
        set_genesis_status(&e, &asset, GenesisStatus::Started);
        bump_instance(&e);
    }

    pub fn genesis_lock(e: Env, asset: Asset) {
        let config = require_config(&e);
        config.admin.require_auth();
        require_not_paused(&e, &config);

        if get_genesis_status(&e, &asset) != GenesisStatus::Started {
            panic_with_error!(&e, Error::GenesisNotStarted);
        }

        let now = e.ledger().timestamp();
        let interval = config.interval_secs;
        let current_epoch = now / interval;

        // The round opened by genesis_start is the one that locks now.
        let lock_epoch = current_epoch;
        let mut round = get_round(&e, &asset, lock_epoch)
            .unwrap_or_else(|| panic_with_error!(&e, Error::RoundNotFound));

        if round.status != RoundStatus::Open {
            panic_with_error!(&e, Error::RoundNotOpen);
        }

        let epoch_ts = current_epoch * interval;
        let oracle_asset = to_oracle_asset(&e, &asset);
        let start_price_opt = fetch_price(
            &e,
            &config.oracle,
            oracle_asset,
            epoch_ts,
            config.oracle_staleness_tolerance,
        );

        match start_price_opt {
            Some(start_price) => {
                round.start_price = start_price;
                round.lock_time = epoch_ts;
                round.status = RoundStatus::Locked;
                set_round(&e, &asset, lock_epoch, &round);
                emit_lock(
                    &e,
                    &asset,
                    lock_epoch,
                    LockEvent {
                        start_price,
                        up_amount: round.up_amount,
                        down_amount: round.down_amount,
                        total_amount: round.total_amount,
                    },
                );
            }
            None => {
                round.status = RoundStatus::Cancelled;
                set_round(&e, &asset, lock_epoch, &round);
                emit_cancel(
                    &e,
                    &asset,
                    lock_epoch,
                    CancelEvent {
                        reason: symbol_short!("stale"),
                        total_amount: round.total_amount,
                    },
                );
            }
        }

        // Open the next betting round.
        let next_epoch = current_epoch + 1;
        if !has_round(&e, &asset, next_epoch) {
            let next = Round::new(epoch_ts, next_epoch * interval, (next_epoch + 1) * interval);
            set_round(&e, &asset, next_epoch, &next);
        }

        set_genesis_status(&e, &asset, GenesisStatus::Running);
        bump_instance(&e);
    }

    // ── execute_round (permissionless) ────────────────────────────────────────
    //
    // Closes Round(E-1), locks Round(E), opens Round(E+1).
    // Uses oracle.price(asset, E*300) for both operations — a single oracle
    // read that makes the outcome independent of when this tx lands.
    //
    // Also retroactively settles Round(E-2) if it was Locked but never resolved
    // (handles the case where execute_round was missed for one epoch).

    pub fn execute_round(e: Env, asset: Asset) {
        let config = require_config(&e);

        if get_genesis_status(&e, &asset) != GenesisStatus::Running {
            panic_with_error!(&e, Error::GenesisNotStarted);
        }

        let now = e.ledger().timestamp();
        let interval = config.interval_secs;
        let current_epoch = now / interval;
        let epoch_ts = current_epoch * interval;

        // Cache Round(E) once; reused for both idempotency check and lock step.
        let curr_round_opt = get_round(&e, &asset, current_epoch);

        // Idempotency: if the current epoch's round has already been locked or
        // cancelled by a previous execute_round call, this epoch is done.
        if let Some(ref curr) = curr_round_opt {
            if curr.status != RoundStatus::Open {
                return;
            }
        }

        // Single oracle read for this epoch boundary.
        let oracle_asset = to_oracle_asset(&e, &asset);
        let price_opt = fetch_price(
            &e,
            &config.oracle,
            oracle_asset.clone(),
            epoch_ts,
            config.oracle_staleness_tolerance,
        );

        // Retroactive settle for Round(E-2) if keeper missed epoch E-1.
        if current_epoch >= 2 {
            let stale_epoch = current_epoch - 2;
            if let Some(stale) = get_round(&e, &asset, stale_epoch) {
                if stale.status == RoundStatus::Locked {
                    let retro_ts = (current_epoch - 1) * interval;
                    let retro_price = fetch_price(
                        &e,
                        &config.oracle,
                        oracle_asset.clone(),
                        retro_ts,
                        config.oracle_staleness_tolerance,
                    );
                    settle_locked_round(&e, &asset, stale_epoch, retro_ts, retro_price, &config);
                }
            }
        }

        // Close Round(E-1).
        if let Some(prev) = get_round(&e, &asset, current_epoch.saturating_sub(1)) {
            match prev.status {
                RoundStatus::Locked => {
                    settle_locked_round(
                        &e,
                        &asset,
                        current_epoch - 1,
                        epoch_ts,
                        price_opt,
                        &config,
                    );
                }
                RoundStatus::Open => {
                    // Was never locked — cancel it.
                    let mut r = prev;
                    r.status = RoundStatus::Cancelled;
                    set_round(&e, &asset, current_epoch - 1, &r);
                    emit_cancel(
                        &e,
                        &asset,
                        current_epoch - 1,
                        CancelEvent {
                            reason: symbol_short!("missed"),
                            total_amount: r.total_amount,
                        },
                    );
                }
                _ => {}
            }
        }

        // Lock Round(E) — reuse the cached value, no second storage read.
        if let Some(mut curr) = curr_round_opt {
            if curr.status == RoundStatus::Open {
                match price_opt {
                    Some(start_price) => {
                        curr.start_price = start_price;
                        curr.lock_time = epoch_ts;
                        curr.status = RoundStatus::Locked;
                        set_round(&e, &asset, current_epoch, &curr);
                        emit_lock(
                            &e,
                            &asset,
                            current_epoch,
                            LockEvent {
                                start_price,
                                up_amount: curr.up_amount,
                                down_amount: curr.down_amount,
                                total_amount: curr.total_amount,
                            },
                        );
                    }
                    None => {
                        curr.status = RoundStatus::Cancelled;
                        set_round(&e, &asset, current_epoch, &curr);
                        emit_cancel(
                            &e,
                            &asset,
                            current_epoch,
                            CancelEvent {
                                reason: symbol_short!("stale"),
                                total_amount: curr.total_amount,
                            },
                        );
                    }
                }
            }
        }

        // Open Round(E+1).
        let next = current_epoch + 1;
        if !has_round(&e, &asset, next) {
            let r = Round::new(epoch_ts, next * interval, (next + 1) * interval);
            set_round(&e, &asset, next, &r);
        }

        bump_instance(&e);
    }

    // ── Betting ───────────────────────────────────────────────────────────────

    pub fn bet(e: Env, user: Address, asset: Asset, epoch: u64, direction: Direction, amount: i128) {
        user.require_auth();
        let config = require_config(&e);
        require_not_paused(&e, &config);

        if amount < config.min_bet {
            panic_with_error!(&e, Error::BetTooSmall);
        }

        let now = e.ledger().timestamp();
        let open_epoch = (now / config.interval_secs) + 1;
        if epoch != open_epoch {
            panic_with_error!(&e, Error::InvalidEpoch);
        }

        let mut round = get_round(&e, &asset, epoch)
            .unwrap_or_else(|| panic_with_error!(&e, Error::RoundNotFound));

        if round.status != RoundStatus::Open {
            panic_with_error!(&e, Error::RoundNotOpen);
        }

        // One bet per user per round.  Prevents re-betting on the opposite side.
        if get_bet(&e, &asset, epoch, &user).is_some() {
            panic_with_error!(&e, Error::AlreadyBet);
        }

        // Transfer XLM from user to contract.
        let tok = token::TokenClient::new(&e, &config.token);
        tok.transfer(&user, &e.current_contract_address(), &amount);

        // Update pool totals.
        match direction {
            Direction::Up => {
                round.up_amount = round.up_amount.checked_add(amount).expect("up overflow");
            }
            Direction::Down => {
                round.down_amount = round.down_amount.checked_add(amount).expect("down overflow");
            }
        }
        round.total_amount = round.total_amount.checked_add(amount).expect("total overflow");
        set_round(&e, &asset, epoch, &round);

        let bet = Bet { direction, amount, claimed: false };
        set_bet(&e, &asset, epoch, &user, &bet);
        push_user_epoch(&e, &asset, &user, epoch);

        emit_bet(
            &e,
            &asset,
            epoch,
            BetEvent {
                user: user.clone(),
                direction,
                amount,
                up_amount: round.up_amount,
                down_amount: round.down_amount,
            },
        );
        bump_instance(&e);
    }

    // ── Claiming ──────────────────────────────────────────────────────────────
    //
    // Permissionless: anyone can trigger a claim for `user`.
    // Payout always transfers to the original bettor (stored in Bet).
    // Reentrancy guard: claimed flag set to true BEFORE the token transfer.

    pub fn claim(e: Env, asset: Asset, epochs: Vec<u64>, user: Address) {
        let config = require_config(&e);
        let tok = token::TokenClient::new(&e, &config.token);
        let mut total_payout: i128 = 0;
        let mut count: u32 = 0;

        for epoch in epochs.iter() {
            let round = match get_round(&e, &asset, epoch) {
                Some(r) if r.status == RoundStatus::Resolved => r,
                _ => continue,
            };
            let mut bet = match get_bet(&e, &asset, epoch, &user) {
                Some(b) if !b.claimed => b,
                _ => continue,
            };

            let winning_side = if round.end_price > round.start_price {
                Direction::Up
            } else {
                Direction::Down
            };
            if bet.direction != winning_side {
                continue;
            }

            let bet_side_amount = match winning_side {
                Direction::Up => round.up_amount,
                Direction::Down => round.down_amount,
            };

            let payout = compute_payout(bet.amount, round.reward_pool, bet_side_amount);
            if payout == 0 {
                continue;
            }

            // Set claimed BEFORE the batch transfer below (reentrancy guard).
            bet.claimed = true;
            set_bet(&e, &asset, epoch, &user, &bet);

            total_payout = total_payout.checked_add(payout).expect("payout sum overflow");
            count += 1;
        }

        if count == 0 {
            panic_with_error!(&e, Error::NothingToClaim);
        }

        // Single cross-contract call instead of one per epoch.
        tok.transfer(&e.current_contract_address(), &user, &total_payout);

        emit_claim(
            &e,
            &asset,
            ClaimEvent {
                user,
                payout: total_payout,
                epoch_count: count,
            },
        );
        bump_instance(&e);
    }

    // ── Refunds (cancelled rounds) ────────────────────────────────────────────

    pub fn refund(e: Env, asset: Asset, epochs: Vec<u64>, user: Address) {
        let config = require_config(&e);
        let tok = token::TokenClient::new(&e, &config.token);
        let mut total: i128 = 0;
        let mut count: u32 = 0;

        for epoch in epochs.iter() {
            match get_round(&e, &asset, epoch) {
                Some(r) if r.status == RoundStatus::Cancelled => {}
                _ => continue,
            }
            let mut bet = match get_bet(&e, &asset, epoch, &user) {
                Some(b) if !b.claimed => b,
                _ => continue,
            };

            // Set claimed BEFORE the batch transfer below (reentrancy guard).
            bet.claimed = true;
            set_bet(&e, &asset, epoch, &user, &bet);
            total = total.checked_add(bet.amount).expect("refund overflow");
            count += 1;
        }

        if count == 0 {
            panic_with_error!(&e, Error::NothingToClaim);
        }

        // Single cross-contract call instead of one per epoch.
        tok.transfer(&e.current_contract_address(), &user, &total);
        bump_instance(&e);
    }

    // ── View functions ────────────────────────────────────────────────────────

    pub fn claimable(e: Env, asset: Asset, epoch: u64, user: Address) -> i128 {
        let round = match get_round(&e, &asset, epoch) {
            Some(r) => r,
            None => return 0,
        };
        let bet = match get_bet(&e, &asset, epoch, &user) {
            Some(b) if !b.claimed => b,
            _ => return 0,
        };

        match round.status {
            RoundStatus::Cancelled => bet.amount, // refund amount
            RoundStatus::Resolved => {
                let winning_side = if round.end_price > round.start_price {
                    Direction::Up
                } else {
                    Direction::Down
                };
                if bet.direction != winning_side {
                    return 0;
                }
                let bet_side_amount = match winning_side {
                    Direction::Up => round.up_amount,
                    Direction::Down => round.down_amount,
                };
                compute_payout(bet.amount, round.reward_pool, bet_side_amount)
            }
            _ => 0,
        }
    }

    pub fn round_info(e: Env, asset: Asset, epoch: u64) -> Option<Round> {
        get_round(&e, &asset, epoch)
    }

    /// Returns the epoch number of the currently-open betting round.
    pub fn current_epoch(e: Env, _asset: Asset) -> u64 {
        let interval = if has_config(&e) {
            get_config(&e).interval_secs
        } else {
            300
        };
        (e.ledger().timestamp() / interval) + 1
    }

    pub fn user_epochs(e: Env, asset: Asset, user: Address) -> Vec<u64> {
        get_user_epochs(&e, &asset, &user)
    }

    // ── Admin ─────────────────────────────────────────────────────────────────

    pub fn pause(e: Env) {
        let mut config = require_config(&e);
        config.admin.require_auth();
        config.paused = true;
        set_config(&e, &config);
        bump_instance(&e);
    }

    pub fn unpause(e: Env) {
        let mut config = require_config(&e);
        config.admin.require_auth();
        config.paused = false;
        set_config(&e, &config);
        bump_instance(&e);
    }

    pub fn set_config(
        e: Env,
        fee_bps: Option<u32>,
        min_bet: Option<i128>,
        oracle_staleness_tolerance: Option<u64>,
        tie_refund: Option<bool>,
        treasury: Option<Address>,
    ) {
        let mut config = require_config(&e);
        config.admin.require_auth();

        if let Some(v) = fee_bps {
            config.fee_bps = v;
        }
        if let Some(v) = min_bet {
            config.min_bet = v;
        }
        if let Some(v) = oracle_staleness_tolerance {
            config.oracle_staleness_tolerance = v;
        }
        if let Some(v) = tie_refund {
            config.tie_refund = v;
        }
        if let Some(v) = treasury {
            config.treasury = v;
        }
        set_config(&e, &config);
        bump_instance(&e);
    }

    /// Sweep accumulated treasury fee dust to the treasury address.
    pub fn claim_treasury(e: Env) {
        let config = require_config(&e);
        config.admin.require_auth();

        let amount = get_treasury(&e);
        if amount > 0 {
            let tok = token::TokenClient::new(&e, &config.token);
            tok.transfer(&e.current_contract_address(), &config.treasury, &amount);
            clear_treasury(&e);
        }
        bump_instance(&e);
    }

    pub fn get_config_view(e: Env) -> Config {
        require_config(&e)
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn require_config(e: &Env) -> Config {
    if !has_config(e) {
        panic_with_error!(e, Error::NotInitialized);
    }
    get_config(e)
}

fn require_not_paused(e: &Env, config: &Config) {
    if config.paused {
        panic_with_error!(e, Error::Paused);
    }
}

/// Settle a Locked round given an optional oracle end-price.
fn settle_locked_round(
    e: &Env,
    asset: &Asset,
    epoch: u64,
    close_ts: u64,
    price_opt: Option<i128>,
    config: &Config,
) {
    let mut round = match get_round(e, asset, epoch) {
        Some(r) if r.status == RoundStatus::Locked => r,
        _ => return,
    };

    match price_opt {
        Some(end_price) => {
            round.end_price = end_price;
            round.close_time = close_ts;

            let one_sided = round.up_amount == 0 || round.down_amount == 0;
            let is_tie = config.tie_refund && round.start_price == end_price;

            if one_sided || is_tie {
                let reason = if is_tie {
                    symbol_short!("tie")
                } else {
                    symbol_short!("onesided")
                };
                round.status = RoundStatus::Cancelled;
                set_round(e, asset, epoch, &round);
                emit_cancel(
                    e,
                    asset,
                    epoch,
                    CancelEvent {
                        reason,
                        total_amount: round.total_amount,
                    },
                );
            } else {
                let (fee, pool) =
                    compute_pools(round.total_amount, config.fee_bps);
                let winner = if end_price > round.start_price {
                    Direction::Up
                } else {
                    Direction::Down
                };
                round.treasury_fee = fee;
                round.reward_pool = pool;
                round.status = RoundStatus::Resolved;
                set_round(e, asset, epoch, &round);
                add_treasury(e, fee);
                emit_resolve(
                    e,
                    asset,
                    epoch,
                    ResolveEvent {
                        start_price: round.start_price,
                        end_price,
                        winner,
                        reward_pool: pool,
                        treasury_fee: fee,
                    },
                );
            }
        }
        None => {
            round.status = RoundStatus::Cancelled;
            set_round(e, asset, epoch, &round);
            emit_cancel(
                e,
                asset,
                epoch,
                CancelEvent {
                    reason: symbol_short!("stale"),
                    total_amount: round.total_amount,
                },
            );
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
