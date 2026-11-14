//! Latency summaries over per-event samples (nanoseconds).

/// Percentiles of a set of samples. Percentile `p` is the sorted sample at
/// index `floor(len · p)`, clamped, matching `flash-core`'s latency harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub events: usize,
    pub p50: u64,
    pub p95: u64,
    pub p99: u64,
    pub max: u64,
}

/// Summarises `samples`, or `None` if there are none.
pub fn summarize(samples: &[u64]) -> Option<Summary> {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let max = *sorted.last()?;
    let pct = |p: f64| sorted[((sorted.len() as f64 * p) as usize).min(sorted.len() - 1)];
    Some(Summary {
        events: sorted.len(),
        p50: pct(0.50),
        p95: pct(0.95),
        p99: pct(0.99),
        max,
    })
}

impl Summary {
    /// One JSON object: `fields` (already-encoded `"key":value` pairs, or
    /// empty) followed by the counts and percentiles.
    pub fn to_json(&self, fields: &str) -> String {
        let sep = if fields.is_empty() { "" } else { "," };
        format!(
            r#"{{{fields}{sep}"events":{},"p50_ns":{},"p95_ns":{},"p99_ns":{},"max_ns":{}}}"#,
            self.events, self.p50, self.p95, self.p99, self.max
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_percentiles() {
        assert!(summarize(&[]).is_none());
        let samples: Vec<u64> = (1..=100).rev().collect();
        let s = summarize(&samples).unwrap();
        assert_eq!(
            (s.events, s.p50, s.p95, s.p99, s.max),
            (100, 51, 96, 100, 100)
        );
        assert_eq!(
            s.to_json(r#""source":"x""#),
            r#"{"source":"x","events":100,"p50_ns":51,"p95_ns":96,"p99_ns":100,"max_ns":100}"#
        );
    }
}
