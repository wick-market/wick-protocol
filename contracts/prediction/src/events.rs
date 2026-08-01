/// Events emitted by the Wick prediction contract.
///
/// The indexer polls `getEvents` and keys on these topic tuples.
/// Topic layout: (contract_name, event_type, asset, epoch)
/// Body: event-specific payload struct.
///
/// All payloads use i128 amounts (stroops).  No floats.
use soroban_sdk::{contracttype, symbol_short, Address, Env, Symbol};
use crate::types::{Asset, Direction};

const CONTRACT: Symbol = symbol_short!("wick");

// ── Event body types ──────────────────────────────────────────────────────────

#[contracttype]
pub struct BetEvent {
    pub user: Address,
    pub direction: Direction,
    pub amount: i128,
    /// Running totals after this bet.
    pub up_amount: i128,
    pub down_amount: i128,
}

#[contracttype]
pub struct LockEvent {
    pub start_price: i128,
    pub up_amount: i128,
    pub down_amount: i128,
    pub total_amount: i128,
}

#[contracttype]
pub struct ResolveEvent {
    pub start_price: i128,
    pub end_price: i128,
    pub winner: Direction,
    pub reward_pool: i128,
    pub treasury_fee: i128,
}

#[contracttype]
pub struct CancelEvent {
    pub reason: Symbol,
    pub total_amount: i128,
}

#[contracttype]
pub struct ClaimEvent {
    pub user: Address,
    pub payout: i128,
    /// Epochs included in this batch claim.
    pub epoch_count: u32,
}

// ── Emitters ─────────────────────────────────────────────────────────────────

pub fn emit_bet(e: &Env, asset: &Asset, epoch: u64, payload: BetEvent) {
    e.events().publish(
        (CONTRACT, symbol_short!("bet"), asset.clone(), epoch),
        payload,
    );
}

pub fn emit_lock(e: &Env, asset: &Asset, epoch: u64, payload: LockEvent) {
    e.events().publish(
        (CONTRACT, symbol_short!("lock"), asset.clone(), epoch),
        payload,
    );
}

pub fn emit_resolve(e: &Env, asset: &Asset, epoch: u64, payload: ResolveEvent) {
    e.events().publish(
        (CONTRACT, symbol_short!("resolve"), asset.clone(), epoch),
        payload,
    );
}

pub fn emit_cancel(e: &Env, asset: &Asset, epoch: u64, payload: CancelEvent) {
    e.events().publish(
        (CONTRACT, symbol_short!("cancel"), asset.clone(), epoch),
        payload,
    );
}

pub fn emit_claim(e: &Env, asset: &Asset, payload: ClaimEvent) {
    e.events().publish(
        (CONTRACT, symbol_short!("claim"), asset.clone()),
        payload,
    );
}
