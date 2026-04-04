//! Async refinement API call for `/reprompt`.
//!
//! Calls an OpenAI-compatible API with a refinement system prompt, requesting
//! structured JSON output matching the `RepromptResult` schema.
//!
//! On any error, returns the original prompt wrapped in a `RepromptResult` with
//! `was_substantive_change = false` so the flow degrades gracefully.

use codex_login::default_client::build_reqwest_client;
use serde_json::json;

use super::RelevantAppPrompt;
use super::RelevantPluginPrompt;
use super::RelevantSkillPrompt;
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
7. Canonicalize file references when context identifies the exact path
8. Canonicalize tool references when context identifies the exact skill, plugin, or app
9. Preserve the user's intent EXACTLY -- do not add scope
10. Keep it concise -- agents work best with clear, focused prompts
11. If the input is already clear and specific, return it mostly unchanged

## Step 4: Generate tips
After refining, generate 0-3 short tips about the ORIGINAL prompt.
Each tip should:
- Identify one critical flaw or missed opportunity
- Be a single sentence under 80 characters
- Be readable within 5 seconds
- Be actionable and specific
- Focus on what the user can improve

Categories to check (priority order):
1. Missing file paths, function names, or modules
2. Ambiguous references ("it", "that", "the bug")
3. No verification criteria
4. Multiple unrelated requests
5. Missing error messages or reproduction steps

Only include tips for real issues. If the prompt is good, return an empty array.

## Conversation context
The input messages may include recent conversation turns before the current
user input. Use this context to:
- Resolve anaphoric references ("that", "it", "the same fix")
- Understand recently discussed files, functions, or patterns
- Detect task continuity vs. topic changes
The LAST user message is the one to refine. Prior messages are read-only context.

## File and tool resolution
The instructions may include relevant files, skills, plugins, and apps.
- When the user references a file by a partial path, basename, or module name,
  correct it to the exact relative path from the provided file hints and render
  it as `@path/to/file.rs`.
- When the user informally references a skill, plugin, or app, map it to the
  exact visible token from the provided hints and render it as `$token`.
- Use the recent conversation, candidate descriptions, and neighboring wording
  to infer whether the user means a file, skill, plugin, or app.
- Prefer auto-correcting near-misses (`token.rs`, `auth file`, `linter skill`,
  `calendar app`) when one candidate is clearly supported by the context.
- Use only candidates that are explicitly provided or strongly supported by the
  recent conversation context.
- Preserve the canonical token or path exactly once you resolve it. Do not
  paraphrase, shorten, or rename a resolved reference.
- Do not invent file paths, skill names, plugin names, or app tokens.
- If multiple candidates are plausible, keep the user's wording instead of
  forcing an uncertain resolution.

## Critical rule:
If the user's input is already precise and well-structured, set
wasSubstantiveChange to false and return the original with minimal changes.
Do NOT over-refine clear instructions. The goal is to help with vague inputs,
not to rewrite everything.

## Output:
Return a RepromptResult JSON with: refinedPrompt, appliedRules, reasoning,
taskType, wasSubstantiveChange, tips."#;

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
                    "enum": ["bugfix", "feature", "refactor", "security", "analysis", "review"]
                },
                "wasSubstantiveChange": { "type": "boolean" },
                "tips": {
                    "type": "array",
                    "items": {
                        "type": "string",
                        "maxLength": 80
                    },
                    "maxItems": 3
                }
            },
            "required": [
                "refinedPrompt",
                "appliedRules",
                "reasoning",
                "taskType",
                "wasSubstantiveChange",
                "tips"
            ],
            "additionalProperties": false
        }
    })
}

pub(crate) struct RefinementPromptContext<'a> {
    pub conversation: &'a [super::thread_context::ContextTurn],
    pub project_structure: Option<&'a str>,
    pub relevant_files: &'a [String],
    pub relevant_skills: &'a [RelevantSkillPrompt],
    pub relevant_plugins: &'a [RelevantPluginPrompt],
    pub relevant_apps: &'a [RelevantAppPrompt],
    pub redaction_mappings: &'a [codex_secrets::RedactionMapping],
}

