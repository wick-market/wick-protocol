/// SEP-40 oracle client for Reflector ReflectorPulse.
///
/// Settlement MUST use price(asset, settle_ts), never lastprice().
/// create_round uses lastprice() to get the opening strike.
/// The distinction is a security control: price(asset, ts) pins settlement to
/// a specific timestamp so the outcome is identical no matter who calls
/// settle() or when. lastprice() would let the settler pick a favourable time.
use soroban_sdk::{contractclient, contracttype, Address, Env, Symbol};

/// Reflector asset discriminant — mirrors the on-chain type exactly so XDR
/// encoding is compatible when the prediction contract calls the oracle.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OracleAsset {
    Stellar(Address),
    Other(Symbol),
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct PriceData {
    pub price: i128,
    pub timestamp: u64,
}

#[contractclient(name = "OracleClient")]
pub trait PriceOracle {
    /// Most recent price for `asset`. Used in create_round to get the strike.
    fn lastprice(env: Env, asset: OracleAsset) -> Option<PriceData>;
    /// Price at or before `timestamp`. Used in settle() — pinned to settle_ts.
    fn price(env: Env, asset: OracleAsset, timestamp: u64) -> Option<PriceData>;
    /// Decimal places used by all prices in this oracle feed.
    fn decimals(env: Env) -> u32;
}

// ── Mock oracle for tests ─────────────────────────────────────────────────────
//
// Included only in test compilation via the enclosing cfg(test) module in lib.rs.

#[cfg(test)]
pub mod mock {
    use super::{OracleAsset, PriceData};
    use soroban_sdk::{contract, contractimpl, contracttype, map, symbol_short, Env, Map, Symbol};

    #[contracttype]
    enum MockKey {
        Prices, // Map<(Symbol, u64), i128>
    }

    fn asset_sym(e: &Env, asset: &OracleAsset) -> Symbol {
        match asset {
            OracleAsset::Other(s) => s.clone(),
            OracleAsset::Stellar(_) => symbol_short!("STELLAR"),
        }
    }

    #[contract]
    pub struct MockOracle;

    #[contractimpl]
    impl MockOracle {
        /// Test helper: store a price for (asset, timestamp).
        pub fn set_price(e: Env, asset: OracleAsset, timestamp: u64, price: i128) {
            let mut m: Map<(Symbol, u64), i128> =
                e.storage().instance().get(&MockKey::Prices).unwrap_or_else(|| map![&e]);
            m.set((asset_sym(&e, &asset), timestamp), price);
            e.storage().instance().set(&MockKey::Prices, &m);
        }

        // ── SEP-40 interface ─────────────────────────────────────────────────

        pub fn lastprice(e: Env, asset: OracleAsset) -> Option<PriceData> {
            let m: Map<(Symbol, u64), i128> =
                e.storage().instance().get(&MockKey::Prices).unwrap_or_else(|| map![&e]);
            let key = asset_sym(&e, &asset);
            let mut best: Option<PriceData> = None;
            for (k, v) in m.iter() {
                let (sym, ts) = k;
                if sym != key {
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

        pub fn price(e: Env, asset: OracleAsset, timestamp: u64) -> Option<PriceData> {
            let m: Map<(Symbol, u64), i128> =
                e.storage().instance().get(&MockKey::Prices).unwrap_or_else(|| map![&e]);
            let key = asset_sym(&e, &asset);
            let mut best: Option<PriceData> = None;
            for (k, v) in m.iter() {
                let (sym, ts) = k;
                if sym != key || ts > timestamp {
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
            // Only return if the timestamp matches exactly (no stale data).
            best.and_then(|p| if p.timestamp == timestamp { Some(p) } else { None })
        }

        pub fn decimals(_e: Env) -> u32 {
            14
        }
    }
}
