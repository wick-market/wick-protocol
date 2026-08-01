# Fixtures

Realistic JSON responses for every GET endpoint. Used by wick-app's mock mode.

## Parimutuel math used in these fixtures

fee_bps = 200 (2%)

For the SETTLED_UP round:
  pool_up   = 8_000_000_000 stroops (800 XLM)
  pool_down = 2_000_000_000 stroops (200 XLM)
  total     = 10_000_000_000
  fee       = 10_000_000_000 * 200 / 10_000 = 200_000_000 (20 XLM)
  distributed = 9_800_000_000 (980 XLM)

  Bettor A: 1_000_000_000 UP → 1_000_000_000 * 9_800_000_000 / 8_000_000_000 = 1_225_000_000
  Bettor B: 2_000_000_000 UP → 2_000_000_000 * 9_800_000_000 / 8_000_000_000 = 2_450_000_000
  Bettor C: 5_000_000_000 UP → 5_000_000_000 * 9_800_000_000 / 8_000_000_000 = 6_125_000_000
  sum(payouts) = 9_800_000_000 = distributed ✓
  Bettor D: 2_000_000_000 DOWN → loses → payout = 0

  conservation: sum(payouts) + fee = 9_800_000_000 + 200_000_000 = 10_000_000_000 = total ✓

For the VOID round: claimable = original amount, no fee deducted.
