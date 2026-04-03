//! Reprompt profile configuration loading from `~/.codex/reprompt/`.
//!
//! Each reprompt profile is defined by a TOML file such as
//! `~/.codex/reprompt/default.toml`. Users can create multiple profiles
//! (e.g. `security.toml`, `concise.toml`, `bugfix.toml`) and switch
//! between them via the `/reprompt` picker.
//!
//! Rules can be tagged with a `task_type` so the refinement engine applies
//! only the relevant group. Rules without a `task_type` are global and always
//! apply.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// A single rule with an optional task-type tag.
#[derive(Debug, Clone)]
pub(crate) struct TaggedRule {
    /// The rule text.
    pub rule: String,
    /// If set, this rule only applies when the detected task type matches.
    /// If `None`, the rule applies to all task types (global).
    pub task_type: Option<String>,
}

/// Reprompt profile loaded from a TOML file in `~/.codex/reprompt/`.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct RepromptProfile {
    /// Profile name (derived from filename or `[reprompt].name`).
    pub name: String,
    /// Model to use for the refinement call.
    pub model: String,
    /// Human-readable description shown in the picker.
    pub description: Option<String>,
    /// Custom system prompt (overrides the built-in refinement prompt).
    pub system_prompt: Option<String>,
    /// Rules with optional task-type tags.
    pub rules: Vec<TaggedRule>,
    /// Minimum input length to trigger refinement.
    pub min_length: Option<usize>,
    /// Seconds before auto-accepting the refined prompt.
    pub auto_accept_delay_secs: Option<u64>,
    /// Override the number of context turns for this profile. `Some(0)` disables.
    pub context_turns: Option<usize>,
}

impl Default for RepromptProfile {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            model: "o4-mini".to_string(),
            description: Some("Default prompt refinement".to_string()),
            system_prompt: None,
            rules: vec![],
            min_length: None,
            auto_accept_delay_secs: None,
            context_turns: None,
        }
    }
}

impl RepromptProfile {
    /// Format rules for injection into the `{reprompt_rules}` placeholder.
    ///
    /// Rules are grouped by task type. Global rules (no task_type) appear first,
    /// then each task type group is listed with a header. This structured format
    /// lets the LLM identify which rules to apply based on detected intent.
    pub(crate) fn format_rules_for_prompt(&self) -> String {
        if self.rules.is_empty() {
            return String::new();
        }

        let mut global_rules: Vec<&str> = Vec::new();
        let mut by_type: BTreeMap<&str, Vec<&str>> = BTreeMap::new();

        for rule in &self.rules {
            match &rule.task_type {
                Some(tt) => by_type.entry(tt.as_str()).or_default().push(&rule.rule),
                None => global_rules.push(&rule.rule),
            }
        }

        let mut output = String::new();

        if !global_rules.is_empty() {
            output.push_str("### Global rules (always apply):\n");
            for rule in &global_rules {
                output.push_str(&format!("- {rule}\n"));
            }
        }

        for (task_type, rules) in &by_type {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&format!("### Rules for {task_type}:\n"));
            for rule in rules {
                output.push_str(&format!("- {rule}\n"));
            }
        }

        output
    }
}

/// Raw TOML structure for deserialization.
#[derive(Debug, Deserialize)]
struct ProfileToml {
    reprompt: Option<RepromptSection>,
}

#[derive(Debug, Deserialize)]
struct RepromptSection {
    name: Option<String>,
    model: Option<String>,
    description: Option<String>,
    prompt: Option<PromptSection>,
    min_length: Option<usize>,
    auto_accept_delay_secs: Option<u64>,
    context_turns: Option<usize>,
    /// Individual rules: `[[reprompt.rules]]` with `rule` + optional `task_type`.
    rules: Option<Vec<RuleEntry>>,
    /// Grouped rules: `[[reprompt.rule_groups]]` with `task_type` + `rules` array.
    rule_groups: Option<Vec<RuleGroupEntry>>,
}

