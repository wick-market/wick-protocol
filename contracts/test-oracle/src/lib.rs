// Test Oracle — admin-controlled price feed for development.
// Implements the same SEP-40 interface as Reflector so wick-predict
// works without changing anything except the oracle address.
//
// update_price(price) — admin sets XLM/USD price and advances the timestamp.
// lastprice()         — returns the latest price (same as Reflector interface).
// price(asset, ts)    — returns price stored at or before ts.
// decimals()          — returns 14 (matching Reflector).
//
// Deploy this on testnet, use it with wick-predict, call update_price()
// from the keeper every minute for fast rounds.

#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Symbol};

#[contracttype]
enum Key { Admin, Price, Timestamp }

#[contracttype]
#[derive(Clone)]
pub struct PriceData {
    pub price: i128,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum OracleAsset {
    Stellar(Address),
    Other(Symbol),
}

#[contract]
pub struct TestOracle;

#[contractimpl]
impl TestOracle {
    /// Deploy once. Admin is the only one who can update the price.
    pub fn initialize(e: Env, admin: Address, initial_price: i128) {
        admin.require_auth();
        e.storage().instance().set(&Key::Admin, &admin);
        e.storage().instance().set(&Key::Price, &initial_price);
        e.storage().instance().set(&Key::Timestamp, &e.ledger().timestamp());
    }

    /// Set a new price. Timestamp advances to current ledger time.
    pub fn update_price(e: Env, price: i128) {
        let admin: Address = e.storage().instance().get(&Key::Admin).unwrap();
        admin.require_auth();
        e.storage().instance().set(&Key::Price, &price);
        e.storage().instance().set(&Key::Timestamp, &e.ledger().timestamp());
    }

    // ── SEP-40 interface (identical to Reflector) ─────────────────────────────

    pub fn lastprice(e: Env, _asset: OracleAsset) -> Option<PriceData> {
        let price: i128 = e.storage().instance().get(&Key::Price)?;
        let timestamp: u64 = e.storage().instance().get(&Key::Timestamp).unwrap_or(0);
        Some(PriceData { price, timestamp })
    }

    pub fn price(e: Env, _asset: OracleAsset, timestamp: u64) -> Option<PriceData> {
        let stored_ts: u64 = e.storage().instance().get(&Key::Timestamp).unwrap_or(0);
        // Return stored price if it matches or is earlier than requested timestamp.
        if stored_ts <= timestamp {
            let price: i128 = e.storage().instance().get(&Key::Price)?;
            Some(PriceData { price, timestamp: stored_ts })
        } else {
            None
        }
    }

    pub fn decimals(_e: Env) -> u32 {
        14 // matches Reflector
    }

    pub fn get_price(e: Env) -> (i128, u64) {
        let price: i128 = e.storage().instance().get(&Key::Price).unwrap_or(0);
        let ts: u64 = e.storage().instance().get(&Key::Timestamp).unwrap_or(0);
        (price, ts)
    }
}
