#!/usr/bin/env bash
# Verify the testnet oracle is live and returning prices for BTC/ETH/SOL.
# Also confirms whether price() calls require XRF tokens (feeConfig warning).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$SCRIPT_DIR/.."
cd "$ROOT"

if [ ! -f .env ]; then echo "ERROR: .env missing"; exit 1; fi
set -a; source .env; set +a

ORACLE="${ORACLE_ADDRESS:-CCYOZJCOPG34LLQQ7N24YXBM7LL62R7ONMZ3G6WZAAYPB5OYKOMJRN63}"

echo "Checking oracle $ORACLE on testnet..."
echo ""

# Fetch decimals
echo "==> decimals()"
stellar contract invoke \
  --id "$ORACLE" \
  --source-account "$ADMIN_SECRET" \
  --network testnet \
  -- decimals
echo ""

# Fetch lastprice for each asset
for ASSET in BTC ETH SOL; do
  echo "==> lastprice($ASSET)"
  stellar contract invoke \
    --id "$ORACLE" \
    --source-account "$ADMIN_SECRET" \
    --network testnet \
    -- lastprice \
    --asset "{\"Other\":\"$ASSET\"}" 2>&1 || echo "  FAILED — may require XRF fee"
  echo ""
done

echo "If any call above shows an auth/fee error, the oracle requires XRF tokens."
echo "In that case, switch to the ReflectorBeam contract and set up XRF funding."
