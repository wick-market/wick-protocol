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
enum Key { Admin, Price, Timestamp, AssetPrice(Symbol), AssetTs(Symbol) }

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

    /// Set a new price for default asset.
    pub fn update_price(e: Env, price: i128) {
        e.storage().instance().set(&Key::Price, &price);
        e.storage().instance().set(&Key::Timestamp, &e.ledger().timestamp());
    }

    /// Set a new price for a specific asset symbol (e.g. BTC, ETH, SOL, XLM).
    pub fn update_asset_price(e: Env, asset: Symbol, price: i128) {
        let ts = e.ledger().timestamp();
        e.storage().instance().set(&Key::AssetPrice(asset.clone()), &price);
        e.storage().instance().set(&Key::AssetTs(asset.clone()), &ts);
        // Also update default if it's XLM
        if asset == Symbol::new(&e, "XLM") {
            e.storage().instance().set(&Key::Price, &price);
            e.storage().instance().set(&Key::Timestamp, &ts);
        }
    }

    // ── SEP-40 interface (identical to Reflector) ─────────────────────────────

    pub fn lastprice(e: Env, asset: OracleAsset) -> Option<PriceData> {
        Self::_get_price(&e, asset)
    }

    pub fn price(e: Env, asset: OracleAsset, _timestamp: u64) -> Option<PriceData> {
        Self::_get_price(&e, asset)
    }

    fn _get_price(e: &Env, asset: OracleAsset) -> Option<PriceData> {
        if let OracleAsset::Other(sym) = asset {
            if let Some(p) = e.storage().instance().get::<_, i128>(&Key::AssetPrice(sym.clone())) {
                let ts = e.storage().instance().get::<_, u64>(&Key::AssetTs(sym)).unwrap_or_else(|| e.ledger().timestamp());
                return Some(PriceData { price: p, timestamp: ts });
            }
        }
        let price: i128 = e.storage().instance().get(&Key::Price)?;
        let timestamp: u64 = e.storage().instance().get(&Key::Timestamp).unwrap_or(0);
        Some(PriceData { price, timestamp })
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
