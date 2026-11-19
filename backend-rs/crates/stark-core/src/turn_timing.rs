//! Turn timing profile, following Codex `core/src/turn_timing.rs`.
//!
//! Codex records time-to-first-token (TTFT), time-to-first-message (TTFM), and
//! the sampling / compaction / tool-blocking split for its analytics sink. Stark
//! has no analytics pipeline, so the same profile is emitted once at the end of
//! every run as a `delta` event with `kind: "turn_timing"`. It is not
//! model-visible and does not change prompts, tool output, or the tool contract.
//!
//! Phase accounting matches Codex: one phase can be active at a time; time
//! between phases before the first sample is `before_first_sampling`, time
//! between samples is `between_sampling_overhead`, and time after the last
//! sample is `after_last_sampling`. Rounding from millisecond truncation is
//! assigned to the phase that was active at completion.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// One measured phase. Mirrors Codex `TurnProfilePhase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Sampling,
    Compaction,
    ToolBlocking,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TurnTimingReport {
    pub time_to_first_token_ms: Option<u64>,
    pub time_to_first_message_ms: Option<u64>,
    pub before_first_sampling_ms: u64,
    pub sampling_ms: u64,
    pub compaction_ms: u64,
    pub between_sampling_overhead_ms: u64,
    pub tool_blocking_ms: u64,
    pub after_last_sampling_ms: u64,
    pub sampling_request_count: u32,
    pub sampling_retry_count: u32,
    pub tools_change_count: u32,
    pub total_ms: u64,
}

impl TurnTimingReport {
    /// Wire shape for `Delta::status`. All fields are additive: clients that do
    /// not know `turn_timing` ignore the event.
    pub(crate) fn as_json(&self) -> Value {
        json!({
            "kind": "turn_timing",
            "time_to_first_token_ms": self.time_to_first_token_ms,
            "time_to_first_message_ms": self.time_to_first_message_ms,
            "before_first_sampling_ms": self.before_first_sampling_ms,
            "sampling_ms": self.sampling_ms,
            "compaction_ms": self.compaction_ms,
            "between_sampling_overhead_ms": self.between_sampling_overhead_ms,
            "tool_blocking_ms": self.tool_blocking_ms,
            "after_last_sampling_ms": self.after_last_sampling_ms,
            "sampling_request_count": self.sampling_request_count,
            "sampling_retry_count": self.sampling_retry_count,
            "tools_change_count": self.tools_change_count,
            "total_ms": self.total_ms,
        })
    }
}

/// Per-run timing state. The run loop is a single task, so this is a plain
/// struct rather than Codex's `Arc<Mutex<..>>`.
pub(crate) struct TurnTiming {
    started_at: Instant,
    first_token_at: Option<Instant>,
    first_message_at: Option<Instant>,
    last_transition_at: Instant,
    active_phase: Option<Phase>,
    seen_sampling: bool,
    before_first_sampling: Duration,
    sampling: Duration,
    compaction: Duration,
    between_sampling_overhead: Duration,
    tool_blocking: Duration,
    pending_idle_after_sampling: Duration,
    sampling_request_count: u32,
    sampling_retry_count: u32,
    tools_change_count: u32,
    completed: Option<TurnTimingReport>,
}

impl TurnTiming {
    pub(crate) fn new() -> Self {
        let now = Instant::now();
        Self {
            started_at: now,
            first_token_at: None,
            first_message_at: None,
            last_transition_at: now,
            active_phase: None,
            seen_sampling: false,
            before_first_sampling: Duration::ZERO,
            sampling: Duration::ZERO,
            compaction: Duration::ZERO,
            between_sampling_overhead: Duration::ZERO,
            tool_blocking: Duration::ZERO,
            pending_idle_after_sampling: Duration::ZERO,
            sampling_request_count: 0,
            sampling_retry_count: 0,
            tools_change_count: 0,
            completed: None,
        }
    }

    /// First provider token: text, reasoning, or the first tool-call delta.
    /// Codex records TTFT on content deltas and completed function-call items.
    pub(crate) fn record_first_token(&mut self) {
        if self.first_token_at.is_none() {
            self.first_token_at = Some(Instant::now());
        }
    }

    /// First completed assistant message item (TTFM).
    pub(crate) fn record_first_message(&mut self) {
        if self.first_message_at.is_none() {
            self.first_message_at = Some(Instant::now());
        }
    }

    /// Start a phase. Returns false when another phase is still active or the
    /// profile is complete, matching Codex `begin_*`.
    pub(crate) fn begin_phase(&mut self, phase: Phase) -> bool {
        if self.completed.is_some() || self.active_phase.is_some() {
            return false;
        }
        self.advance(Instant::now());
        if phase == Phase::Sampling {
            if self.seen_sampling {
                self.between_sampling_overhead +=
                    std::mem::take(&mut self.pending_idle_after_sampling);
            }
            self.seen_sampling = true;
            self.sampling_request_count = self.sampling_request_count.saturating_add(1);
        }
        self.active_phase = Some(phase);
        true
    }

    pub(crate) fn end_phase(&mut self, phase: Phase) {
        if self.completed.is_some() || self.active_phase != Some(phase) {
            return;
        }
        self.advance(Instant::now());
        self.active_phase = None;
    }

    pub(crate) fn record_sampling_retry(&mut self) {
        if self.completed.is_none() {
            self.sampling_retry_count = self.sampling_retry_count.saturating_add(1);
        }
    }