/// Build the system prompt by substituting placeholders.
fn build_system_prompt(
    base_prompt: &str,
    reprompt_rules: &str,
    prompt_context: &RefinementPromptContext<'_>,
) -> String {
    let mut prompt = base_prompt.replace("{reprompt_rules}", reprompt_rules);
    append_prompt_section(
        &mut prompt,
        "## Relevant files",
        prompt_context.relevant_files.to_vec(),
    );
    append_prompt_section(
        &mut prompt,
        "## Relevant skills",
        prompt_context
            .relevant_skills
            .iter()
            .map(RelevantSkillPrompt::render_line)
            .collect(),
    );
    append_prompt_section(
        &mut prompt,
        "## Relevant plugins",
        prompt_context
            .relevant_plugins
            .iter()
            .map(RelevantPluginPrompt::render_line)
            .collect(),
    );
    append_prompt_section(
        &mut prompt,
        "## Relevant apps",
        prompt_context
            .relevant_apps
            .iter()
            .map(RelevantAppPrompt::render_line)
            .collect(),
    );

    if prompt_context.relevant_files.is_empty()
        && let Some(project_structure) = prompt_context
            .project_structure
            .filter(|project_structure| !project_structure.trim().is_empty())
    {
        if !prompt.ends_with('\n') {
            prompt.push('\n');
        }
        prompt.push_str(
            r#"

## Project structure
The project tree below is read-only context. Use it to map vague references to
real files or modules when possible. Do not invent paths that are not present.

<project_structure>
"#,
        );
        prompt.push_str(project_structure);
        prompt.push_str("\n</project_structure>");
    }
    prompt
}

fn append_prompt_section(prompt: &mut String, heading: &str, lines: Vec<String>) {
    if lines.is_empty() {
        return;
    }
    if !prompt.ends_with('\n') {
        prompt.push('\n');
    }
    prompt.push('\n');
    prompt.push_str(heading);
    prompt.push('\n');
    for line in lines {
        prompt.push_str(&line);
        prompt.push('\n');
    }
}

fn build_request_body(
    model: &str,
    prompt_for_model: &str,
    system_prompt: &str,
    context: &[super::thread_context::ContextTurn],
    schema: &serde_json::Value,
) -> serde_json::Value {
    let mut input_messages: Vec<serde_json::Value> = Vec::new();
    for turn in context {
        let role = match turn.role {
            super::thread_context::ContextRole::User => "user",
            super::thread_context::ContextRole::Assistant => "assistant",
        };
        input_messages.push(json!({ "role": role, "content": turn.text }));
    }
    input_messages.push(json!({ "role": "user", "content": prompt_for_model }));

    json!({
        "model": model,
        "input": input_messages,
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
    })
}

/// Build a fallback `RepromptResult` that passes the original through unchanged.
fn fallback_result(original: &str) -> RepromptResult {
    RepromptResult {
        refined_prompt: original.to_string(),
        applied_rules: vec![],
        reasoning: "Refinement API call failed; returning original input.".to_string(),
        task_type: TaskType::Analysis,
        was_substantive_change: false,
        tips: vec![],
    }
}

fn finalize_result(
    result: RepromptResult,
    redaction_mappings: &[codex_secrets::RedactionMapping],
) -> RepromptResult {
    let refined_prompt = if redaction_mappings.is_empty() {
        result.refined_prompt
    } else {
        codex_secrets::rehydrate_redacted_text(&result.refined_prompt, redaction_mappings)
    };
    RepromptResult {
        refined_prompt,
        ..result
    }
}

