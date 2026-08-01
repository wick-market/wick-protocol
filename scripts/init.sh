#!/usr/bin/env bash
# Call initialize() on the deployed Fair Market contract.
# Run once after deploy-testnet.sh succeeds.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$SCRIPT_DIR/.."
cd "$ROOT"

if [ ! -f .env ]; then echo "ERROR: .env missing"; exit 1; fi
set -a; source .env; set +a

if [ -z "${CONTRACT_ID:-}" ]; then
  echo "ERROR: CONTRACT_ID not set in .env.  Run deploy-testnet.sh first."
  exit 1
fi

echo "Initialising contract $CONTRACT_ID on testnet..."
stellar contract invoke \
  --id "$CONTRACT_ID" \
  --source-account "$ADMIN_SECRET" \
  --network testnet \
  -- initialize \
  --admin        "$ADMIN_ADDRESS" \
  --oracle       "$ORACLE_ADDRESS" \
  --token        "$XLM_SAC_ADDRESS" \
  --fee_bps      "$FEE_BPS" \
  --min_bet      "$MIN_BET_STROOPS" \
  --lock_offset  "$LOCK_OFFSET_SECS"

echo "Contract initialised."
echo ""
echo "Oracle decimals will be read from the oracle at init time and stored."
echo "Next step: run 'make genesis' to open the first round for each asset."