    /// The advertised tool set changed (for example `tool_search` activated a
    /// deferred MCP tool).
    pub(crate) fn record_tools_change(&mut self) {
        if self.completed.is_none() {
            self.tools_change_count = self.tools_change_count.saturating_add(1);
        }
    }

    /// Freeze the profile. Idempotent: later calls return the same report.
    pub(crate) fn complete(&mut self) -> TurnTimingReport {
        if let Some(report) = &self.completed {
            return report.clone();
        }
        let now = Instant::now();
        let final_phase = self.active_phase;
        self.advance(now);
        let after_last_sampling = if self.seen_sampling {
            std::mem::take(&mut self.pending_idle_after_sampling)
        } else {
            Duration::ZERO
        };

        let mut report = TurnTimingReport {
            time_to_first_token_ms: self
                .first_token_at
                .and_then(|at| at.checked_duration_since(self.started_at))
                .map(duration_ms),
            time_to_first_message_ms: self
                .first_message_at
                .and_then(|at| at.checked_duration_since(self.started_at))
                .map(duration_ms),
            before_first_sampling_ms: duration_ms(self.before_first_sampling),
            sampling_ms: duration_ms(self.sampling),
            compaction_ms: duration_ms(self.compaction),
            between_sampling_overhead_ms: duration_ms(self.between_sampling_overhead),
            tool_blocking_ms: duration_ms(self.tool_blocking),
            after_last_sampling_ms: duration_ms(after_last_sampling),
            sampling_request_count: self.sampling_request_count,
            sampling_retry_count: self.sampling_retry_count,
            tools_change_count: self.tools_change_count,
            total_ms: duration_ms(now.saturating_duration_since(self.started_at)),
        };

        let classified = report
            .before_first_sampling_ms
            .saturating_add(report.sampling_ms)
            .saturating_add(report.compaction_ms)
            .saturating_add(report.between_sampling_overhead_ms)
            .saturating_add(report.tool_blocking_ms)
            .saturating_add(report.after_last_sampling_ms);
        let rounding = report.total_ms.saturating_sub(classified);
        match final_phase {
            Some(Phase::Sampling) => report.sampling_ms += rounding,
            Some(Phase::Compaction) => report.compaction_ms += rounding,
            Some(Phase::ToolBlocking) => report.tool_blocking_ms += rounding,
            None if self.seen_sampling => report.after_last_sampling_ms += rounding,
            None => report.before_first_sampling_ms += rounding,
        }

        self.active_phase = None;
        self.completed = Some(report.clone());
        report
    }

    fn advance(&mut self, now: Instant) {
        let previous = std::mem::replace(&mut self.last_transition_at, now);
        let elapsed = now.saturating_duration_since(previous);
        match self.active_phase {
            Some(Phase::Sampling) => self.sampling += elapsed,
            Some(Phase::Compaction) => self.compaction += elapsed,
            Some(Phase::ToolBlocking) => self.tool_blocking += elapsed,
            None if self.seen_sampling => self.pending_idle_after_sampling += elapsed,
            None => self.before_first_sampling += elapsed,
        }
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn phases_are_exclusive_and_split_the_turn() {
        let mut timing = TurnTiming::new();
        assert!(timing.begin_phase(Phase::Sampling));
        // A second phase cannot start while sampling is active.
        assert!(!timing.begin_phase(Phase::ToolBlocking));
        sleep(Duration::from_millis(10));
        timing.end_phase(Phase::Sampling);
        assert!(timing.begin_phase(Phase::ToolBlocking));
        sleep(Duration::from_millis(10));
        timing.end_phase(Phase::ToolBlocking);
        let report = timing.complete();
        assert!(report.sampling_ms >= 5, "{report:?}");
        assert!(report.tool_blocking_ms >= 5, "{report:?}");
        assert_eq!(report.sampling_request_count, 1);
    }

    #[test]
    fn ttft_and_ttfm_are_recorded_once() {
        let mut timing = TurnTiming::new();
        sleep(Duration::from_millis(5));
        timing.record_first_token();
        sleep(Duration::from_millis(5));
        timing.record_first_token();
        timing.record_first_message();
        let report = timing.complete();
        let ttft = report.time_to_first_token_ms.expect("ttft");
        let ttfm = report.time_to_first_message_ms.expect("ttfm");
        assert!(ttft >= 3, "{report:?}");
        assert!(ttfm >= ttft, "{report:?}");
        assert_eq!(timing.complete(), report);
    }

    #[test]
    fn retries_and_tool_changes_are_counted() {
        let mut timing = TurnTiming::new();
        timing.record_sampling_retry();
        timing.record_sampling_retry();
        timing.record_tools_change();
        let report = timing.complete();
        assert_eq!(report.sampling_retry_count, 2);
        assert_eq!(report.tools_change_count, 1);
        // A completed profile ignores later events.
        timing.record_sampling_retry();
        assert_eq!(timing.complete().sampling_retry_count, 2);
    }

    #[test]
    fn report_is_additive_json() {
        let mut timing = TurnTiming::new();
        let report = timing.complete();
        let value = report.as_json();
        assert_eq!(value["kind"], "turn_timing");
        assert!(value.get("sampling_ms").is_some());
        assert!(value.get("tools_change_count").is_some());
    }
}