/// Refine a user's input via an API call.
///
/// This is a stateless, single-shot call — it is never part of the Codex
/// thread. On failure, returns the original text wrapped in a `RepromptResult`
/// with `was_substantive_change = false`.
pub(crate) async fn refine_input(
    original: &str,
    prompt_for_model: &str,
    config: &RepromptConfig,
    auth: &super::RepromptAuthInfo,
    profile: &RepromptProfile,
    prompt_context: &RefinementPromptContext<'_>,
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

    let system_prompt = build_system_prompt(base_prompt, &rules_text, prompt_context);

    tracing::debug!(
        "REPROMPT: profile={}, model={}, rules_chars={}, custom_system_prompt={}, context_turns={}, relevant_files={}, relevant_skills={}, relevant_plugins={}, relevant_apps={}, project_structure_chars={}",
        profile.name,
        model,
        rules_text.len(),
        profile.system_prompt.is_some(),
        prompt_context.conversation.len(),
        prompt_context.relevant_files.len(),
        prompt_context.relevant_skills.len(),
        prompt_context.relevant_plugins.len(),
        prompt_context.relevant_apps.len(),
        prompt_context.project_structure.map_or(0, str::len),
    );

    let schema = reprompt_result_json_schema();
    let request_body = build_request_body(
        model,
        prompt_for_model,
        &system_prompt,
        prompt_context.conversation,
        &schema,
    );

    let client = build_reqwest_client();
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
            let result = finalize_result(result, prompt_context.redaction_mappings);
            tracing::debug!(
                "Reprompt refinement succeeded: substantive={}, task={}, reasoning_chars={}, refined_chars={}, redactions={}",
                result.was_substantive_change,
                result.task_type,
                result.reasoning.len(),
                result.refined_prompt.len(),
                prompt_context.redaction_mappings.len(),
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

    fn empty_prompt_context<'a>() -> RefinementPromptContext<'a> {
        RefinementPromptContext {
            conversation: &[],
            project_structure: None,
            relevant_files: &[],
            relevant_skills: &[],
            relevant_plugins: &[],
            relevant_apps: &[],
            redaction_mappings: &[],
        }
    }

    #[test]
    fn fallback_result_preserves_original() {
        let result = fallback_result("my original prompt");
        assert_eq!(result.refined_prompt, "my original prompt");
        assert!(!result.was_substantive_change);
        assert!(result.applied_rules.is_empty());
    }

    #[test]
    fn build_system_prompt_substitutes_placeholders() {
        let prompt = build_system_prompt(
            REFINEMENT_SYSTEM_PROMPT,
            "always include tests",
            &empty_prompt_context(),
        );
        assert!(prompt.contains("always include tests"));
    }

    #[test]
    fn build_system_prompt_with_empty_rules() {
        let prompt = build_system_prompt(REFINEMENT_SYSTEM_PROMPT, "", &empty_prompt_context());
        assert!(prompt.contains("Step 2: Apply rules"));
        assert!(!prompt.contains("{reprompt_rules}"));
    }

    #[test]
    fn build_system_prompt_appends_project_structure() {
        let prompt_context = RefinementPromptContext {
            project_structure: Some("src/\n  main.rs"),
            ..empty_prompt_context()
        };
        let prompt = build_system_prompt(REFINEMENT_SYSTEM_PROMPT, "", &prompt_context);

        assert!(prompt.contains("## Project structure"));
        assert!(prompt.contains("<project_structure>"));
        assert!(prompt.contains("src/\n  main.rs"));
        assert!(prompt.contains("</project_structure>"));
    }

    #[test]
    fn build_system_prompt_includes_relevant_files_and_tools() {
        let relevant_files = ["@src/auth/token.rs".to_string()];
        let skills = [RelevantSkillPrompt {
            token: "repo:linter".to_string(),
            display_name: "Repo Linter".to_string(),
            description: Some("Run linters across the repo".to_string()),
            path: "/tmp/repo:linter/SKILL.md".into(),
        }];
        let plugins = [RelevantPluginPrompt {
            token: "calendar".to_string(),
            display_name: "Google Calendar".to_string(),
            description: Some("Plugin for event automation".to_string()),
            path: "plugin://calendar@debug".to_string(),
        }];
        let apps = [RelevantAppPrompt {
            token: "google-calendar".to_string(),
            display_name: "Google Calendar".to_string(),
            description: Some("Check availability".to_string()),
            path: "app://google_calendar".to_string(),
        }];
        let prompt_context = RefinementPromptContext {
            relevant_files: &relevant_files,
            relevant_skills: &skills,
            relevant_plugins: &plugins,
            relevant_apps: &apps,
            ..empty_prompt_context()
        };
        let prompt = build_system_prompt(REFINEMENT_SYSTEM_PROMPT, "", &prompt_context);

        assert!(prompt.contains("## Relevant files"));
        assert!(prompt.contains("@src/auth/token.rs"));
        assert!(prompt.contains("## Relevant skills"));
        assert!(prompt.contains("$repo:linter"));
        assert!(prompt.contains("## Relevant plugins"));
        assert!(prompt.contains("$calendar"));
        assert!(prompt.contains("## Relevant apps"));
        assert!(prompt.contains("$google-calendar"));
        assert!(!prompt.contains("## Project structure"));
    }

    #[test]
    fn build_request_body_uses_redacted_prompt_and_instructions() {
        let redaction = codex_secrets::redact_secrets_structured(
            "use sk-abcdefghijklmnopqrstuvwxyz123456 for auth",
            &Default::default(),
        );
        let schema = reprompt_result_json_schema();
        let relevant_files = ["@src/auth.rs".to_string()];
        let prompt_context = RefinementPromptContext {
            project_structure: Some("src/\n  auth.rs"),
            relevant_files: &relevant_files,
            ..empty_prompt_context()
        };
        let instructions = build_system_prompt(REFINEMENT_SYSTEM_PROMPT, "rule", &prompt_context);
        let body = build_request_body(
            "o4-mini",
            &redaction.redacted_text,
            &instructions,
            &[super::super::thread_context::ContextTurn {
                role: super::super::thread_context::ContextRole::Assistant,
                text: "Earlier answer".to_string(),
            }],
            &schema,
        );

        assert_eq!(body["model"], "o4-mini");
        assert_eq!(body["input"].as_array().expect("input array").len(), 2);
        assert_eq!(body["input"][0]["role"], "assistant");
        assert_eq!(
            body["input"][1]["content"],
            serde_json::Value::String(redaction.redacted_text)
        );
        let instructions = body["instructions"].as_str().expect("instructions");
        assert!(instructions.contains("## Relevant files"));
        assert!(instructions.contains("@src/auth.rs"));
        assert!(!instructions.contains("## Project structure"));
    }

    #[test]
    fn finalize_result_rehydrates_only_refined_prompt() {
        let redaction = codex_secrets::redact_secrets_structured(
            "use sk-abcdefghijklmnopqrstuvwxyz123456 for auth",
            &Default::default(),
        );
        let placeholder = redaction
            .mappings
            .first()
            .expect("placeholder mapping")
            .placeholder
            .clone();
        let result = finalize_result(
            RepromptResult {
                refined_prompt: format!("Apply config using {placeholder}"),
                applied_rules: vec!["rule".to_string()],
                reasoning: format!("Reason keeps {placeholder}"),
                task_type: TaskType::Analysis,
                was_substantive_change: true,
                tips: vec![format!("Tip keeps {placeholder}")],
            },
            &redaction.mappings,
        );

        assert_eq!(
            result.refined_prompt,
            "Apply config using sk-abcdefghijklmnopqrstuvwxyz123456"
        );
        assert_eq!(result.reasoning, format!("Reason keeps {placeholder}"));
        assert_eq!(result.tips, vec![format!("Tip keeps {placeholder}")]);
    }

    #[test]
    fn json_schema_is_valid() {
        let schema = reprompt_result_json_schema();
        assert!(schema.get("name").is_some());
        assert!(schema.get("schema").is_some());
        let inner = schema.get("schema").unwrap();
        let required = inner.get("required").unwrap().as_array().unwrap();
        assert_eq!(required.len(), 6);
        assert!(required.contains(&serde_json::Value::String("tips".to_string())));
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
        let prompt_context = empty_prompt_context();
        let result = refine_input(
            "test prompt",
            "test prompt",
            &config,
            &auth,
            &profile,
            &prompt_context,
        )
        .await
        .unwrap();
        assert_eq!(result.refined_prompt, "test prompt");
        assert!(!result.was_substantive_change);
    }

    #[test]
    fn build_request_body_omits_project_structure_when_none() {
        let schema = reprompt_result_json_schema();
        let instructions = build_system_prompt(
            "Custom prompt {reprompt_rules}",
            "rule",
            &empty_prompt_context(),
        );
        let body = build_request_body("o4-mini", "simple prompt", &instructions, &[], &schema);

        let instructions = body["instructions"].as_str().expect("instructions");
        assert!(instructions.starts_with("Custom prompt rule"));
        assert!(!instructions.contains("## Project structure"));
    }
}
