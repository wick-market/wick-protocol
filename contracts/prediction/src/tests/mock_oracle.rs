/// Mock SEP-40 oracle for testing.  Stores prices by (asset_key, timestamp)
/// and exposes a test-only `set_price` function.
use soroban_sdk::{
    contract, contractimpl, contracttype, map, Address, Env, Map, Symbol, Vec,
};

// Re-export the oracle types so tests can reference them without importing
// from the oracle module directly.
use crate::oracle::{OracleAsset, PriceData};

// Key type for the price map stored in the mock's persistent storage.
#[contracttype]
#[derive(Clone)]
pub enum MockKey {
    Prices,
    Decimals,
}

/// Identifies an asset for storage keying. We stringify the asset to a Symbol.
fn asset_key(e: &Env, asset: &OracleAsset) -> Symbol {
    match asset {
        OracleAsset::Other(sym) => sym.clone(),
        OracleAsset::Stellar(_) => Symbol::new(e, "STELLAR"),
    }
}

#[contract]
pub struct MockOracle;

#[contractimpl]
impl MockOracle {
    /// Store a price for (asset, timestamp).  Call this from test code.
    pub fn set_price(e: Env, asset: OracleAsset, timestamp: u64, price: i128) {
        let mut prices: Map<(Symbol, u64), i128> = e
            .storage()
            .instance()
            .get(&MockKey::Prices)
            .unwrap_or_else(|| map![&e]);

        let key = asset_key(&e, &asset);
        prices.set((key, timestamp), price);
        e.storage().instance().set(&MockKey::Prices, &prices);
    }

    // ── SEP-40 interface ───────────────────────────────────────────────────

    pub fn price(e: Env, asset: OracleAsset, timestamp: u64) -> Option<PriceData> {
        let prices: Map<(Symbol, u64), i128> = e
            .storage()
            .instance()
            .get(&MockKey::Prices)
            .unwrap_or_else(|| map![&e]);

        let key = asset_key(&e, &asset);

        // Find the most recent stored price at or before `timestamp`.
        let mut best: Option<PriceData> = None;
        for (k, v) in prices.iter() {
            let (asset_sym, ts) = k;
            if asset_sym != key || ts > timestamp {
                continue;
            }
            match &best {
                None => best = Some(PriceData { price: v, timestamp: ts }),
                Some(b) if ts > b.timestamp => {
                    best = Some(PriceData { price: v, timestamp: ts });
                }
                _ => {}
            }
        }
        best
    }

    pub fn lastprice(e: Env, asset: OracleAsset) -> Option<PriceData> {
        let prices: Map<(Symbol, u64), i128> = e
            .storage()
            .instance()
            .get(&MockKey::Prices)
            .unwrap_or_else(|| map![&e]);

        let key = asset_key(&e, &asset);
        let mut best: Option<PriceData> = None;
        for (k, v) in prices.iter() {
            let (asset_sym, ts) = k;
            if asset_sym != key {
                continue;
            }
            match &best {
                None => best = Some(PriceData { price: v, timestamp: ts }),
                Some(b) if ts > b.timestamp => {
                    best = Some(PriceData { price: v, timestamp: ts });
                }
                _ => {}
            }
        }
        best
    }

    pub fn decimals(_e: Env) -> u32 {
        14
    }

    pub fn resolution(_e: Env) -> u32 {
        300
    }
}
