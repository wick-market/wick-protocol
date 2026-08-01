#!/usr/bin/env bash
# Deploy Wick Fair Market to Stellar testnet.
# Requires: .env populated with ADMIN_SECRET and ADMIN_ADDRESS.
# Output: CONTRACT_ID written to .contract-id and .env CONTRACT_ID line updated.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$SCRIPT_DIR/.."
cd "$ROOT"

# Load env
if [ ! -f .env ]; then
  echo "ERROR: .env not found.  Copy .env.example → .env and fill in blanks."
  exit 1
fi
set -a; source .env; set +a

# Confirm testnet only
if [ "${NETWORK:-}" != "testnet" ]; then
  echo "ERROR: NETWORK must be 'testnet'.  This script has no mainnet path."
  exit 1
fi

WASM="target/wasm32v1-none/release/wick_fair_market.optimized.wasm"
if [ ! -f "$WASM" ]; then
  echo "WASM not found — running build first..."
  bash scripts/build.sh
fi

echo "Deploying to testnet..."
CONTRACT_ID=$(stellar contract deploy \
  --wasm "$WASM" \
  --source-account "$ADMIN_SECRET" \
  --network testnet)

echo "CONTRACT_ID=$CONTRACT_ID"
echo "$CONTRACT_ID" > .contract-id

# Update CONTRACT_ID in .env (or append if not present)
if grep -q "^CONTRACT_ID=" .env; then
  sed -i "s|^CONTRACT_ID=.*|CONTRACT_ID=$CONTRACT_ID|" .env
else
  echo "CONTRACT_ID=$CONTRACT_ID" >> .env
fi

echo "Deploy complete.  Run 'make init' next."
