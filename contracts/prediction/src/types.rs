use soroban_sdk::{contracttype, Address};

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asset {
    Btc,
    Eth,
    Sol,
    Xlm,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq, Copy)]
pub enum Direction {
    Up,
    Down,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq, Copy)]
pub enum RoundStatus {
    Open,
    Locked,
    Resolved,
    Cancelled,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Round {
    /// Unix timestamp when betting opened.
    pub start_time: u64,
    /// Unix timestamp when startPrice was recorded (epoch boundary).
    pub lock_time: u64,
    /// Unix timestamp when endPrice was recorded (next epoch boundary).
    pub close_time: u64,
    /// Oracle price at lock_time. 0 until locked.
    pub start_price: i128,
    /// Oracle price at close_time. 0 until resolved.
    pub end_price: i128,
    pub up_amount: i128,
    pub down_amount: i128,
    pub total_amount: i128,
    /// total_amount - treasury_fee; set on resolution.
    pub reward_pool: i128,
    /// Treasury fee collected; set on resolution.
    pub treasury_fee: i128,
    pub status: RoundStatus,
}

impl Round {
    pub fn new(start_time: u64, lock_time: u64, close_time: u64) -> Self {
        Round {
            start_time,
            lock_time,
            close_time,
            start_price: 0,
            end_price: 0,
            up_amount: 0,
            down_amount: 0,
            total_amount: 0,
            reward_pool: 0,
            treasury_fee: 0,
            status: RoundStatus::Open,
        }
    }
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Bet {
    pub direction: Direction,
    pub amount: i128,
    pub claimed: bool,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Config {
    pub admin: Address,
    /// Reflector oracle contract address.
    pub oracle: Address,
    /// XLM Stellar Asset Contract address.
    pub token: Address,
    pub treasury: Address,
    /// Fee in basis points (e.g. 300 = 3%).
    pub fee_bps: u32,
    /// Round interval in seconds (300).
    pub interval_secs: u64,
    /// Minimum bet amount in stroops (100_000_000 = 10 XLM).
    pub min_bet: i128,
    /// Maximum seconds a returned oracle price may lag behind the requested timestamp.
    pub oracle_staleness_tolerance: u64,
    pub paused: bool,
    /// Whether an exact price tie refunds (true) or house takes all (false). Default: true.
    pub tie_refund: bool,
}

/// Tracks whether genesis has completed for a given asset so execute_round can take over.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq, Copy)]
pub enum GenesisStatus {
    /// genesis_start not yet called.
    None,
    /// genesis_start called; one round open, awaiting genesis_lock.
    Started,
    /// genesis_lock called; normal execute_round cadence active.
    Running,
}
