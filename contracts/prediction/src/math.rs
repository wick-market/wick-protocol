/// Pure payout math — no Env dependency, trivially unit-testable.
///
/// # Invariant (enforced by tests)
/// sum(claimable_payout(bet_i)) + treasury_fee <= total_amount
///
/// Integer division truncates; dust (≤ 1 stroop per bettor) accrues to
/// treasury.  This is intentional and cannot cause overpayment.

/// Compute treasury fee and reward pool for a settled round.
///
/// Returns (treasury_fee, reward_pool).
/// Panics on overflow — checked arithmetic throughout.
pub fn compute_pools(total_amount: i128, fee_bps: u32) -> (i128, i128) {
    let fee = total_amount
        .checked_mul(fee_bps as i128)
        .expect("fee mul overflow")
        .checked_div(10_000)
        .expect("fee div overflow");
    let pool = total_amount.checked_sub(fee).expect("pool sub overflow");
    (fee, pool)
}

/// Compute a single bettor's payout from the reward pool.
///
/// payout = bet_amount * reward_pool / winning_side_amount
///
/// Returns 0 if the caller is not on the winning side (caller must check
/// direction before calling) or if inputs are zero.
pub fn compute_payout(bet_amount: i128, reward_pool: i128, winning_side_amount: i128) -> i128 {
    if bet_amount == 0 || reward_pool == 0 || winning_side_amount == 0 {
        return 0;
    }
    bet_amount
        .checked_mul(reward_pool)
        .expect("payout mul overflow")
        .checked_div(winning_side_amount)
        .expect("payout div overflow")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── compute_pools ─────────────────────────────────────────────────────────

    #[test]
    fn pools_300bps() {
        let (fee, pool) = compute_pools(1_000_000_000, 300);
        assert_eq!(fee, 30_000_000);
        assert_eq!(pool, 970_000_000);
    }

    #[test]
    fn pools_zero_amount() {
        let (fee, pool) = compute_pools(0, 300);
        assert_eq!(fee, 0);
        assert_eq!(pool, 0);
    }

    #[test]
    fn pools_zero_fee() {
        let total = 500_000_000i128;
        let (fee, pool) = compute_pools(total, 0);
        assert_eq!(fee, 0);
        assert_eq!(pool, total);
    }

    // ── compute_payout ────────────────────────────────────────────────────────

    #[test]
    fn payout_proportional() {
        // 3 bettors on winning side: 100, 200, 300 stroops.  Pool = 900.
        let pool = 900i128;
        let winning_side = 600i128;
        assert_eq!(compute_payout(100, pool, winning_side), 150);
        assert_eq!(compute_payout(200, pool, winning_side), 300);
        assert_eq!(compute_payout(300, pool, winning_side), 450);
    }

    #[test]
    fn payout_zero_inputs() {
        assert_eq!(compute_payout(0, 1000, 500), 0);
        assert_eq!(compute_payout(100, 0, 500), 0);
        assert_eq!(compute_payout(100, 1000, 0), 0);
    }

    // ── Conservation invariant ────────────────────────────────────────────────
    //
    // sum(payouts) + treasury_fee <= total_amount   MUST always hold.
    // A payout system that can pay out more than collected is the failure mode
    // that kills the protocol.

    #[test]
    fn conservation_simple() {
        let total = 1_000_000_000i128;
        let (fee, pool) = compute_pools(total, 300);

        // Suppose up side = 600_000_000, down side loses.
        let winning_side = 600_000_000i128;
        let bettors = [100_000_000i128, 200_000_000i128, 300_000_000i128];

        let sum_payouts: i128 = bettors
            .iter()
            .map(|&b| compute_payout(b, pool, winning_side))
            .sum();

        assert!(
            sum_payouts + fee <= total,
            "invariant violated: sum_payouts={sum_payouts} fee={fee} total={total}"
        );
    }

    #[test]
    fn conservation_many_small_bettors() {
        // 1000 bettors of 1_000_000 each; total = 1_000_000_000
        let n = 1000i128;
        let per_bet = 1_000_000i128;
        let total = n * per_bet;
        let (fee, pool) = compute_pools(total, 300);

        // All on winning side.
        let winning_side = total;
        let sum_payouts: i128 = (0..n).map(|_| compute_payout(per_bet, pool, winning_side)).sum();

        assert!(
            sum_payouts + fee <= total,
            "invariant violated with many bettors"
        );
        // Dust should be tiny (≤ n stroops).
        let dust = total - sum_payouts - fee;
        assert!(dust <= n, "unexpected dust amount: {dust}");
    }

    #[test]
    fn conservation_extreme_pool_split() {
        // One winner with 1 stroop; losers had 999_999_999 stroops.
        let total = 1_000_000_000i128;
        let (fee, pool) = compute_pools(total, 300);
        let winning_side = 1i128;

        let payout = compute_payout(1, pool, winning_side);

        // payout can't exceed pool.
        assert!(payout <= pool, "payout {payout} exceeds pool {pool}");
        assert!(payout + fee <= total);
    }

    #[test]
    fn conservation_fuzz() {
        // Pseudo-random sweep over (total, fee_bps, split_pct).
        // All (total, fee_bps, winning_units) tuples must satisfy:
        //   winning_units * pool < i128::MAX, where pool ≈ total * (1 - fee_bps/10000).
        // With total ≤ 1e18 and winning_units ≤ total, product ≤ 1e36 << i128::MAX.
        let cases: &[(i128, u32, i128)] = &[
            (1, 300, 1),
            (100, 0, 50),
            (100, 10_000, 50),
            (1_000_000_000_000_000i128, 300, 500_000_000_000_000),
            (1_000_000_000_000_000_000i128, 300, 500_000_000_000_000_000),
            (99_999_999_999i128, 150, 75_000_000_000),
            (7, 300, 4),
            (1_000_000, 300, 1),
            (1_000_000, 300, 999_999),
        ];

        for &(total, fee_bps, winning_units) in cases {
            if total <= 0 || winning_units <= 0 || winning_units > total {
                continue;
            }
            let (fee, pool) = compute_pools(total, fee_bps);
            let losing_units = total - winning_units;
            // Simulate a single bettor holding all the winning side.
            let payout = compute_payout(winning_units, pool, winning_units);
            assert!(
                payout + fee <= total,
                "invariant violated: total={total} fee_bps={fee_bps} winning={winning_units} payout={payout} fee={fee}"
            );
            // Split into two equal bettors on winning side.
            let half = winning_units / 2;
            let p1 = compute_payout(half, pool, winning_units);
            let p2 = compute_payout(winning_units - half, pool, winning_units);
            assert!(
                p1 + p2 + fee <= total,
                "split invariant violated"
            );
        }
    }

    #[test]
    fn conservation_extreme_values() {
        // Realistic upper bound: 100 billion XLM in stroops (far more than max supply).
        // bet_amount * pool must fit in i128; with both ≤ 1e18, product ≤ 1e36 < i128::MAX.
        let total = 1_000_000_000_000_000_000i128; // 10^18 stroops = 100 billion XLM
        let (fee, pool) = compute_pools(total, 300);
        assert!(fee >= 0 && pool >= 0);
        assert!(fee + pool <= total);

        let payout = compute_payout(total / 2, pool, total / 2);
        assert!(payout + fee <= total);

        // Also verify fee calc doesn't overflow at 9_999 bps.
        let (fee2, pool2) = compute_pools(total, 9_999);
        assert!(fee2 + pool2 <= total);
    }
}
