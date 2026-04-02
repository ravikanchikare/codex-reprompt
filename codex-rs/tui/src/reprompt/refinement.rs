//! Async refinement API call for `/reprompt`.
//!
//! Calls an OpenAI-compatible API with a refinement system prompt, requesting
//! structured JSON output matching the `RepromptResult` schema.
//!
//! On any error, returns the original prompt wrapped in a `RepromptResult` with
//! `was_substantive_change = false` so the flow degrades gracefully.

use serde_json::json;

use super::RepromptConfig;
use super::RepromptProfile;
use super::RepromptResult;
use super::TaskType;

/// The built-in refinement system prompt used when no profile overrides it.
const REFINEMENT_SYSTEM_PROMPT: &str = r#"You are a prompt refinement engine for Codex, an AI coding agent.

Your job: take the user's raw input for a conversation turn and transform it
into a clear, actionable instruction that will produce the best outcome.

## Step 1: Detect task type

Before refining, classify the user's intent:
- bugfix: fixing broken behavior, errors, crashes, regressions
- feature: adding new functionality or capabilities
- refactor: restructuring code without changing behavior
- security: auth, input validation, secrets, vulnerabilities
- analysis: understanding code, exploring architecture
- review: code review, PR review
Set taskType accordingly.

## Step 2: Apply rules

Rules below are grouped by task type. Apply ONLY:
1. Global rules (always apply)
2. Rules matching the detected task type

Do NOT apply rules from other task types.

{reprompt_rules}

## Step 3: Refine using these principles
1. Make implicit requirements explicit
2. Resolve ambiguous references when possible
3. Expand shorthand ("fix it too" -> "apply the same fix pattern to X")
4. Add verification steps where appropriate (tests, checks)
5. Specify investigation order when applicable
6. Include relevant context hints (files, modules)
7. Preserve the user's intent EXACTLY -- do not add scope
8. Keep it concise -- agents work best with clear, focused prompts
9. If the input is already clear and specific, return it mostly unchanged

## Critical rule:
If the user's input is already precise and well-structured, set
wasSubstantiveChange to false and return the original with minimal changes.
Do NOT over-refine clear instructions. The goal is to help with vague inputs,
not to rewrite everything.

## Output:
Return a RepromptResult JSON with: refinedPrompt, appliedRules, reasoning,
taskType, wasSubstantiveChange."#;

/// JSON schema for structured output, matching the `RepromptResult` type.
fn reprompt_result_json_schema() -> serde_json::Value {
    json!({
        "name": "RepromptResult",
        "strict": true,
        "schema": {
            "type": "object",
            "properties": {
                "refinedPrompt": { "type": "string" },
                "appliedRules": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "reasoning": { "type": "string" },
                "taskType": {
                    "type": "string",
                    "enum": ["bugfix", "feature", "refactor", "analysis", "review"]
                },
                "wasSubstantiveChange": { "type": "boolean" }
            },
            "required": [
                "refinedPrompt",
                "appliedRules",
                "reasoning",
                "taskType",
                "wasSubstantiveChange"
            ],
            "additionalProperties": false
        }
    })
}

/// Build the system prompt by substituting placeholders.
fn build_system_prompt(base_prompt: &str, reprompt_rules: &str) -> String {
    base_prompt.replace("{reprompt_rules}", reprompt_rules)
}

/// Build a fallback `RepromptResult` that passes the original through unchanged.
fn fallback_result(original: &str) -> RepromptResult {
    RepromptResult {
        refined_prompt: original.to_string(),
        applied_rules: vec![],
        reasoning: "Refinement API call failed; returning original input.".to_string(),
        task_type: TaskType::Analysis,
        was_substantive_change: false,
    }
}

