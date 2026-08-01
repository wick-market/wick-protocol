#!/usr/bin/env bash
# Generate TypeScript contract bindings from the deployed fair-market contract.
# Output: packages/bindings/  (committed — wick-app vendors this directly)
#
# Re-run this whenever the contract is re-deployed with a new contract ID.
# Update the CONTRACT_ID below, then commit the regenerated output.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$SCRIPT_DIR/.."
cd "$ROOT"

CONTRACT_ID="CBJINJPV6DKXYGC3XUARWXLYQ3V6CAW2BH5PATAKEEIWSWOT5K4KEIAF"
NETWORK="testnet"
OUT_DIR="packages/bindings"

echo "Generating TypeScript bindings for $CONTRACT_ID on $NETWORK..."

stellar contract bindings typescript \
  --contract-id "$CONTRACT_ID" \
  --network "$NETWORK" \
  --output-dir "$OUT_DIR" \
  --overwrite

echo "Installing and building bindings..."
cd "$OUT_DIR"
npm install
npm run build

echo ""
echo "Done. Commit packages/bindings/ to the repo."
echo "Remember to update packages/bindings/addresses.ts if the contract ID changed."
