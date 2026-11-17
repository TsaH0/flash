//! Latency summaries over per-event samples (nanoseconds), and event times.

use std::time::Instant;

/// An input event's time for the editor and the trace: nanoseconds from
/// `session` to when the input thread *read* it (not when the main loop
/// got to it), so a stall on the main thread cannot change undo grouping
/// or key-sequence timeouts. An event read before the session started is
/// at 0.
pub fn event_time(session: Instant, read_at: Instant) -> u64 {
    u64::try_from(read_at.saturating_duration_since(session).as_nanos()).unwrap_or(u64::MAX)
}

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

/// Per-event timings along the input path (PLAN Stage 6
/// instrumentation), in nanoseconds. Events handled in one batch share
/// the batch's layout and flush.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Timing {
    /// Event read (input thread) → frame flushed.
    pub total: u64,
    /// Event read → dequeued by the main loop.
    pub queue: u64,
    /// Dequeued → command done (key resolution, command, commit).
    pub command: u64,
    /// The commit inside the command (0 if none).
    pub commit: u64,
    /// Building the frame.
    pub layout: u64,
    /// Writing and flushing the frame.
    pub flush: u64,
    /// Bytes the frame wrote (0 without a frame): what the terminal still
    /// has to parse and repaint after `flush` returns (PLAN section 4).
    pub bytes: u64,
    /// Cells the frame wrote (0 without a frame).
    pub cells: u64,
}

/// Timings of many events, summarised per stage.
#[derive(Debug, Default)]
pub struct Stages {
    samples: Vec<Timing>,
}

impl Stages {
    pub fn push(&mut self, t: Timing) {
        self.samples.push(t);
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The last timing pushed.
    pub fn last(&self) -> Option<&Timing> {
        self.samples.last()
    }

    /// One JSON object: `fields`, then the total's counts and percentiles
    /// at the top level (as before Stage 6), then `"stages"` with each
    /// stage's percentiles. `None` without samples.
    pub fn to_json(&self, fields: &str) -> Option<String> {
        let pick = |f: fn(&Timing) -> u64| -> Vec<u64> { self.samples.iter().map(f).collect() };
        let total = summarize(&pick(|t| t.total))?;
        let mut json = total.to_json(fields);
        json.pop();
        json.push_str(",\"stages\":{");
        type Stage = (&'static str, fn(&Timing) -> u64);
        let stages: [Stage; 5] = [
            ("queue", |t| t.queue),
            ("command", |t| t.command),
            ("commit", |t| t.commit),
            ("layout", |t| t.layout),
            ("flush", |t| t.flush),
        ];
        // Not times: summarised in their own objects after "stages".
        let bytes = summarize(&pick(|t| t.bytes))?;
        let cells = summarize(&pick(|t| t.cells))?;
        for (i, (name, f)) in stages.into_iter().enumerate() {
            let s = summarize(&pick(f))?;
            if i > 0 {
                json.push(',');
            }
            json.push_str(&format!(
                r#""{name}":{{"p50_ns":{},"p95_ns":{},"p99_ns":{},"max_ns":{}}}"#,
                s.p50, s.p95, s.p99, s.max
            ));
        }
        json.push_str(&format!(
            r#"}},"frame_bytes":{{"p50":{},"p95":{},"p99":{},"max":{}}},"frame_cells":{{"p50":{},"p95":{},"p99":{},"max":{}}}}}"#,
            bytes.p50, bytes.p95, bytes.p99, bytes.max, cells.p50, cells.p95, cells.p99, cells.max
        ));
        Some(json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_time_is_read_time_not_now() {
        use std::time::Duration;
        let session = Instant::now();
        let read = session + Duration::from_millis(5);
        // However late it is handled, the time is when it was read.
        assert_eq!(event_time(session, read), 5_000_000);
        assert_eq!(event_time(read, session), 0, "read before the session");
    }

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

    #[test]
    fn stages_json() {
        let mut st = Stages::default();
        assert!(st.to_json("").is_none());
        for i in 1..=4 {
            st.push(Timing {
                total: 100 * i,
                queue: i,
                command: 10 * i,
                commit: 0,
                layout: 20,
                flush: 30,
                bytes: 1000 * i,
                cells: 10 * i,
            });
        }
        let json = st.to_json(r#""source":"t""#).unwrap();
        assert!(
            json.starts_with(r#"{"source":"t","events":4,"p50_ns":300,"#),
            "{json}"
        );
        assert!(json.contains(r#""stages":{"queue":{"p50_ns":3,"#), "{json}");
        assert!(
            json.contains(r#""flush":{"p50_ns":30,"p95_ns":30,"p99_ns":30,"max_ns":30}}"#),
            "{json}"
        );
        assert!(
            json.ends_with(r#","frame_bytes":{"p50":3000,"p95":4000,"p99":4000,"max":4000},"frame_cells":{"p50":30,"p95":40,"p99":40,"max":40}}"#),
            "{json}"
        );
        assert_eq!(st.len(), 4);
        assert_eq!(st.last().unwrap().total, 400);
    }
}
