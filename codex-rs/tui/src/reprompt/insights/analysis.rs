//! Insights analysis via LLM API call.
//!
//! Sends accumulated refinement entries to an OpenAI-compatible API with a
//! coaching-oriented system prompt. Returns an [`InsightsResult`] with skill
//! assessment, gap patterns, suggestions, and Reprompt quality evaluation.
//!
//! Follows the same API call pattern as `refinement.rs`: structured JSON
//! output via the `/responses` endpoint with SSE streaming.

use codex_login::default_client::build_reqwest_client;
use serde_json::json;

use super::InsightsResult;
use super::RefinementEntry;
use super::SkillAssessment;
use super::SkillLevel;
use crate::reprompt::RepromptAuthInfo;

/// System prompt for insights analysis.
const INSIGHTS_SYSTEM_PROMPT: &str = r#"You are a prompt coaching engine for Codex, an AI coding agent.

You receive a history of prompt refinements: each entry contains the user's
original prompt and the Reprompt-refined version, along with the task type and
which rules were applied.

Your job: analyze these refinement pairs and produce a coaching report that
helps the user improve their prompting skills, and evaluates how well Reprompt
itself performed.

## Analysis Steps

### 1. Assess Prompting Skill Level
Based on the overall quality of original prompts:
- beginner: Prompts are vague, missing context, ambiguous references
- intermediate: Clear goals but often omits file paths, verification steps, or specifics
- advanced: Specific and well-structured, occasional minor gaps
- expert: Consistently precise, includes verification, context, and file paths

### 2. Detect Gap Patterns
Compare each original→refined pair and categorize what was added or fixed:
- missing_path: File or module paths that were omitted
- ambiguous_reference: Pronouns or vague references ("it", "that", "the bug")
- no_verification: Missing test/check requirements
- scope_unclear: Ambiguous scope or multiple unrelated tasks
- missing_context: Missing error messages, reproduction steps, or module names
- missing_error_info: No specific error details when reporting bugs

For each gap category, count how many entries exhibited it and provide a
concrete example (original excerpt + how it was fixed).

### 3. Identify Patterns
List 2-4 high-level observations about how the user writes prompts.
Be specific and constructive, not generic.

### 4. Generate Suggestions
Provide 2-4 actionable, specific suggestions for improving future prompts.
Each suggestion should include a title, explanation, and ideally an example
drawn from the actual refinement history.

### 5. Evaluate Reprompt Quality
For each refinement pair, assess:
- Did Reprompt preserve the user's original intent?
- Did Reprompt add unnecessary scope (scope creep)?
Summarize with counts and a brief qualitative assessment.

## Output
Return an InsightsResult JSON with: skillAssessment, gaps, patterns,
suggestions, repromptQuality."#;

/// JSON schema for structured output matching [`InsightsResult`].
fn insights_result_json_schema() -> serde_json::Value {
    json!({
        "name": "InsightsResult",
        "strict": true,
        "schema": {
            "type": "object",
            "properties": {
                "skillAssessment": {
                    "type": "object",
                    "properties": {
                        "level": {
                            "type": "string",
                            "enum": ["beginner", "intermediate", "advanced", "expert"]
                        },
                        "explanation": { "type": "string" },
                        "topImprovement": { "type": "string" }
                    },
                    "required": ["level", "explanation", "topImprovement"],
                    "additionalProperties": false
                },
                "gaps": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "category": { "type": "string" },
                            "count": { "type": "integer" },
                            "total": { "type": "integer" },
                            "description": { "type": "string" },
                            "exampleOriginal": { "type": "string" },
                            "exampleFix": { "type": "string" }
                        },
                        "required": [
                            "category", "count", "total",
                            "description", "exampleOriginal", "exampleFix"
                        ],
                        "additionalProperties": false
                    }
                },
                "patterns": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "suggestions": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string" },
                            "detail": { "type": "string" },
                            "example": { "type": ["string", "null"] }
                        },
                        "required": ["title", "detail", "example"],
                        "additionalProperties": false
                    }
                },
                "repromptQuality": {
                    "type": ["object", "null"],
                    "properties": {
                        "intentPreservedCount": { "type": "integer" },
                        "scopeCreepCount": { "type": "integer" },
                        "total": { "type": "integer" },
                        "assessment": { "type": "string" }
                    },
                    "required": [
                        "intentPreservedCount", "scopeCreepCount",
                        "total", "assessment"
                    ],
                    "additionalProperties": false
                }
            },
            "required": [
                "skillAssessment", "gaps", "patterns",
                "suggestions", "repromptQuality"
            ],
            "additionalProperties": false
        }
    })
}

/// Format refinement entries as context for the insights model.
fn format_entries_for_prompt(entries: &[RefinementEntry]) -> String {
    let mut context = String::new();
    for (i, entry) in entries.iter().enumerate() {
        context.push_str(&format!(
            "### Refinement {}\n\
             Task type: {}\n\
             Original: {}\n\
             Refined: {}\n\
             Rules applied: {}\n\n",
            i + 1,
            entry.result.task_type,
            entry.original_prompt,
            entry.result.refined_prompt,
            entry.result.applied_rules.join(", "),
        ));
    }
    context
}

