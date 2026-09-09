use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Regressed,
    Improved,
    Unchanged,
    Inconclusive,
    Skipped,
    Error,
    Added,
    Removed,
    NoBaseWorker,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RoundStat {
    pub base: f64,
    pub current: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Detail {
    pub median_delta_pct: f64,
    pub sign_agreement: f64,
    pub base_spread_pct: f64,
    #[serde(default)]
    pub zero_baseline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeltaClass {
    Neg,
    Zero,
    Pos,
}

impl DeltaClass {
    fn of(delta: f64) -> Self {
        match delta.total_cmp(&0.0) {
            std::cmp::Ordering::Less => Self::Neg,
            std::cmp::Ordering::Equal => Self::Zero,
            std::cmp::Ordering::Greater => Self::Pos,
        }
    }
}

#[must_use]
pub fn judge_latency(rounds: &[RoundStat], threshold_pct: f64) -> (Verdict, Detail) {
    if rounds.is_empty() {
        return (Verdict::Error, Detail::default());
    }
    let zero_baseline = rounds.iter().any(|round| round.base == 0.0);
    let mut deltas: Vec<f64> = rounds
        .iter()
        .map(|round| {
            if round.base == 0.0 {
                if round.current == 0.0 {
                    0.0
                } else {
                    100.0 * threshold_pct
                }
            } else {
                (round.current / round.base - 1.0) * 100.0
            }
        })
        .collect();
    deltas.sort_by(f64::total_cmp);
    let median = deltas[deltas.len() / 2];
    let median_class = DeltaClass::of(median);
    #[expect(
        clippy::cast_precision_loss,
        reason = "round counts are small and only used for a percentage"
    )]
    let agree = deltas
        .iter()
        .filter(|delta| DeltaClass::of(**delta) == median_class)
        .count() as f64
        / deltas.len() as f64;
    let mut bases: Vec<f64> = rounds.iter().map(|round| round.base).collect();
    bases.sort_by(f64::total_cmp);
    let base_median = bases[bases.len() / 2];
    let spread = if base_median == 0.0 {
        0.0
    } else {
        (bases[bases.len() - 1] - bases[0]) / base_median * 100.0
    };
    let detail = Detail {
        median_delta_pct: median,
        sign_agreement: agree,
        base_spread_pct: spread,
        zero_baseline,
    };
    let over = median.abs() > threshold_pct;
    let verdict = match (over, agree >= 0.8, spread > threshold_pct) {
        (true, true, _) if median_class == DeltaClass::Pos => Verdict::Regressed,
        (true, true, _) if median_class == DeltaClass::Neg => Verdict::Improved,
        (true, false, _) | (false, _, true) => Verdict::Inconclusive,
        (false, _, false) | (true, true, _) => Verdict::Unchanged,
    };
    (verdict, detail)
}

