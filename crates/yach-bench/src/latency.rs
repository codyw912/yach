use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencySummary {
    pub label: Option<String>,
    pub count: usize,
    pub p50: Option<Duration>,
    pub p95: Option<Duration>,
    pub p99: Option<Duration>,
    pub max: Option<Duration>,
}

impl LatencySummary {
    #[must_use]
    pub fn empty(label: impl Into<Option<String>>) -> Self {
        Self {
            label: label.into(),
            count: 0,
            p50: None,
            p95: None,
            p99: None,
            max: None,
        }
    }

    #[must_use]
    pub fn from_samples(label: impl Into<Option<String>>, samples: &[Duration]) -> Self {
        if samples.is_empty() {
            return Self::empty(label);
        }

        let mut sorted = samples.to_vec();
        sorted.sort_unstable();

        let count = sorted.len();
        Self {
            label: label.into(),
            count,
            p50: percentile_nearest_rank(&sorted, 50),
            p95: percentile_nearest_rank(&sorted, 95),
            p99: percentile_nearest_rank(&sorted, 99),
            max: sorted.last().copied(),
        }
    }

    #[must_use]
    pub fn has_data(&self) -> bool {
        self.count > 0
    }
}

fn percentile_nearest_rank(sorted_samples: &[Duration], percentile: u32) -> Option<Duration> {
    if sorted_samples.is_empty() {
        return None;
    }

    let len = sorted_samples.len();
    let rank = (len.saturating_mul(percentile as usize)).div_ceil(100);
    let index = rank.saturating_sub(1).min(len.saturating_sub(1));
    sorted_samples.get(index).copied()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::LatencySummary;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn summarizes_sorted_samples() {
        let summary = LatencySummary::from_samples(None, &[ms(1), ms(2), ms(3), ms(4), ms(5)]);

        assert_eq!(summary.count, 5);
        assert_eq!(summary.p50, Some(ms(3)));
        assert_eq!(summary.p95, Some(ms(5)));
        assert_eq!(summary.p99, Some(ms(5)));
        assert_eq!(summary.max, Some(ms(5)));
    }

    #[test]
    fn one_sample_populates_all_percentiles() {
        let summary = LatencySummary::from_samples(Some(String::from("single")), &[ms(7)]);

        assert_eq!(summary.label.as_deref(), Some("single"));
        assert_eq!(summary.count, 1);
        assert_eq!(summary.p50, Some(ms(7)));
        assert_eq!(summary.p95, Some(ms(7)));
        assert_eq!(summary.p99, Some(ms(7)));
        assert_eq!(summary.max, Some(ms(7)));
    }

    #[test]
    fn empty_samples_are_no_data() {
        let summary = LatencySummary::from_samples(Some(String::from("empty")), &[]);

        assert_eq!(summary.label.as_deref(), Some("empty"));
        assert_eq!(summary.count, 0);
        assert_eq!(summary.p50, None);
        assert_eq!(summary.p95, None);
        assert_eq!(summary.p99, None);
        assert_eq!(summary.max, None);
        assert!(!summary.has_data());
    }

    #[test]
    fn unsorted_samples_match_sorted_samples() {
        let sorted = LatencySummary::from_samples(None, &[ms(1), ms(2), ms(3), ms(4), ms(5)]);
        let unsorted = LatencySummary::from_samples(None, &[ms(5), ms(1), ms(4), ms(2), ms(3)]);

        assert_eq!(unsorted, sorted);
    }
}
