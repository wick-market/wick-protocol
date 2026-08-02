#!/usr/bin/env bash
# Full mechanical test: 3 wallets bet on opposite sides, wait for settlement,
# verify Ninetails payouts (winners profit, losers get partial refund).
#
# Usage: bash scripts/test-round.sh
set -euo pipefail

CONTRACT=CB3UZK2OQZ3CNJ2R64N7NI3EW6MEKFJDC5TTXYEBY5BL2EL2CLPNHDD2
NETWORK=testnet

W1=GDNDQAV6RZFEMJCGV3AM7QMOLE3LD7G6HYBXI3M43K2V4M35PTNBDQIS  # test-wallet-1
W2=GDHNRHK5RUG2F72LQCGZWACPZC3FFV7TKUEXSLPMBDNUSFYPZ6WF7VXH  # test-wallet-2
W3=GDDDUUJOIVRVBYQ4PMVACRA2CYBXQMKKFTTWWANHPGS3MVRKKOESJGX5  # test-wallet-3

invoke() {
  stellar contract invoke --id $CONTRACT --source-account "$1" --network $NETWORK --send=yes -- "${@:2}" 2>&1 | grep -E "Success|Event|error" | head -3
}

echo ""
echo "══════════════════════════════════════"
echo "  WICK PREDICTION MARKET — TEST RUN  "
echo "══════════════════════════════════════"
echo ""

# ── Step 1: Open round ────────────────────────────────────────────────────────
echo "▶ Step 1: Opening new round..."
ROUND_OUTPUT=$(stellar contract invoke --id $CONTRACT --source-account wick-admin --network $NETWORK --send=yes -- create_round 2>&1)
ROUND_ID=$(echo "$ROUND_OUTPUT" | grep '"u64"' | head -1 | python3 -c "import sys,re; m=re.search(r'\"u64\":\"(\d+)\"', sys.stdin.read()); print(m.group(1))" 2>/dev/null || echo "")
if [ -z "$ROUND_ID" ]; then
  # Try to get current round id if DuplicateRound
  ROUND_ID=$(stellar contract invoke --id $CONTRACT --source-account wick-admin --network $NETWORK -- current_round_id 2>&1 | grep -v "^ℹ" | tr -d '[:space:]')
  echo "  (Using existing round $ROUND_ID)"
else
  echo "  Round #$ROUND_ID opened ✓"
fi

# Show round details
ROUND=$(stellar contract invoke --id $CONTRACT --source-account wick-admin --network $NETWORK -- get_round --round_id "$ROUND_ID" 2>&1 | grep -v "^ℹ")
STRIKE=$(echo $ROUND | python3 -c "import sys,json; d=json.load(sys.stdin); print(int(d['strike'])/1e14)" 2>/dev/null || echo "?")
LOCK_TS=$(echo $ROUND | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['lock_ts'])" 2>/dev/null || echo "?")
SETTLE_TS=$(echo $ROUND | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['settle_ts'])" 2>/dev/null || echo "?")
STATUS=$(echo $ROUND | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['status'])" 2>/dev/null || echo "?")
echo "  Strike price: \$$STRIKE USD"
echo "  Betting closes: $(date -d @$LOCK_TS 2>/dev/null || echo "ts=$LOCK_TS")"
echo "  Settles at:     $(date -d @$SETTLE_TS 2>/dev/null || echo "ts=$SETTLE_TS")"
echo "  Status: $STATUS"
echo ""

# ── Step 2: Place bets ────────────────────────────────────────────────────────
echo "▶ Step 2: Placing bets..."
echo "  wallet-1 → 200 XLM ABOVE (early bettor)"
invoke test-wallet-1 bet_above --user "$W1" --round_id "$ROUND_ID" --amount 2000000000

echo "  wallet-2 → 100 XLM BELOW"
invoke test-wallet-2 bet_below --user "$W2" --round_id "$ROUND_ID" --amount 1000000000

