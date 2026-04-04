//! Reprompt Insights — `/reprompt-insights` analysis and coaching.
//!
//! This module provides post-hoc analysis of accumulated prompt refinements.
//! It persists each refinement pair (original + result) to disk, and on
//! invocation via `/reprompt-insights`, sends accumulated data to an LLM
//! for synthesis into a coaching report.
//!
//! # Module structure
//!
//! - `analysis` — async API call to generate insights from refinement history
//! - `overlay` — scrollable TUI overlay for displaying insights
//! - `storage` — disk persistence for refinement entries

pub(crate) mod analysis;
pub(crate) mod overlay;
pub(crate) mod storage;

use serde::Deserialize;
use serde::Serialize;

use super::RepromptResult;

// ---------------------------------------------------------------------------
// RefinementEntry — a persisted original + refinement pair
// ---------------------------------------------------------------------------

/// A single refinement event persisted to disk for later analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RefinementEntry {
    /// Unix timestamp (seconds) when the refinement occurred.
    pub timestamp: i64,
    /// The raw text the user typed before refinement.
    pub original_prompt: String,
    /// The structured refinement result from the API.
    pub result: RepromptResult,
    /// The project directory path at the time of refinement.
    pub project_path: Option<String>,
}

// ---------------------------------------------------------------------------
// InsightsResult — structured output from the insights analysis API
// ---------------------------------------------------------------------------

/// The structured JSON output returned by the insights analysis model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InsightsResult {
    /// Overall prompting skill assessment.
    pub skill_assessment: SkillAssessment,
    /// Recurring gap categories detected across refinements.
    pub gaps: Vec<InsightGap>,
    /// High-level patterns observed in the user's prompting style.
    pub patterns: Vec<String>,
    /// Actionable suggestions for improving future prompts.
    pub suggestions: Vec<InsightSuggestion>,
    /// Evaluation of Reprompt's own refinement quality.
    pub reprompt_quality: Option<RepromptQuality>,
}

/// Skill level classification for the user's prompting ability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SkillLevel {
    Beginner,
    Intermediate,
    Advanced,
    Expert,
}

impl std::fmt::Display for SkillLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Beginner => write!(f, "Beginner"),
            Self::Intermediate => write!(f, "Intermediate"),
            Self::Advanced => write!(f, "Advanced"),
            Self::Expert => write!(f, "Expert"),
        }
    }
}

/// Assessment of the user's prompting skill based on refinement history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillAssessment {
    /// Assessed skill level.
    pub level: SkillLevel,
    /// One-sentence explanation of the assessment.
    pub explanation: String,
    /// The single most impactful thing the user could improve.
    pub top_improvement: String,
}

/// A recurring gap category detected across multiple refinements.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InsightGap {
    /// Category identifier (e.g. "missing_path", "ambiguous_reference").
    pub category: String,
    /// How many refinements exhibited this gap.
    pub count: u32,
    /// Total refinements analyzed.
    pub total: u32,
    /// Human-readable description of the gap pattern.
    pub description: String,
    /// Example original prompt excerpt that exhibited this gap.
    pub example_original: String,
    /// How the refinement addressed the gap.
    pub example_fix: String,
}

/// An actionable suggestion for improving future prompts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InsightSuggestion {
    /// Short title (under 60 chars).
    pub title: String,
    /// One-sentence explanation.
    pub detail: String,
    /// An example of the improved pattern applied to a real prompt.
    pub example: Option<String>,
}

/// Evaluation of Reprompt's own refinement quality.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepromptQuality {
    /// Number of refinements that preserved user intent.
    pub intent_preserved_count: u32,
    /// Number of refinements that added unnecessary scope.
    pub scope_creep_count: u32,
    /// Total refinements evaluated.
    pub total: u32,
    /// Brief qualitative assessment.
    pub assessment: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reprompt::config::TaskType;

    fn sample_insights_result() -> InsightsResult {
        InsightsResult {
            skill_assessment: SkillAssessment {
                level: SkillLevel::Intermediate,
                explanation: "Clear goals but often omits file paths.".to_string(),
                top_improvement: "Include @file paths when you know them.".to_string(),
            },
            gaps: vec![InsightGap {
                category: "missing_path".to_string(),
                count: 6,
                total: 8,
                description: "File paths omitted when they could be specified.".to_string(),
                example_original: "fix the auth bug in payments".to_string(),
                example_fix: "fix the JWT bug in @src/payments/auth.rs".to_string(),
            }],
            patterns: vec!["Frequently uses pronouns instead of specific names.".to_string()],
            suggestions: vec![InsightSuggestion {
                title: "Always include file paths".to_string(),
                detail: "Specify @file paths to help the agent find code faster.".to_string(),
                example: Some("fix JWT in @src/payments/auth.rs".to_string()),
            }],
            reprompt_quality: Some(RepromptQuality {
                intent_preserved_count: 7,
                scope_creep_count: 1,
                total: 8,
                assessment: "Good refinement quality with minimal scope creep.".to_string(),
            }),
        }
    }

    fn sample_refinement_entry() -> RefinementEntry {
        RefinementEntry {
            timestamp: 1712188800,
            original_prompt: "fix the auth bug".to_string(),
            result: RepromptResult {
                refined_prompt: "Fix the JWT validation bug in @src/auth.rs".to_string(),
                applied_rules: vec!["specify file paths".to_string()],
                reasoning: "Added specific file path.".to_string(),
                task_type: TaskType::Bugfix,
                was_substantive_change: true,
                tips: vec!["Include file paths.".to_string()],
            },
            project_path: Some("/home/user/project".to_string()),
        }
    }

    #[test]
    fn refinement_entry_serde_roundtrip() {
        let entry = sample_refinement_entry();
        let json = serde_json::to_string(&entry).expect("serialize");
        let deserialized: RefinementEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.timestamp, entry.timestamp);
        assert_eq!(deserialized.original_prompt, entry.original_prompt);
        assert_eq!(
            deserialized.result.refined_prompt,
            entry.result.refined_prompt
        );
    }

    #[test]
    fn insights_result_serde_roundtrip() {
        let result = sample_insights_result();
        let json = serde_json::to_string(&result).expect("serialize");
        let deserialized: InsightsResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            deserialized.skill_assessment.level,
            SkillLevel::Intermediate
        );
        assert_eq!(deserialized.gaps.len(), 1);
        assert_eq!(deserialized.suggestions.len(), 1);
        assert!(deserialized.reprompt_quality.is_some());
    }

    #[test]
    fn skill_level_display() {
        assert_eq!(SkillLevel::Beginner.to_string(), "Beginner");
        assert_eq!(SkillLevel::Intermediate.to_string(), "Intermediate");
        assert_eq!(SkillLevel::Advanced.to_string(), "Advanced");
        assert_eq!(SkillLevel::Expert.to_string(), "Expert");
    }

    #[test]
    fn insights_result_deserializes_from_camel_case() {
        let json = r#"{
            "skillAssessment": {
                "level": "advanced",
                "explanation": "Strong prompts overall.",
                "topImprovement": "Add verification steps."
            },
            "gaps": [],
            "patterns": ["Clear and specific prompts."],
            "suggestions": [],
            "repromptQuality": null
        }"#;
        let result: InsightsResult = serde_json::from_str(json).expect("deserialize");
        assert_eq!(result.skill_assessment.level, SkillLevel::Advanced);
        assert!(result.reprompt_quality.is_none());
    }
}
