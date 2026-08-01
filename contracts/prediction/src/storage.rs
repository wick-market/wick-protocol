use soroban_sdk::{contracttype, Env, Address};
use crate::types::{Asset, Bet, Config, GenesisStatus, Round};

// ── TTL constants ────────────────────────────────────────────────────────────
// 1 ledger ≈ 5 s.  Values are in ledgers.

/// Bump instance storage when TTL falls below this (15 days).
pub const INSTANCE_RENEWAL_THRESHOLD: u32 = 15 * 24 * 3600 / 5; // 259_200
/// Target instance TTL after bump (30 days).
pub const INSTANCE_TARGET_TTL: u32 = 30 * 24 * 3600 / 5; // 518_400

/// Bump persistent entries when TTL falls below this (30 days).
pub const PERSISTENT_RENEWAL_THRESHOLD: u32 = 30 * 24 * 3600 / 5; // 518_400
/// Target persistent TTL after bump (60 days).  Winners can claim indefinitely;
/// funds are stranded only if no contract interaction occurs for > 60 days AND
/// the user hasn't bumped their own entries.  Document this prominently.
pub const PERSISTENT_TARGET_TTL: u32 = 60 * 24 * 3600 / 5; // 1_036_800

// ── Storage keys ────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Config,
    GenesisStatus(Asset),
    Round(Asset, u64),              // (asset, epoch)
    Bet(Asset, u64, Address),       // (asset, epoch, user)
    UserEpochs(Asset, Address),     // bounded ring-buffer of user's epochs
    Treasury,                       // accumulated fee dust awaiting admin sweep
}

// ── Config ───────────────────────────────────────────────────────────────────

pub fn set_config(e: &Env, config: &Config) {
    e.storage().instance().set(&DataKey::Config, config);
}

pub fn get_config(e: &Env) -> Config {
    e.storage().instance().get(&DataKey::Config).unwrap()
}

pub fn has_config(e: &Env) -> bool {
    e.storage().instance().has(&DataKey::Config)
}

// ── Genesis status ────────────────────────────────────────────────────────────

pub fn set_genesis_status(e: &Env, asset: &Asset, status: GenesisStatus) {
    e.storage().instance().set(&DataKey::GenesisStatus(asset.clone()), &status);
}

pub fn get_genesis_status(e: &Env, asset: &Asset) -> GenesisStatus {
    e.storage()
        .instance()
        .get(&DataKey::GenesisStatus(asset.clone()))
        .unwrap_or(GenesisStatus::None)
}

// ── Round ─────────────────────────────────────────────────────────────────────

pub fn set_round(e: &Env, asset: &Asset, epoch: u64, round: &Round) {
    let key = DataKey::Round(asset.clone(), epoch);
    e.storage().persistent().set(&key, round);
    e.storage()
        .persistent()
        .extend_ttl(&key, PERSISTENT_RENEWAL_THRESHOLD, PERSISTENT_TARGET_TTL);
}

pub fn get_round(e: &Env, asset: &Asset, epoch: u64) -> Option<Round> {
    e.storage()
        .persistent()
        .get(&DataKey::Round(asset.clone(), epoch))
}

pub fn has_round(e: &Env, asset: &Asset, epoch: u64) -> bool {
    e.storage()
        .persistent()
        .has(&DataKey::Round(asset.clone(), epoch))
}

// ── Bet ───────────────────────────────────────────────────────────────────────

pub fn set_bet(e: &Env, asset: &Asset, epoch: u64, user: &Address, bet: &Bet) {
    let key = DataKey::Bet(asset.clone(), epoch, user.clone());
    e.storage().persistent().set(&key, bet);
    e.storage()
        .persistent()
        .extend_ttl(&key, PERSISTENT_RENEWAL_THRESHOLD, PERSISTENT_TARGET_TTL);
}

pub fn get_bet(e: &Env, asset: &Asset, epoch: u64, user: &Address) -> Option<Bet> {
    e.storage()
        .persistent()
        .get(&DataKey::Bet(asset.clone(), epoch, user.clone()))
}

// ── UserEpochs ring-buffer ────────────────────────────────────────────────────

/// Maximum epochs tracked per (asset, user).  Old entries are overwritten once
/// the buffer fills.  16 entries is enough for 80 minutes of active rounds.
const USER_EPOCHS_CAP: u32 = 32;

pub fn push_user_epoch(e: &Env, asset: &Asset, user: &Address, epoch: u64) {
    let key = DataKey::UserEpochs(asset.clone(), user.clone());
    let mut buf: soroban_sdk::Vec<u64> = e
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| soroban_sdk::Vec::new(e));

    // Avoid duplicate entries for the same epoch.
    for i in 0..buf.len() {
        if buf.get(i).unwrap() == epoch {
            return;
        }
    }

    if buf.len() >= USER_EPOCHS_CAP {
        // Drop oldest entry (index 0).
        let mut new_buf = soroban_sdk::Vec::new(e);
        for i in 1..buf.len() {
            new_buf.push_back(buf.get(i).unwrap());
        }
        buf = new_buf;
    }
    buf.push_back(epoch);

    e.storage().persistent().set(&key, &buf);
    e.storage()
        .persistent()
        .extend_ttl(&key, PERSISTENT_RENEWAL_THRESHOLD, PERSISTENT_TARGET_TTL);
}

pub fn get_user_epochs(e: &Env, asset: &Asset, user: &Address) -> soroban_sdk::Vec<u64> {
    e.storage()
        .persistent()
        .get(&DataKey::UserEpochs(asset.clone(), user.clone()))
        .unwrap_or_else(|| soroban_sdk::Vec::new(e))
}

// ── Treasury dust ─────────────────────────────────────────────────────────────

pub fn add_treasury(e: &Env, amount: i128) {
    let current: i128 = e
        .storage()
        .instance()
        .get(&DataKey::Treasury)
        .unwrap_or(0);
    e.storage()
        .instance()
        .set(&DataKey::Treasury, &current.checked_add(amount).expect("treasury overflow"));
}

pub fn get_treasury(e: &Env) -> i128 {
    e.storage().instance().get(&DataKey::Treasury).unwrap_or(0)
}

pub fn clear_treasury(e: &Env) {
    e.storage().instance().set(&DataKey::Treasury, &0i128);
}

// ── Instance TTL bump ─────────────────────────────────────────────────────────

pub fn bump_instance(e: &Env) {
    e.storage()
        .instance()
        .extend_ttl(INSTANCE_RENEWAL_THRESHOLD, INSTANCE_TARGET_TTL);
}
