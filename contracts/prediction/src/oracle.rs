/// Reflector SEP-40 oracle client interface.
///
/// ReflectorPulse contract on testnet:
///   CCYOZJCOPG34LLQQ7N24YXBM7LL62R7ONMZ3G6WZAAYPB5OYKOMJRN63
/// Decimals: 14  (price = raw_i128 / 10^14)
/// Timeframe: 300 s (5 minutes)
/// Base asset: USD
/// Assets: BTC, ETH, SOL, XLM (all as Other(Symbol))
///
/// WARNING: The testnet contract has a `feeConfig` field in the orchestrator
/// config that is absent on mainnet.  Step 3 integration tests will verify that
/// `price()` reads are truly free before we rely on this for settlement.
use soroban_sdk::{contractclient, contracttype, Address, Env, Symbol, Vec};

/// Asset discriminant used by the Reflector oracle.
#[contracttype]
#[derive(Clone)]
pub enum OracleAsset {
    Stellar(Address),
    Other(Symbol),
}

/// Price record returned by the oracle.
#[contracttype]
#[derive(Clone, Debug)]
pub struct PriceData {
    /// i128 where actual_price = price / 10^decimals
    pub price: i128,
    pub timestamp: u64,
}

/// Minimal SEP-40 interface — we only use the two functions the contract calls.
#[contractclient(name = "OracleClient")]
pub trait Oracle {
    /// Returns the oracle price at or before `timestamp`.
    /// Returns None if no price is available for that timestamp.
    fn price(env: Env, asset: OracleAsset, timestamp: u64) -> Option<PriceData>;

    /// Number of decimal places for all prices in this oracle.
    fn decimals(env: Env) -> u32;

    /// Resolution of price updates in seconds.
    fn resolution(env: Env) -> u32;
}

/// Fetch point-in-time price for settlement.  Returns None when the oracle has
/// no data at or recent enough to `timestamp`.
///
/// This intentionally calls `price(asset, timestamp)` not `lastprice(asset)`.
/// Settlement must be deterministic: a late keeper call produces the identical
/// result as an on-time one.  `lastprice` would make outcome keeper-timing
/// dependent — a manipulation vector.
pub fn fetch_price(
    e: &Env,
    oracle: &Address,
    asset: OracleAsset,
    timestamp: u64,
    staleness_tolerance_secs: u64,
) -> Option<i128> {
    let client = OracleClient::new(e, oracle);
    let data = client.price(&asset, &timestamp)?;

    // The oracle returns the most recent price at or before `timestamp`.
    // Reject if that price is older than we will tolerate.
    let age = timestamp.saturating_sub(data.timestamp);
    if age > staleness_tolerance_secs {
        return None;
    }

    Some(data.price)
}

/// Convert a Wick Asset to its Reflector OracleAsset counterpart.
pub fn to_oracle_asset(e: &Env, asset: &crate::types::Asset) -> OracleAsset {
    use crate::types::Asset;
    let sym = match asset {
        Asset::Btc => Symbol::new(e, "BTC"),
        Asset::Eth => Symbol::new(e, "ETH"),
        Asset::Sol => Symbol::new(e, "SOL"),
        Asset::Xlm => Symbol::new(e, "XLM"),
    };
    OracleAsset::Other(sym)
}
