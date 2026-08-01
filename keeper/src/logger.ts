import { writeFileSync } from "fs";

const DRY_RUN = process.argv.includes("--dry-run");

function stamp(): string {
  return new Date().toISOString();
}

export function info(msg: string, data?: object) {
  console.log(JSON.stringify({ ts: stamp(), level: "info", msg, ...data }));
}

export function warn(msg: string, data?: object) {
  console.warn(JSON.stringify({ ts: stamp(), level: "warn", msg, ...data }));
}

export function error(msg: string, data?: object) {
  console.error(JSON.stringify({ ts: stamp(), level: "error", msg, ...data }));
}

export function dryRun(action: string, data?: object) {
  if (DRY_RUN) {
    console.log(JSON.stringify({ ts: stamp(), level: "dry-run", action, ...data }));
  }
}

export function isDryRun(): boolean {
  return DRY_RUN;
}

/** Fire-and-forget webhook alert. Used for: missed settle, low balance, oracle stale. */
export async function alert(subject: string, body: object) {
  const url = process.env.ALERT_WEBHOOK_URL;
  if (!url) return;
  try {
    await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ subject, body, ts: stamp() }),
    });
  } catch {
    // Webhook failures must not crash the keeper.
  }
}