#[derive(Debug, Deserialize)]
struct PromptSection {
    system: Option<String>,
    system_file: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RuleEntry {
    rule: String,
    /// Optional task type this rule applies to. Omit for global rules.
    task_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RuleGroupEntry {
    /// Optional task type. Omit for global rules.
    task_type: Option<String>,
    /// List of rule strings in this group.
    rules: Vec<String>,
}

/// Load a reprompt profile from `~/.codex/reprompt/<profile_name>.toml`.
///
/// Returns the default profile if the file does not exist.
pub(crate) fn load_reprompt_profile(codex_home: &Path, profile_name: &str) -> RepromptProfile {
    let profiles_dir = codex_home.join("reprompt");
    let config_path = profiles_dir.join(format!("{profile_name}.toml"));

    if !config_path.exists() {
        return RepromptProfile {
            name: profile_name.to_string(),
            ..RepromptProfile::default()
        };
    }

    let contents = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                "Failed to read reprompt profile {}: {e}",
                config_path.display()
            );
            return RepromptProfile {
                name: profile_name.to_string(),
                ..RepromptProfile::default()
            };
        }
    };

    let toml: ProfileToml = match toml::from_str(&contents) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(
                "Failed to parse reprompt profile {}: {e}",
                config_path.display()
            );
            return RepromptProfile {
                name: profile_name.to_string(),
                ..RepromptProfile::default()
            };
        }
    };

    let Some(section) = toml.reprompt else {
        return RepromptProfile {
            name: profile_name.to_string(),
            ..RepromptProfile::default()
        };
    };

    let system_prompt = section.prompt.and_then(|p| {
        if let Some(prompt) = p.system {
            Some(prompt)
        } else if let Some(file) = p.system_file {
            let prompt_path = if file.starts_with('~') {
                dirs::home_dir()
                    .map(|h| h.join(&file[2..]))
                    .unwrap_or_else(|| profiles_dir.join(&file))
            } else if Path::new(&file).is_relative() {
                profiles_dir.join(&file)
            } else {
                file.into()
            };
            match std::fs::read_to_string(&prompt_path) {
                Ok(content) => Some(content),
                Err(e) => {
                    tracing::warn!(
                        "Failed to read system prompt file {}: {e}",
                        prompt_path.display()
                    );
                    None
                }
            }
        } else {
            None
        }
    });

    let mut rules: Vec<TaggedRule> = section
        .rules
        .unwrap_or_default()
        .into_iter()
        .map(|r| TaggedRule {
            rule: r.rule,
            task_type: r.task_type,
        })
        .collect();

    // Merge grouped rules: [[reprompt.rule_groups]] with task_type + rules array.
    for group in section.rule_groups.unwrap_or_default() {
        for rule in group.rules {
            rules.push(TaggedRule {
                rule,
                task_type: group.task_type.clone(),
            });
        }
    }

    RepromptProfile {
        name: section.name.unwrap_or_else(|| profile_name.to_string()),
        model: section.model.unwrap_or_else(|| "o4-mini".to_string()),
        description: section.description,
        system_prompt,
        rules,
        min_length: section.min_length,
        auto_accept_delay_secs: section.auto_accept_delay_secs,
        context_turns: section.context_turns,
    }
}

