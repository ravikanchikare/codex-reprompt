//! Data types for the `/reprompt` every-turn prompt refinement feature.
//!
//! These structs define the structured output schema returned by the refinement
//! API call, runtime configuration, and overlay state machine types.

use serde::Deserialize;
use serde::Serialize;
use std::time::Duration;
use std::time::Instant;

// ---------------------------------------------------------------------------
// TaskType — the detected category of the user's request
// ---------------------------------------------------------------------------

/// Classification of the task the user is asking the agent to perform.
///
/// The refinement model emits this as part of `RepromptResult` so that
/// task-type-specific rules can be applied (e.g. "require regression test"
/// only for bugfix tasks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TaskType {
    Bugfix,
    Feature,
    Refactor,
    Analysis,
    Review,
}

impl std::fmt::Display for TaskType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bugfix => write!(f, "bugfix"),
            Self::Feature => write!(f, "feature"),
            Self::Refactor => write!(f, "refactor"),
            Self::Analysis => write!(f, "analysis"),
            Self::Review => write!(f, "review"),
        }
    }
}

// ---------------------------------------------------------------------------
// RepromptResult — structured output from the refinement API
// ---------------------------------------------------------------------------

/// The structured JSON output returned by the refinement model.
///
/// Field names use `camelCase` in the JSON and are mapped to `snake_case`
/// in Rust via serde rename.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepromptResult {
    /// The improved/refined prompt text.
    pub refined_prompt: String,

    /// Which user-defined rules were applied during refinement.
    pub applied_rules: Vec<String>,

    /// Human-readable explanation of why these changes were made.
    /// Shown when the user presses `[r]` in the overlay.
    pub reasoning: String,

    /// The detected (or inherited) task category.
    pub task_type: TaskType,

    /// `false` when the refinement barely changed the original input.
    /// When `false`, the overlay is skipped and the original is sent through.
    pub was_substantive_change: bool,
}

// ---------------------------------------------------------------------------
// RepromptConfig — user-configurable settings for reprompt mode
// ---------------------------------------------------------------------------

/// Runtime configuration for the `/reprompt` feature, loaded from
/// `~/.codex/config.toml` and optionally `~/.codex/agents/reprompt*.toml`.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct RepromptConfig {
    /// Whether reprompt mode is currently active.
    pub enabled: bool,

    /// Model to use for the refinement call (cheap + fast).
    pub model: String,

    /// Name of the reprompt profile to use (selects `~/.codex/reprompt/<name>.toml`).
    pub profile_name: Option<String>,

    /// Skip refinement for inputs shorter than this many characters.
    pub min_length: usize,

    /// Show a diff between original and refined text in the overlay.
    pub show_diff: bool,

    /// Seconds to wait before auto-accepting the refinement.
    /// `0` means manual-only (no auto-accept).
    pub auto_accept_delay: Duration,
}

impl Default for RepromptConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: "o4-mini".to_string(),
            profile_name: None,
            min_length: 20,
            show_diff: false,
            auto_accept_delay: Duration::from_secs(15),
        }
    }
}

// ---------------------------------------------------------------------------
// RepromptAuthInfo — resolved credentials for the refinement API call
// ---------------------------------------------------------------------------

/// Resolved authentication credentials for the reprompt refinement API call.
#[derive(Debug, Clone)]
pub(crate) struct RepromptAuthInfo {
    /// Bearer token (API key or ChatGPT OAuth access token).
    pub token: String,
    /// Base URL for the OpenAI-compatible API.
    pub base_url: String,
    /// ChatGPT account ID header value (required for ChatGPT OAuth).
    pub account_id: Option<String>,
}

// ---------------------------------------------------------------------------
// RepromptOverlayState / RepromptOverlayAction — overlay state machine
// ---------------------------------------------------------------------------

/// The current state of the RepromptOverlay modal.
///
/// State transitions:
/// ```text
/// Reviewing ──[Enter]──> Accepted(refined)
///     | ──[e]──> Editing(text)
///     | ──[s]──> Skipped
///     | ──[Esc]─> Cancelled
///     | ──[timer]> Accepted(refined)
///
/// Editing ──[Enter]──> Accepted(edited)
///     | ──[Esc]──> Reviewing
/// ```
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum RepromptOverlayState {
    /// User is reviewing the refinement side-by-side with the original.
    Reviewing,
    /// User pressed `[e]` and is editing the refined text inline.
    Editing(String),
    /// User accepted the (possibly edited) refined prompt.
    Accepted(String),
    /// User pressed `[s]` — send the original input unmodified.
    Skipped,
    /// User pressed `[Esc]` — discard everything, return to composer.
    Cancelled,
}

/// Actions emitted by the overlay's key-event handler, consumed by
/// `ChatWidget` to drive the post-submission flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RepromptOverlayAction {
    /// Accept the refined (or edited) prompt and send it as the turn input.
    Accept(String),
    /// Skip refinement this time — send the original input.
    Skip,
    /// Show the model's reasoning in a secondary pane / tooltip.
    ShowReasoning,
    /// Cancel the submission entirely — return to the composer.
    Cancel,
    /// No actionable event (e.g. unrecognized key).
    None,
}

