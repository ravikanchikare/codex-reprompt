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
    Security,
    Analysis,
    Review,
}

impl std::fmt::Display for TaskType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bugfix => write!(f, "bugfix"),
            Self::Feature => write!(f, "feature"),
            Self::Refactor => write!(f, "refactor"),
            Self::Security => write!(f, "security"),
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

    /// Short suggestions for improving the original prompt on future turns.
    #[serde(default)]
    pub tips: Vec<String>,
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

    /// Maximum number of prior conversation turns to include as context
    /// for the refinement model. `0` disables context.
    pub context_turns: usize,

    /// Whether to inject a matched list of relevant project files into the
    /// refinement instructions.
    pub include_relevant_files: bool,

    /// Maximum number of matched file candidates to include in the prompt.
    pub relevant_files_max_count: usize,

    /// Maximum number of characters to spend on relevant file candidates.
    pub relevant_files_max_chars: usize,

    /// Whether to inject a matched list of relevant skills into the
    /// refinement instructions.
    pub include_relevant_skills: bool,

    /// Maximum number of matched skill candidates to include in the prompt.
    pub relevant_skills_max_count: usize,

    /// Maximum number of characters to spend on relevant skill candidates.
    pub relevant_skills_max_chars: usize,

    /// Whether to inject a matched list of relevant plugins into the
    /// refinement instructions.
    pub include_relevant_plugins: bool,

    /// Maximum number of matched plugin candidates to include in the prompt.
    pub relevant_plugins_max_count: usize,

    /// Maximum number of characters to spend on relevant plugin candidates.
    pub relevant_plugins_max_chars: usize,

    /// Whether to inject a matched list of relevant apps into the refinement
    /// instructions.
    pub include_relevant_apps: bool,

    /// Maximum number of matched app candidates to include in the prompt.
    pub relevant_apps_max_count: usize,

    /// Maximum number of characters to spend on relevant app candidates.
    pub relevant_apps_max_chars: usize,

    /// Whether to append a filtered project structure summary to the
    /// refinement instructions.
    pub include_project_structure: bool,

    /// Maximum directory depth to include in the project structure summary.
    pub project_structure_max_depth: usize,

    /// Maximum number of characters to include in the project structure
    /// summary. Additional content is truncated.
    pub project_structure_max_chars: usize,

    /// How long project structure summaries stay cached, in seconds.
    pub project_structure_cache_ttl_secs: u64,

    /// Additional glob/path fragments to exclude from the project structure
    /// summary.
    pub project_structure_extra_excludes: Vec<String>,

    /// Whether to redact secrets before sending the prompt to the reprompt
    /// refinement model.
    pub redact_secrets: bool,

    /// Whether to apply the high-entropy fallback detector after known secret
    /// patterns.
    pub redact_high_entropy: bool,

    /// Minimum Shannon entropy to consider a token secret-like.
    pub redaction_entropy_threshold: f64,

    /// Minimum token length for entropy-based secret detection.
    pub redaction_min_length: usize,

    /// Whether refined `@...` and `$...` mentions should be reparsed into
    /// structured file/tool inputs before submission.
    pub reparse_refined_mentions: bool,
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
            context_turns: 4,
            include_relevant_files: true,
            relevant_files_max_count: 8,
            relevant_files_max_chars: 600,
            include_relevant_skills: true,
            relevant_skills_max_count: 5,
            relevant_skills_max_chars: 600,
            include_relevant_plugins: true,
            relevant_plugins_max_count: 4,
            relevant_plugins_max_chars: 400,
            include_relevant_apps: true,
            relevant_apps_max_count: 4,
            relevant_apps_max_chars: 400,
            include_project_structure: true,
            project_structure_max_depth: 4,
            project_structure_max_chars: 2_000,
            project_structure_cache_ttl_secs: 30,
            project_structure_extra_excludes: Vec::new(),
            redact_secrets: true,
            redact_high_entropy: true,
            redaction_entropy_threshold: 4.5,
            redaction_min_length: 24,
            reparse_refined_mentions: true,
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
    /// Insert the refined prompt back into the composer for further editing.
    Iterate(String),
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
    pub fn new(original: String, mut result: RepromptResult, auto_accept_delay: Duration) -> Self {
        result.tips.truncate(3);
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
            tips: vec![
                "Add a file path to skip discovery".to_string(),
                "Name the exact bug instead of 'it'".to_string(),
                "Add a verification step like tests".to_string(),
            ],
        }
    }

    #[test]
    fn task_type_display_roundtrip() {
        for (variant, expected) in [
            (TaskType::Bugfix, "bugfix"),
            (TaskType::Feature, "feature"),
            (TaskType::Refactor, "refactor"),
            (TaskType::Security, "security"),
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
        assert!(result.tips.is_empty());
    }

    #[test]
    fn reprompt_config_defaults_are_sensible() {
        let config = RepromptConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.model, "o4-mini");
        assert_eq!(config.min_length, 20);
        assert!(!config.show_diff);
        assert_eq!(config.auto_accept_delay, Duration::from_secs(15));
        assert_eq!(config.context_turns, 4);
        assert!(config.include_relevant_files);
        assert_eq!(config.relevant_files_max_count, 8);
        assert_eq!(config.relevant_files_max_chars, 600);
        assert!(config.include_relevant_skills);
        assert_eq!(config.relevant_skills_max_count, 5);
        assert_eq!(config.relevant_skills_max_chars, 600);
        assert!(config.include_relevant_plugins);
        assert_eq!(config.relevant_plugins_max_count, 4);
        assert_eq!(config.relevant_plugins_max_chars, 400);
        assert!(config.include_relevant_apps);
        assert_eq!(config.relevant_apps_max_count, 4);
        assert_eq!(config.relevant_apps_max_chars, 400);
        assert!(config.include_project_structure);
        assert_eq!(config.project_structure_max_depth, 4);
        assert_eq!(config.project_structure_max_chars, 2_000);
        assert_eq!(config.project_structure_cache_ttl_secs, 30);
        assert!(config.project_structure_extra_excludes.is_empty());
        assert!(config.redact_secrets);
        assert!(config.redact_high_entropy);
        assert_eq!(config.redaction_entropy_threshold, 4.5);
        assert_eq!(config.redaction_min_length, 24);
        assert!(config.reparse_refined_mentions);
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
        assert!(json.get("tips").is_some());
        assert_eq!(json["taskType"], "bugfix");
    }

    #[test]
    fn overlay_data_caps_tips_at_three() {
        let mut result = sample_reprompt_result();
        result.tips.push("Fourth tip should be dropped".to_string());

        let data = RepromptOverlayData::new("original".to_string(), result, Duration::ZERO);

        assert_eq!(
            data.result.tips,
            vec![
                "Add a file path to skip discovery".to_string(),
                "Name the exact bug instead of 'it'".to_string(),
                "Add a verification step like tests".to_string(),
            ]
        );
    }
}
