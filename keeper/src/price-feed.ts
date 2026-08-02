/**
 * Real spot prices for the demo oracle.
 *
 * The keeper used to invent prices with a random walk from a hardcoded seed, so
 * ETH and SOL were simply wrong on screen — no seed can track a live market.
 * This fetches actual USD spot and converts to oracle units (USD x 1e14).
 *
 * One shared poller serves all four markets. Each market pushes a price twice a
 * round (once at create, once before settle), so per-market fetching would be
 * ~8 requests/min and would trip CoinGecko's free-tier limit. Polling once into
 * a cache keeps it to ~4/min no matter how many markets there are.
 *
 * TradingView is deliberately not used here: it has no free quote API, and its
 * widgets are cross-origin iframes a keeper can't read. More importantly the
 * price has to be pushed on-chain, since that is what the contract settles
 * against — a browser-side feed would let the display and the settlement
 * disagree.
 */

/** CoinGecko id -> our market symbol. */
const COINGECKO_IDS: Record<string, string> = {
  bitcoin: "BTC",
  ethereum: "ETH",
  solana: "SOL",
  stellar: "XLM",
};

/** Oracle fixed-point scale: USD x 1e14, matching Reflector's 14 decimals. */
const ORACLE_SCALE = 14;

export interface PriceQuote {
  /** Price in oracle units (USD x 1e14). */
  units: bigint;
  /** Plain USD, for logging. */
  usd: number;
  /** When we fetched it (ms since epoch). */
  fetchedAt: number;
}

/**
 * Decimal string -> scaled bigint, without going through float.
 * Number(63397.12) * 1e14 loses precision at this magnitude; string shifting
 * does not.
 */
export function toOracleUnits(usd: string | number, scale = ORACLE_SCALE): bigint {
  const s = typeof usd === "number" ? usd.toFixed(scale) : usd.trim();
  const neg = s.startsWith("-");
  const body = neg ? s.slice(1) : s;
  const [intPart = "0", fracRaw = ""] = body.split(".");
  const frac = fracRaw.slice(0, scale).padEnd(scale, "0");
  const digits = `${intPart}${frac}`.replace(/^0+(?=\d)/, "");
  const v = BigInt(digits === "" ? "0" : digits);
  return neg ? -v : v;
}

const cache = new Map<string, PriceQuote>();
let lastFetch = 0;
let inFlight: Promise<void> | null = null;

/** How stale a cached quote may get before we refetch. */
const REFRESH_MS = 15_000;

async function fetchAll(): Promise<void> {
  const ids = Object.keys(COINGECKO_IDS).join(",");
  const url = `https://api.coingecko.com/api/v3/simple/price?ids=${ids}&vs_currencies=usd`;

  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), 8_000);
  try {
    const res = await fetch(url, { signal: ctl.signal });
    if (!res.ok) throw new Error(`coingecko HTTP ${res.status}`);
    const body = (await res.json()) as Record<string, { usd?: number }>;
    const now = Date.now();

    for (const [id, symbol] of Object.entries(COINGECKO_IDS)) {
      const usd = body[id]?.usd;
      if (typeof usd !== "number" || !Number.isFinite(usd) || usd <= 0) continue;
      cache.set(symbol, { units: toOracleUnits(usd), usd, fetchedAt: now });
    }
    lastFetch = now;
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Latest spot for a symbol, or null if we have never successfully fetched it.
 *
 * On a failed refresh the last good quote is returned rather than throwing —
 * a transient API blip should not stall round production. Callers that need to
 * know the data is fresh can check `fetchedAt`.
 */
export async function getSpot(symbol: string): Promise<PriceQuote | null> {
  if (Date.now() - lastFetch > REFRESH_MS) {
    // Collapse concurrent callers onto one request.
    if (!inFlight) {
      inFlight = fetchAll().finally(() => { inFlight = null; });
    }
    try {
      await inFlight;
    } catch {
      // Fall through to whatever is cached.
    }
  }
  return cache.get(symbol) ?? null;
}