// ---------------------------------------------------------------------------
// RepromptOverlayData — everything needed to render the overlay
// ---------------------------------------------------------------------------

/// Bundles the original input, the refinement result, and overlay state so
/// `RepromptOverlay` has everything it needs.
#[derive(Debug, Clone)]
pub(crate) struct RepromptOverlayData {
    /// The raw text the user typed before refinement.
    pub original: String,
    /// The structured refinement result from the API.
    pub result: RepromptResult,
    /// Current overlay interaction state.
    pub state: RepromptOverlayState,
    /// When the overlay was first shown (for auto-accept countdown).
    pub shown_at: Instant,
    /// Auto-accept delay copied from `RepromptConfig` at overlay creation time.
    pub auto_accept_delay: Duration,
}

impl RepromptOverlayData {
    /// Create a new overlay data bundle in the `Reviewing` state.
    pub fn new(original: String, result: RepromptResult, auto_accept_delay: Duration) -> Self {
        Self {
            original,
            result,
            state: RepromptOverlayState::Reviewing,
            shown_at: Instant::now(),
            auto_accept_delay,
        }
    }

    /// Returns the number of seconds remaining before auto-accept, or `None`
    /// if auto-accept is disabled (delay == 0) or already elapsed.
    pub fn auto_accept_remaining(&self) -> Option<u64> {
        if self.auto_accept_delay.is_zero() {
            return None;
        }
        let elapsed = self.shown_at.elapsed();
        if elapsed >= self.auto_accept_delay {
            return Some(0);
        }
        Some((self.auto_accept_delay - elapsed).as_secs())
    }

    /// Check whether the auto-accept timer has fired. Returns `Some(action)`
    /// if so, `None` otherwise. Only fires in the `Reviewing` state.
    pub fn tick(&self) -> Option<RepromptOverlayAction> {
        if !matches!(self.state, RepromptOverlayState::Reviewing) {
            return None;
        }
        if self.auto_accept_delay.is_zero() {
            return None;
        }
        if self.shown_at.elapsed() >= self.auto_accept_delay {
            Some(RepromptOverlayAction::Accept(
                self.result.refined_prompt.clone(),
            ))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_reprompt_result() -> RepromptResult {
        RepromptResult {
            refined_prompt: "Refined: apply the JWT fix to payments".to_string(),
            applied_rules: vec!["regression test required".to_string()],
            reasoning: "Expanded vague reference to specific fix pattern".to_string(),
            task_type: TaskType::Bugfix,
            was_substantive_change: true,
        }
    }

    #[test]
    fn task_type_display_roundtrip() {
        for (variant, expected) in [
            (TaskType::Bugfix, "bugfix"),
            (TaskType::Feature, "feature"),
            (TaskType::Refactor, "refactor"),
            (TaskType::Analysis, "analysis"),
            (TaskType::Review, "review"),
        ] {
            assert_eq!(variant.to_string(), expected);
        }
    }

    #[test]
    fn reprompt_result_deserializes_from_camel_case_json() {
        let json = r#"{
            "refinedPrompt": "Apply the JWT fix to src/payments/token.ts",
            "appliedRules": ["regression test required"],
            "reasoning": "Expanded vague reference",
            "taskType": "bugfix",
            "wasSubstantiveChange": true
        }"#;

        let result: RepromptResult = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(result.task_type, TaskType::Bugfix);
        assert!(result.was_substantive_change);
        assert_eq!(result.applied_rules.len(), 1);
    }

    #[test]
    fn reprompt_config_defaults_are_sensible() {
        let config = RepromptConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.model, "o4-mini");
        assert_eq!(config.min_length, 20);
        assert!(!config.show_diff);
        assert_eq!(config.auto_accept_delay, Duration::from_secs(15));
        assert!(config.profile_name.is_none());
    }

    #[test]
    fn overlay_data_auto_accept_disabled_when_zero_delay() {
        let data = RepromptOverlayData::new(
            "original".to_string(),
            sample_reprompt_result(),
            Duration::ZERO,
        );
        assert!(data.auto_accept_remaining().is_none());
        assert!(data.tick().is_none());
    }

    #[test]
    fn overlay_data_tick_returns_none_when_not_reviewing() {
        let mut data = RepromptOverlayData::new(
            "original".to_string(),
            sample_reprompt_result(),
            Duration::from_secs(0),
        );
        data.state = RepromptOverlayState::Editing("editing".to_string());
        assert!(data.tick().is_none());
    }

    #[test]
    fn overlay_data_tick_fires_after_delay() {
        let data = RepromptOverlayData::new(
            "original".to_string(),
            sample_reprompt_result(),
            Duration::from_millis(1),
        );
        std::thread::sleep(Duration::from_millis(5));
        let action = data.tick();
        assert!(matches!(action, Some(RepromptOverlayAction::Accept(_))));
    }

    #[test]
    fn reprompt_result_serializes_to_camel_case() {
        let result = sample_reprompt_result();
        let json = serde_json::to_value(&result).expect("should serialize");
        assert!(json.get("refinedPrompt").is_some());
        assert!(json.get("wasSubstantiveChange").is_some());
        assert!(json.get("taskType").is_some());
        assert_eq!(json["taskType"], "bugfix");
    }
}