#[must_use]
pub fn judge_value(
    base: u64,
    current: u64,
    budget_pct: Option<f64>,
    budget_abs: Option<u64>,
) -> (Verdict, Detail) {
    let difference = current.abs_diff(base);
    let over = if let Some(budget_pct) = budget_pct {
        if base == 0 {
            difference > 0
        } else {
            #[expect(
                clippy::cast_precision_loss,
                reason = "measurement values are converted to compare a percentage budget"
            )]
            let delta_pct = difference as f64 / base as f64 * 100.0;
            delta_pct > budget_pct
        }
    } else if let Some(budget_abs) = budget_abs {
        difference > budget_abs
    } else {
        false
    };
    let median_delta_pct = if base == 0 {
        0.0
    } else {
        #[expect(
            clippy::cast_precision_loss,
            reason = "measurement values are converted to report a percentage"
        )]
        {
            (current as f64 / base as f64 - 1.0) * 100.0
        }
    };
    let verdict = if !over {
        Verdict::Unchanged
    } else if current > base {
        Verdict::Regressed
    } else {
        Verdict::Improved
    };
    (
        verdict,
        Detail {
            median_delta_pct,
            sign_agreement: 1.0,
            base_spread_pct: 0.0,
            zero_baseline: base == 0,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{judge_latency, judge_value, RoundStat, Verdict};

    fn rounds(pairs: &[(f64, f64)]) -> Vec<RoundStat> {
        pairs
            .iter()
            .map(|&(base, current)| RoundStat { base, current })
            .collect()
    }

    fn assert_finite(detail: super::Detail) {
        assert!(detail.median_delta_pct.is_finite());
        assert!(detail.sign_agreement.is_finite());
        assert!(detail.base_spread_pct.is_finite());
    }

    #[test]
    fn zero_rounds_do_not_agree_with_positive_median() {
        let (v, d) = judge_latency(
            &rounds(&[
                (100.0, 110.0),
                (100.0, 110.0),
                (100.0, 110.0),
                (100.0, 100.0),
                (100.0, 100.0),
            ]),
            5.0,
        );
        assert_eq!(v, Verdict::Inconclusive);
        assert!((d.sign_agreement - 0.6).abs() < 1e-9);
    }

    #[test]
    fn all_zero_delta_rounds_are_unchanged() {
        let (v, d) = judge_latency(&rounds(&[(100.0, 100.0); 5]), 5.0);
        assert_eq!(v, Verdict::Unchanged);
        assert_finite(d);
    }

    #[test]
    fn zero_baseline_is_finite_and_unchanged_when_both_zero() {
        let (v, d) = judge_latency(&rounds(&[(0.0, 0.0); 5]), 5.0);
        assert_eq!(v, Verdict::Unchanged);
        assert!(d.zero_baseline);
        assert_finite(d);
    }

    #[test]
    fn zero_baseline_to_value_is_regressed_and_finite() {
        let (v, d) = judge_latency(&rounds(&[(0.0, 50.0); 5]), 5.0);
        assert_eq!(v, Verdict::Regressed);
        assert!(d.zero_baseline);
        assert_finite(d);
    }

    #[test]
    fn value_zero_baseline_is_finite() {
        let (v, d) = judge_value(0, 50, Some(5.0), None);
        assert_eq!(v, Verdict::Regressed);
        assert_finite(d);
    }


    #[test]
    fn regressed_when_median_over_threshold_and_signs_agree() {
        let (v, d) = judge_latency(
            &rounds(&[
                (100.0, 108.0),
                (100.0, 107.0),
                (100.0, 109.0),
                (100.0, 106.0),
                (100.0, 110.0),
            ]),
            5.0,
        );
        assert_eq!(v, Verdict::Regressed);
        assert!(d.median_delta_pct > 5.0);
    }

    #[test]
    fn improved_symmetric() {
        let (v, _) = judge_latency(
            &rounds(&[
                (100.0, 92.0),
                (100.0, 93.0),
                (100.0, 91.0),
                (100.0, 94.0),
                (100.0, 92.0),
            ]),
            5.0,
        );
        assert_eq!(v, Verdict::Improved);
    }

    #[test]
    fn inconclusive_when_signs_disagree() {
        let (v, _) = judge_latency(
            &rounds(&[
                (100.0, 110.0),
                (100.0, 90.0),
                (100.0, 112.0),
                (100.0, 88.0),
                (100.0, 111.0),
            ]),
            5.0,
        );
        assert_eq!(v, Verdict::Inconclusive);
    }

    #[test]
    fn inconclusive_when_base_spread_exceeds_threshold_and_delta_within() {
        let (v, d) = judge_latency(
            &rounds(&[
                (100.0, 101.0),
                (120.0, 121.0),
                (90.0, 91.0),
                (100.0, 100.0),
                (110.0, 111.0),
            ]),
            5.0,
        );
        assert_eq!(v, Verdict::Inconclusive);
        assert!(d.base_spread_pct > 5.0);
    }

    #[test]
    fn unchanged_within_threshold_and_quiet_base() {
        let (v, _) = judge_latency(
            &rounds(&[
                (100.0, 101.0),
                (101.0, 100.0),
                (100.0, 102.0),
                (100.0, 99.0),
                (101.0, 101.0),
            ]),
            5.0,
        );
        assert_eq!(v, Verdict::Unchanged);
    }

    #[test]
    fn value_compare_uses_pct_or_abs() {
        assert_eq!(
            judge_value(1000, 1004, Some(0.5), None).0,
            Verdict::Unchanged
        );
        assert_eq!(
            judge_value(1000, 1006, Some(0.5), None).0,
            Verdict::Regressed
        );
        assert_eq!(judge_value(10, 11, None, Some(0)).0, Verdict::Regressed);
        assert_eq!(judge_value(10, 9, None, Some(0)).0, Verdict::Improved);
        assert_eq!(judge_value(10, 10, None, Some(0)).0, Verdict::Unchanged);
    }
}