echo "  wallet-3 → 150 XLM BELOW"
invoke test-wallet-3 bet_below --user "$W3" --round_id "$ROUND_ID" --amount 1500000000

echo ""
echo "  Pools:"
ROUND2=$(stellar contract invoke --id $CONTRACT --source-account wick-admin --network $NETWORK -- get_round --round_id "$ROUND_ID" 2>&1 | grep -v "^ℹ")
PA=$(echo $ROUND2 | python3 -c "import sys,json; d=json.load(sys.stdin); print(int(d['pool_above'])/1e7)" 2>/dev/null)
PB=$(echo $ROUND2 | python3 -c "import sys,json; d=json.load(sys.stdin); print(int(d['pool_below'])/1e7)" 2>/dev/null)
echo "  Above: ${PA} XLM   Below: ${PB} XLM"
echo ""

# ── Step 3: Wait and show expected payouts ────────────────────────────────────
echo "▶ Step 3: Expected payouts (fee=2%)"
echo ""
echo "  IF ABOVE WINS:"
echo "  wallet-1 (200 XLM staked): keeps 200 + share of 245 XLM Below pool"
echo "  wallet-2 (100 XLM staked): partial refund ~some XLM from Ninetails"
echo "  wallet-3 (150 XLM staked): partial refund ~some XLM from Ninetails"
echo ""
echo "  IF BELOW WINS:"
echo "  wallet-2 + wallet-3 share the 196 XLM Above pool (weighted by boost)"
echo "  wallet-1 gets partial Ninetails refund"
echo ""

# ── Step 4: Wait for settle_ts ────────────────────────────────────────────────
NOW=$(date +%s)
WAIT=$((SETTLE_TS - NOW))
if [ "$WAIT" -gt 0 ]; then
  echo "▶ Step 4: Waiting ${WAIT}s for oracle tick at settle_ts..."
  echo "  (Oracle updates every 5 minutes — this is the Reflector cadence)"
  sleep $WAIT
  sleep 10  # extra buffer for ledger propagation
fi

# ── Step 5: Settle ────────────────────────────────────────────────────────────
echo ""
echo "▶ Step 5: Settling round #$ROUND_ID..."
invoke wick-admin settle --round_id "$ROUND_ID"

ROUND3=$(stellar contract invoke --id $CONTRACT --source-account wick-admin --network $NETWORK -- get_round --round_id "$ROUND_ID" 2>&1 | grep -v "^ℹ")
OUTCOME=$(echo $ROUND3 | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['outcome'])" 2>/dev/null)
SP=$(echo $ROUND3 | python3 -c "import sys,json; d=json.load(sys.stdin); print(int(d['settle_price'])/1e14)" 2>/dev/null)
echo "  Outcome: $OUTCOME"
echo "  Settle price: \$$SP"
echo ""

# ── Step 6: Claim ─────────────────────────────────────────────────────────────
echo "▶ Step 6: Claiming payouts..."
echo ""

for i in 1 2 3; do
  WALLET_KEY="test-wallet-$i"
  ADDR=$(stellar keys public-key "$WALLET_KEY")
  BAL_BEFORE=$(curl -s "https://horizon-testnet.stellar.org/accounts/$ADDR" | python3 -c "import json,sys; d=json.load(sys.stdin); print(next(b['balance'] for b in d['balances'] if b['asset_type']=='native'))" 2>/dev/null || echo "?")
  invoke "$WALLET_KEY" claim --user "$ADDR" --round_id "$ROUND_ID" > /dev/null 2>&1 || true
  BAL_AFTER=$(curl -s "https://horizon-testnet.stellar.org/accounts/$ADDR" | python3 -c "import json,sys; d=json.load(sys.stdin); print(next(b['balance'] for b in d['balances'] if b['asset_type']=='native'))" 2>/dev/null || echo "?")
  echo "  wallet-$i: $BAL_BEFORE XLM → $BAL_AFTER XLM"
done

echo ""
echo "══════════════════════════════════════"
echo "  TEST COMPLETE"
echo "══════════════════════════════════════"