/// Build a fallback [`InsightsResult`] for when the API call fails.
fn fallback_result() -> InsightsResult {
    InsightsResult {
        skill_assessment: SkillAssessment {
            level: SkillLevel::Intermediate,
            explanation: "Unable to analyze — API call failed.".to_string(),
            top_improvement: String::new(),
        },
        gaps: vec![],
        patterns: vec![],
        suggestions: vec![],
        reprompt_quality: None,
    }
}

/// Generate insights from accumulated refinement entries via an API call.
///
/// On any error, returns a minimal fallback result so the UI always has
/// something to display.
pub(crate) async fn generate_insights(
    entries: &[RefinementEntry],
    auth: &RepromptAuthInfo,
    model: &str,
) -> InsightsResult {
    if entries.is_empty() {
        return fallback_result();
    }

    let schema = insights_result_json_schema();
    let entries_context = format_entries_for_prompt(entries);

    let request_body = json!({
        "model": model,
        "input": [
            {
                "role": "user",
                "content": format!(
                    "Analyze these {count} prompt refinements and generate a coaching report.\n\n{entries_context}",
                    count = entries.len(),
                )
            }
        ],
        "instructions": INSIGHTS_SYSTEM_PROMPT,
        "text": {
            "format": {
                "type": "json_schema",
                "name": "InsightsResult",
                "strict": true,
                "schema": schema["schema"]
            }
        },
        "store": false,
        "stream": true
    });

    let client = build_reqwest_client();
    let mut req = client
        .post(format!("{}/responses", auth.base_url))
        .header("Authorization", format!("Bearer {}", auth.token))
        .header("Content-Type", "application/json");
    if let Some(account_id) = &auth.account_id {
        req = req.header("ChatGPT-Account-ID", account_id);
    }

    let response = match req
        .json(&request_body)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Insights API request failed: {e}");
            return fallback_result();
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        tracing::warn!("Insights API returned {status}: {body}");
        return fallback_result();
    }

    let full_body = match response.text().await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("Failed to read insights stream: {e}");
            return fallback_result();
        }
    };

    let output_text = crate::reprompt::api_utils::extract_output_text_from_sse(&full_body);

    if output_text.is_empty() {
        tracing::warn!("No output text collected from insights stream");
        return fallback_result();
    }

    match serde_json::from_str::<InsightsResult>(&output_text) {
        Ok(result) => {
            tracing::debug!(
                "Insights analysis succeeded: skill={}, gaps={}, suggestions={}",
                result.skill_assessment.level,
                result.gaps.len(),
                result.suggestions.len(),
            );
            result
        }
        Err(e) => {
            tracing::warn!("Failed to parse InsightsResult JSON: {e}\nRaw: {output_text}");
            fallback_result()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reprompt::config::RepromptResult;
    use crate::reprompt::config::TaskType;

    fn sample_entry() -> RefinementEntry {
        RefinementEntry {
            timestamp: 1712188800,
            original_prompt: "fix the auth bug".to_string(),
            result: RepromptResult {
                refined_prompt: "Fix the JWT validation bug in @src/auth.rs".to_string(),
                applied_rules: vec!["specify file paths".to_string()],
                reasoning: "Added specific file path.".to_string(),
                task_type: TaskType::Bugfix,
                was_substantive_change: true,
                tips: vec![],
            },
            project_path: None,
        }
    }

    #[test]
    fn format_entries_includes_all_fields() {
        let entries = vec![sample_entry()];
        let text = format_entries_for_prompt(&entries);
        assert!(text.contains("Refinement 1"));
        assert!(text.contains("bugfix"));
        assert!(text.contains("fix the auth bug"));
        assert!(text.contains("Fix the JWT validation bug"));
        assert!(text.contains("specify file paths"));
    }

    #[test]
    fn fallback_result_has_sensible_defaults() {
        let result = fallback_result();
        assert_eq!(result.skill_assessment.level, SkillLevel::Intermediate);
        assert!(result.gaps.is_empty());
        assert!(result.suggestions.is_empty());
        assert!(result.reprompt_quality.is_none());
    }

    #[test]
    fn json_schema_is_valid() {
        let schema = insights_result_json_schema();
        assert!(schema.get("name").is_some());
        assert!(schema.get("schema").is_some());
        let inner = schema.get("schema").unwrap();
        let required = inner.get("required").unwrap().as_array().unwrap();
        assert_eq!(required.len(), 5);
    }

    #[tokio::test]
    async fn generate_insights_with_invalid_auth_returns_fallback() {
        let auth = RepromptAuthInfo {
            token: "invalid-token".to_string(),
            base_url: "http://localhost:1".to_string(),
            account_id: None,
        };
        let entries = vec![sample_entry()];
        let result = generate_insights(&entries, &auth, "o4-mini").await;
        // Should gracefully fall back, not panic.
        assert_eq!(result.skill_assessment.level, SkillLevel::Intermediate);
    }

    #[tokio::test]
    async fn generate_insights_with_empty_entries_returns_fallback() {
        let auth = RepromptAuthInfo {
            token: "test".to_string(),
            base_url: "http://localhost:1".to_string(),
            account_id: None,
        };
        let result = generate_insights(&[], &auth, "o4-mini").await;
        assert!(result.gaps.is_empty());
    }
}