/// Refine a user's input via an API call.
///
/// This is a stateless, single-shot call — it is never part of the Codex
/// thread. On failure, returns the original text wrapped in a `RepromptResult`
/// with `was_substantive_change = false`.
pub(crate) async fn refine_input(
    original: &str,
    config: &RepromptConfig,
    auth: &super::RepromptAuthInfo,
    profile: &RepromptProfile,
) -> anyhow::Result<RepromptResult> {
    let api_key = &auth.token;
    let base_url = &auth.base_url;

    let base_prompt = profile
        .system_prompt
        .as_deref()
        .unwrap_or(REFINEMENT_SYSTEM_PROMPT);

    let rules_text = profile.format_rules_for_prompt();

    let model = if profile.model != "o4-mini" {
        &profile.model
    } else {
        &config.model
    };

    let system_prompt = build_system_prompt(base_prompt, &rules_text);

    tracing::info!(
        "REPROMPT: profile={}, model={}, rules={}, custom_system_prompt={}, user_input={}",
        profile.name,
        model,
        rules_text,
        profile.system_prompt.is_some(),
        original,
    );
    tracing::info!("REPROMPT SYSTEM PROMPT:\n{system_prompt}");

    let schema = reprompt_result_json_schema();

    let request_body = json!({
        "model": model,
        "input": [
            {
                "role": "user",
                "content": original
            }
        ],
        "instructions": system_prompt,
        "text": {
            "format": {
                "type": "json_schema",
                "name": "RepromptResult",
                "strict": true,
                "schema": schema["schema"]
            }
        },
        "store": false,
        "stream": true
    });

    let client = reqwest::Client::new();
    let mut req = client
        .post(format!("{base_url}/responses"))
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json");
    if let Some(account_id) = &auth.account_id {
        req = req.header("ChatGPT-Account-ID", account_id);
    }
    let response = req
        .json(&request_body)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Reprompt refinement API request failed: {e}");
            return Ok(fallback_result(original));
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        tracing::warn!("Reprompt refinement API returned {status}: {body}");
        let api_msg = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v["error"]["message"]
                    .as_str()
                    .or_else(|| v["detail"].as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| format!("{status}"));
        let mut result = fallback_result(original);
        result.reasoning = format!("API error: {api_msg}");
        return Ok(result);
    }

    // Consume the SSE stream and collect output text deltas.
    let full_body = match response.text().await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("Failed to read reprompt refinement stream: {e}");
            return Ok(fallback_result(original));
        }
    };

    let mut output_text = String::new();
    for line in full_body.lines() {
        let line = line.trim();
        if !line.starts_with("data: ") {
            continue;
        }
        let data = &line["data: ".len()..];
        if data == "[DONE]" {
            break;
        }
        if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
            let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match event_type {
                "response.output_text.delta" => {
                    if let Some(delta) = event.get("delta").and_then(|d| d.as_str()) {
                        output_text.push_str(delta);
                    }
                }
                "response.completed" => {
                    if output_text.is_empty()
                        && let Some(resp) = event.get("response")
                        && let Some(text) = resp.get("output_text").and_then(|t| t.as_str())
                    {
                        output_text = text.to_string();
                    }
                }
                _ => {}
            }
        }
    }

    if output_text.is_empty() {
        tracing::warn!("No output text collected from reprompt refinement stream");
        return Ok(fallback_result(original));
    }

    match serde_json::from_str::<RepromptResult>(&output_text) {
        Ok(result) => {
            tracing::info!(
                "Reprompt refinement succeeded: substantive={}, task={}, reasoning={}, refined={}",
                result.was_substantive_change,
                result.task_type,
                result.reasoning,
                result.refined_prompt,
            );
            Ok(result)
        }
        Err(e) => {
            tracing::warn!("Failed to parse RepromptResult JSON: {e}\nRaw: {output_text}");
            Ok(fallback_result(original))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_result_preserves_original() {
        let result = fallback_result("my original prompt");
        assert_eq!(result.refined_prompt, "my original prompt");
        assert!(!result.was_substantive_change);
        assert!(result.applied_rules.is_empty());
    }

    #[test]
    fn build_system_prompt_substitutes_placeholders() {
        let prompt = build_system_prompt(REFINEMENT_SYSTEM_PROMPT, "always include tests");
        assert!(prompt.contains("always include tests"));
    }

    #[test]
    fn build_system_prompt_with_empty_rules() {
        let prompt = build_system_prompt(REFINEMENT_SYSTEM_PROMPT, "");
        assert!(prompt.contains("Step 2: Apply rules"));
        assert!(!prompt.contains("{reprompt_rules}"));
    }

    #[test]
    fn json_schema_is_valid() {
        let schema = reprompt_result_json_schema();
        assert!(schema.get("name").is_some());
        assert!(schema.get("schema").is_some());
        let inner = schema.get("schema").unwrap();
        let required = inner.get("required").unwrap().as_array().unwrap();
        assert_eq!(required.len(), 5);
    }

    #[tokio::test]
    async fn refine_input_with_invalid_auth_returns_fallback() {
        let config = RepromptConfig::default();
        let auth = super::super::RepromptAuthInfo {
            token: "invalid-token".to_string(),
            base_url: "http://localhost:1".to_string(),
            account_id: None,
        };
        let profile = RepromptProfile::default();
        let result = refine_input("test prompt", &config, &auth, &profile)
            .await
            .unwrap();
        assert_eq!(result.refined_prompt, "test prompt");
        assert!(!result.was_substantive_change);
    }
}
