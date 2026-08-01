/**
 * WebSocket hub — broadcasts live round state to all connected clients.
 *
 * Messages pushed to clients:
 *   { type: "connected" }                    sent on initial connection
 *   { type: "round",  data: Round }          round state update (every 5 s)
 *   { type: "price",  asset, price, ts }     indicative CEX price tick
 *
 * CRITICAL SEPARATION: the "price" message is sourced from Binance WebSocket
 * (smooth, real-time). The settlement price comes from the Reflector oracle
 * (pinned to a 5-minute boundary). These are TWO DIFFERENT DATA SOURCES.
 * The displayed price is INDICATIVE ONLY and will differ slightly from the
 * settlement price. This is documented in the UI — do not merge these sources.
 */
import { WebSocket } from "ws";
import { pool } from "./db";

// All currently connected WebSocket clients.
const clients = new Set<WebSocket>();

export function registerClient(ws: WebSocket) {
  clients.add(ws);
  ws.on("close", () => clients.delete(ws));
}

export function broadcast(msg: object) {
  const text = JSON.stringify(msg);
  for (const ws of clients) {
    if (ws.readyState === WebSocket.OPEN) {
      ws.send(text);
    }
  }
}

// ── Indicative price ticker (Binance) ─────────────────────────────────────────
// This feeds the UI's live price chart. It is NOT the settlement price.
// Settlement uses oracle.price(asset, settle_ts) — a completely separate path.

const BINANCE_SYMBOLS: Record<string, string> = {
  BTC: "btcusdt",
  ETH: "ethusdt",
  SOL: "solusdt",
  XLM: "xlmusdt",
};

export function startPriceTicker() {
  const streams = Object.values(BINANCE_SYMBOLS)
    .map((s) => `${s}@miniTicker`)
    .join("/");
  const url = `wss://stream.binance.com:9443/stream?streams=${streams}`;

  function connect() {
    const ws = new WebSocket(url);

    ws.on("message", (raw: Buffer) => {
      try {
        const msg = JSON.parse(raw.toString());
        const data = msg.data;
        if (!data?.s) return;

        const asset = Object.entries(BINANCE_SYMBOLS).find(
          ([, v]) => v.toUpperCase() === data.s
        )?.[0];
        if (!asset) return;

        // Broadcast indicative price to all connected UI clients.
        broadcast({
          type: "price",
          asset,
          // Indicative display price from Binance — NOT the settlement price.
          price: data.c,
          ts: Date.now(),
        });
      } catch {
        // Malformed message — ignore.
      }
    });

    ws.on("close", () => setTimeout(connect, 5000));
    ws.on("error", () => ws.close());
  }

  connect();
}

// ── Round state broadcaster ───────────────────────────────────────────────────
// Pushes live round data (pools, status, countdown) every 5 seconds.

export function startRoundBroadcaster() {
  async function tick() {
    try {
      const { rows } = await pool.query(
        `SELECT * FROM rounds WHERE status != 'Settled'
         ORDER BY settle_ts ASC LIMIT 20`
      );
      for (const round of rows) {
        broadcast({ type: "round", data: round });
      }
    } catch {
      // DB hiccup — skip this tick.
    }
  }

  setInterval(tick, 5000);
}
