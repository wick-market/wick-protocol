/**
 * Wick REST + WebSocket API
 *
 * REST:
 *   GET /api/rounds/current?asset=BTC
 *   GET /api/rounds/history?asset=BTC&limit=50
 *   GET /api/rounds/:id
 *   GET /api/users/:address/positions
 *   GET /api/users/:address/claimable
 *   GET /api/leaderboard?window=7d
 *   GET /api/stats
 *   GET /health
 *
 * WebSocket:
 *   WS /ws → receives { type, data } messages (see ws.ts)
 */
import "dotenv/config";
import Fastify from "fastify";
import fastifyWebSocket from "@fastify/websocket";
import fastifyCors from "@fastify/cors";
import {
  getCurrentRounds,
  getRoundHistory,
  getRoundById,
  getUserPositions,
  getUserClaimable,
  getLeaderboard,
  getStats,
} from "./db";
import { registerClient, startPriceTicker, startRoundBroadcaster } from "./ws";

const PORT = parseInt(process.env.API_PORT ?? "3000");

const app = Fastify({ logger: true });

// ── Plugins ───────────────────────────────────────────────────────────────────

// ALLOWED_ORIGINS is a comma-separated list, e.g. "https://wick-app.vercel.app,http://localhost:3000"
// Defaults to localhost:3000 so local frontend dev works with no config.
const allowedOrigins = (process.env.ALLOWED_ORIGINS ?? "http://localhost:3000")
  .split(",")
  .map((o) => o.trim());
await app.register(fastifyCors, { origin: allowedOrigins });
await app.register(fastifyWebSocket);

// ── WebSocket ─────────────────────────────────────────────────────────────────

app.get("/ws", { websocket: true }, (socket) => {
  registerClient(socket);
  socket.send(JSON.stringify({ type: "connected" }));
});

// ── REST routes ───────────────────────────────────────────────────────────────

app.get("/health", async () => ({ ok: true, ts: Date.now() }));

app.get<{ Querystring: { asset?: string } }>(
  "/api/rounds/current",
  async (req, reply) => {
    const asset = req.query.asset?.toUpperCase();
    if (!asset) return reply.code(400).send({ error: "asset required" });
    return getCurrentRounds(asset);
  }
);

app.get<{ Querystring: { asset?: string; limit?: string } }>(
  "/api/rounds/history",
  async (req, reply) => {
    const asset = req.query.asset?.toUpperCase();
    const limit = parseInt(req.query.limit ?? "50");
    if (!asset) return reply.code(400).send({ error: "asset required" });
    return getRoundHistory(asset, limit);
  }
);

app.get<{ Params: { id: string } }>(
  "/api/rounds/:id",
  async (req, reply) => {
    const round = await getRoundById(req.params.id);
    if (!round) return reply.code(404).send({ error: "not found" });
    return round;
  }
);

app.get<{ Params: { address: string } }>(
  "/api/users/:address/positions",
  async (req) => getUserPositions(req.params.address)
);

app.get<{ Params: { address: string } }>(
  "/api/users/:address/claimable",
  async (req) => getUserClaimable(req.params.address)
);

app.get<{ Querystring: { window?: string } }>(
  "/api/leaderboard",
  async (req) => getLeaderboard(req.query.window ?? "7d")
);

app.get("/api/stats", async () => getStats());

// ── Start ─────────────────────────────────────────────────────────────────────

await app.listen({ port: PORT, host: "0.0.0.0" });

// Start background WebSocket feeds.
startPriceTicker();
startRoundBroadcaster();

console.log(`API running on port ${PORT}`);
