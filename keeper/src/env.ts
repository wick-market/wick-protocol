/**
 * Keeper secrets and config, loaded from .env (gitignored).
 *
 * These used to be hardcoded literals in demo-keeper.ts and init-markets.ts,
 * which meant every push copied live testnet keys into a public repo. There are
 * deliberately no inline fallbacks here: a missing key should fail loudly at
 * startup rather than silently fall back to a key baked into the source.
 *
 * Two .env files exist — keeper/.env and the repo root .env — so both are read,
 * keeper-local first and winning on conflict. Loading only one is a trap: the
 * keeper is launched from keeper/, so a bare `dotenv/config` never sees the root
 * file, and a key present only there would read as missing.
 */
import dotenv from "dotenv";
import path from "path";

// dotenv does not overwrite an already-set var, so the first load wins.
dotenv.config({ path: path.resolve(__dirname, "../.env") });      // keeper/.env
dotenv.config({ path: path.resolve(__dirname, "../../.env") });   // repo root .env

function required(name: string): string {
  const v = process.env[name]?.trim();
  if (!v) {
    throw new Error(
      `Missing ${name}. Add it to wick-protocol/.env or keeper/.env ` +
      `(see .env.example). Never hardcode secrets in source.`
    );
  }
  return v;
}

/** Admin/deployer key. Signs oracle pushes, create_round and settle. */
export const ADMIN_SECRET = required("ADMIN_SECRET");

/**
 * Demo bot wallets that take both sides so rounds resolve instead of voiding
 * for want of an opposing bet. Testnet only — funded by friendbot.
 */
export const BOT_SECRETS: string[] = [
  required("BOT_SECRET_1"),
  required("BOT_SECRET_2"),
  required("BOT_SECRET_3"),
];
