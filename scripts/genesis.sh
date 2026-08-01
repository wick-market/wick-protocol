#!/usr/bin/env bash
# Open the first round for BTC, ETH, and SOL.
# create_round() is permissionless but the keeper calls it going forward.
# Run once after init.sh to bootstrap the live round series.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$SCRIPT_DIR/.."
cd "$ROOT"

if [ ! -f .env ]; then echo "ERROR: .env missing"; exit 1; fi
set -a; source .env; set +a

if [ -z "${CONTRACT_ID:-}" ]; then
  echo "ERROR: CONTRACT_ID not set in .env."; exit 1
fi

for ASSET in BTC ETH SOL; do
  echo "Creating first round for $ASSET..."
  ROUND_ID=$(stellar contract invoke \
    --id "$CONTRACT_ID" \
    --source-account "$ADMIN_SECRET" \
    --network testnet \
    -- create_round \
    --asset "$ASSET")
  echo "  $ASSET round_id=$ROUND_ID"
  echo "  lock at: strike_ts + $LOCK_OFFSET_SECS s"
  echo "  settles at: strike_ts + 300 s"
done

echo ""
echo "Genesis complete. Start the keeper: make keeper"