/// List available reprompt profiles from `~/.codex/reprompt/`.
///
/// Returns a list of profile names (filenames without `.toml` extension).
pub(crate) fn list_reprompt_profiles(codex_home: &Path) -> Vec<String> {
    let profiles_dir = codex_home.join("reprompt");
    if !profiles_dir.exists() {
        return vec![];
    }

    let mut profiles = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&profiles_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                profiles.push(stem.to_string());
            }
        }
    }
    profiles.sort();
    profiles
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn default_profile_when_no_file() {
        let tmp = TempDir::new().unwrap();
        let profile = load_reprompt_profile(tmp.path(), "default");
        assert_eq!(profile.name, "default");
        assert_eq!(profile.model, "o4-mini");
        assert!(profile.system_prompt.is_none());
        assert!(profile.rules.is_empty());
    }

    #[test]
    fn loads_profile_with_tagged_rules() {
        let tmp = TempDir::new().unwrap();
        let profiles_dir = tmp.path().join("reprompt");
        fs::create_dir_all(&profiles_dir).unwrap();
        fs::write(
            profiles_dir.join("test.toml"),
            r#"
[reprompt]
name = "test"
model = "gpt-4o"

[[reprompt.rules]]
rule = "Global rule applies everywhere"

[[reprompt.rules]]
task_type = "bugfix"
rule = "Require reproduction steps"

[[reprompt.rules]]
task_type = "bugfix"
rule = "Add regression test"

[[reprompt.rules]]
task_type = "security"
rule = "Check OWASP top 10"

[[reprompt.rules]]
task_type = "security"
rule = "Verify input sanitization"
"#,
        )
        .unwrap();

        let profile = load_reprompt_profile(tmp.path(), "test");
        assert_eq!(profile.rules.len(), 5);

        // Global rule
        assert!(profile.rules[0].task_type.is_none());
        assert_eq!(profile.rules[0].rule, "Global rule applies everywhere");

        // Bugfix rules
        assert_eq!(profile.rules[1].task_type.as_deref(), Some("bugfix"));
        assert_eq!(profile.rules[2].task_type.as_deref(), Some("bugfix"));

        // Security rules
        assert_eq!(profile.rules[3].task_type.as_deref(), Some("security"));
        assert_eq!(profile.rules[4].task_type.as_deref(), Some("security"));
    }

    #[test]
    fn format_rules_groups_by_task_type() {
        let profile = RepromptProfile {
            rules: vec![
                TaggedRule {
                    rule: "Always test".to_string(),
                    task_type: None,
                },
                TaggedRule {
                    rule: "Require repro steps".to_string(),
                    task_type: Some("bugfix".to_string()),
                },
                TaggedRule {
                    rule: "Add regression test".to_string(),
                    task_type: Some("bugfix".to_string()),
                },
                TaggedRule {
                    rule: "Check OWASP".to_string(),
                    task_type: Some("security".to_string()),
                },
            ],
            ..RepromptProfile::default()
        };

        let formatted = profile.format_rules_for_prompt();
        assert!(formatted.contains("### Global rules (always apply):"));
        assert!(formatted.contains("- Always test"));
        assert!(formatted.contains("### Rules for bugfix:"));
        assert!(formatted.contains("- Require repro steps"));
        assert!(formatted.contains("- Add regression test"));
        assert!(formatted.contains("### Rules for security:"));
        assert!(formatted.contains("- Check OWASP"));
    }

    #[test]
    fn format_rules_empty_when_no_rules() {
        let profile = RepromptProfile::default();
        assert!(profile.format_rules_for_prompt().is_empty());
    }

    #[test]
    fn format_rules_only_global() {
        let profile = RepromptProfile {
            rules: vec![TaggedRule {
                rule: "Always be concise".to_string(),
                task_type: None,
            }],
            ..RepromptProfile::default()
        };
        let formatted = profile.format_rules_for_prompt();
        assert!(formatted.contains("### Global rules"));
        assert!(!formatted.contains("### Rules for"));
    }

    #[test]
    fn format_rules_only_typed() {
        let profile = RepromptProfile {
            rules: vec![TaggedRule {
                rule: "Require repro steps".to_string(),
                task_type: Some("bugfix".to_string()),
            }],
            ..RepromptProfile::default()
        };
        let formatted = profile.format_rules_for_prompt();
        assert!(!formatted.contains("### Global rules"));
        assert!(formatted.contains("### Rules for bugfix:"));
    }

    #[test]
    fn list_profiles_finds_all_toml_files() {
        let tmp = TempDir::new().unwrap();
        let profiles_dir = tmp.path().join("reprompt");
        fs::create_dir_all(&profiles_dir).unwrap();
        fs::write(profiles_dir.join("default.toml"), "[reprompt]").unwrap();
        fs::write(profiles_dir.join("security.toml"), "[reprompt]").unwrap();
        fs::write(profiles_dir.join("concise.toml"), "[reprompt]").unwrap();

        let profiles = list_reprompt_profiles(tmp.path());
        assert_eq!(profiles, vec!["concise", "default", "security"]);
    }

    #[test]
    fn list_profiles_empty_when_no_dir() {
        let tmp = TempDir::new().unwrap();
        let profiles = list_reprompt_profiles(tmp.path());
        assert!(profiles.is_empty());
    }

    #[test]
    fn loads_profile_with_rule_groups() {
        let tmp = TempDir::new().unwrap();
        let profiles_dir = tmp.path().join("reprompt");
        fs::create_dir_all(&profiles_dir).unwrap();
        fs::write(
            profiles_dir.join("grouped.toml"),
            r#"
[reprompt]
name = "grouped"

[[reprompt.rule_groups]]
rules = [
    "Global rule A",
    "Global rule B",
]

[[reprompt.rule_groups]]
task_type = "bugfix"
rules = [
    "Require reproduction steps",
    "Add regression test",
    "Check related code paths",
]

[[reprompt.rule_groups]]
task_type = "security"
rules = [
    "Check OWASP top 10",
    "Verify input sanitization",
]
"#,
        )
        .unwrap();

        let profile = load_reprompt_profile(tmp.path(), "grouped");
        assert_eq!(profile.rules.len(), 7);

        // Global rules
        assert!(profile.rules[0].task_type.is_none());
        assert_eq!(profile.rules[0].rule, "Global rule A");
        assert!(profile.rules[1].task_type.is_none());
        assert_eq!(profile.rules[1].rule, "Global rule B");

        // Bugfix rules
        assert_eq!(profile.rules[2].task_type.as_deref(), Some("bugfix"));
        assert_eq!(profile.rules[2].rule, "Require reproduction steps");
        assert_eq!(profile.rules[3].task_type.as_deref(), Some("bugfix"));
        assert_eq!(profile.rules[4].task_type.as_deref(), Some("bugfix"));

        // Security rules
        assert_eq!(profile.rules[5].task_type.as_deref(), Some("security"));
        assert_eq!(profile.rules[6].task_type.as_deref(), Some("security"));
    }

    #[test]
    fn loads_profile_with_mixed_rules_and_groups() {
        let tmp = TempDir::new().unwrap();
        let profiles_dir = tmp.path().join("reprompt");
        fs::create_dir_all(&profiles_dir).unwrap();
        fs::write(
            profiles_dir.join("mixed.toml"),
            r#"
[reprompt]
name = "mixed"

[[reprompt.rules]]
rule = "Individual global rule"

[[reprompt.rules]]
task_type = "bugfix"
rule = "Individual bugfix rule"

[[reprompt.rule_groups]]
task_type = "security"
rules = [
    "Grouped security rule A",
    "Grouped security rule B",
]
"#,
        )
        .unwrap();

        let profile = load_reprompt_profile(tmp.path(), "mixed");
        assert_eq!(profile.rules.len(), 4);

        // Individual rules come first
        assert!(profile.rules[0].task_type.is_none());
        assert_eq!(profile.rules[0].rule, "Individual global rule");
        assert_eq!(profile.rules[1].task_type.as_deref(), Some("bugfix"));

        // Grouped rules appended after
        assert_eq!(profile.rules[2].task_type.as_deref(), Some("security"));
        assert_eq!(profile.rules[2].rule, "Grouped security rule A");
        assert_eq!(profile.rules[3].task_type.as_deref(), Some("security"));
    }
}
