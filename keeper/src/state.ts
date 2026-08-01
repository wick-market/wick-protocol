/**
 * Persists the latest round ID and settle timestamp per asset to disk.
 * This lets the keeper resume after a restart without losing track of
 * which rounds it needs to settle.
 */
import { readFileSync, writeFileSync, existsSync } from "fs";
import { join } from "path";

const STATE_PATH = join(__dirname, "..", "keeper-state.json");

export interface AssetState {
  roundId: string; // u64 as string (JSON can't handle BigInt natively)
  settleTs: number; // unix seconds
  asset: string;
}

type StateFile = Record<string, AssetState>;

export function loadState(): StateFile {
  if (!existsSync(STATE_PATH)) return {};
  try {
    return JSON.parse(readFileSync(STATE_PATH, "utf8"));
  } catch {
    return {};
  }
}

export function saveAsset(asset: string, state: AssetState) {
  const all = loadState();
  all[asset] = state;
  writeFileSync(STATE_PATH, JSON.stringify(all, null, 2));
}

export function getAsset(asset: string): AssetState | null {
  return loadState()[asset] ?? null;
}
