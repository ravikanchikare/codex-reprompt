//! Relevant file/tool matching and refined mention resolution for `/reprompt`.

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::PathBuf;

use codex_chatgpt::connectors::AppInfo;
use codex_chatgpt::connectors::connector_display_label;
use codex_core::connectors::connector_mention_slug;
use codex_core::plugins::PluginCapabilitySummary;
use codex_core::skills::model::SkillMetadata;

use crate::skills_helpers::skill_description;
use crate::skills_helpers::skill_display_name;
use crate::text_formatting::truncate_text;

use super::ProjectContextSnapshot;
use super::RepromptConfig;

const DESCRIPTION_TRUNCATE_LEN: usize = 120;
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "app", "apps", "bug", "check", "do", "feature", "file", "files", "fix",
    "for", "help", "in", "into", "it", "its", "module", "modules", "of", "on", "or", "plugin",
    "plugins", "please", "run", "skill", "skills", "that", "the", "this", "to", "use", "with",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelevantSkillPrompt {
    pub token: String,
    pub display_name: String,
    pub description: Option<String>,
    pub path: PathBuf,
}

impl RelevantSkillPrompt {
    pub(crate) fn render_line(&self) -> String {
        render_tool_prompt_line(&self.token, &self.display_name, self.description.as_deref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelevantPluginPrompt {
    pub token: String,
    pub display_name: String,
    pub description: Option<String>,
    pub path: String,
}

impl RelevantPluginPrompt {
    pub(crate) fn render_line(&self) -> String {
        render_tool_prompt_line(&self.token, &self.display_name, self.description.as_deref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelevantAppPrompt {
    pub token: String,
    pub display_name: String,
    pub description: Option<String>,
    pub path: String,
}

impl RelevantAppPrompt {
    pub(crate) fn render_line(&self) -> String {
        render_tool_prompt_line(&self.token, &self.display_name, self.description.as_deref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct RelevantPromptContext {
    pub files: Vec<String>,
    pub skills: Vec<RelevantSkillPrompt>,
    pub plugins: Vec<RelevantPluginPrompt>,
    pub apps: Vec<RelevantAppPrompt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedSkillInput {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedMentionInput {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ResolvedRepromptInput {
    pub skills: Vec<ResolvedSkillInput>,
    pub mentions: Vec<ResolvedMentionInput>,
    pub tool_tokens: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct RepromptResolutionContext {
    files_by_path: BTreeMap<String, ResolvedMentionInput>,
    skills_by_token: BTreeMap<String, ResolvedSkillInput>,
    plugins_by_token: BTreeMap<String, ResolvedMentionInput>,
    apps_by_token: BTreeMap<String, ResolvedMentionInput>,
}

impl RepromptResolutionContext {
    pub(crate) fn is_empty(&self) -> bool {
        self.files_by_path.is_empty()
            && self.skills_by_token.is_empty()
            && self.plugins_by_token.is_empty()
            && self.apps_by_token.is_empty()
    }

    pub(crate) fn resolve_text(&self, text: &str) -> ResolvedRepromptInput {
        let mut resolved = ResolvedRepromptInput::default();
        let mut seen_skill_paths: HashSet<PathBuf> = HashSet::new();
        let mut seen_mention_paths: HashSet<String> = HashSet::new();
        let bytes = text.as_bytes();
        let mut index = 0usize;

        while index < bytes.len() {
            match bytes[index] {
                b'@' => {
                    let (token, next_index) = parse_path_token(text, index);
                    if let Some(token) = token
                        && let Some(mention) = self.files_by_path.get(token.as_str())
                        && seen_mention_paths.insert(mention.path.clone())
                    {
                        resolved.mentions.push(mention.clone());
                    }
                    index = next_index;
                }
                b'$' => {
                    let (token, next_index) = parse_tool_token(text, index);
                    if let Some(token) = token {
                        let key = token.to_ascii_lowercase();
                        if let Some(skill) = self.skills_by_token.get(key.as_str())
                            && seen_skill_paths.insert(skill.path.clone())
                        {
                            resolved.tool_tokens.insert(key.clone());
                            resolved.skills.push(skill.clone());
                        } else if let Some(plugin) = self.plugins_by_token.get(key.as_str())
                            && seen_mention_paths.insert(plugin.path.clone())
                        {
                            resolved.tool_tokens.insert(key.clone());
                            resolved.mentions.push(plugin.clone());
                        } else if let Some(app) = self.apps_by_token.get(key.as_str())
                            && seen_mention_paths.insert(app.path.clone())
                        {
                            resolved.tool_tokens.insert(key);
                            resolved.mentions.push(app.clone());
                        }
                    }
                    index = next_index;
                }
                _ => index += 1,
            }
        }

        resolved
    }
}

pub(crate) fn build_relevant_prompt_context(
    text: &str,
    snapshot: Option<&ProjectContextSnapshot>,
    skills: &[SkillMetadata],
    plugins: &[PluginCapabilitySummary],
    apps: &[AppInfo],
    config: &RepromptConfig,
) -> RelevantPromptContext {
    let mut relevant = RelevantPromptContext::default();
    let tokens = extract_match_tokens(text);

    if config.include_relevant_files
        && let Some(snapshot) = snapshot
    {
        relevant.files = match_relevant_files(
            &tokens,
            snapshot,
            config.relevant_files_max_count,
            config.relevant_files_max_chars,
        );
    }

    if config.include_relevant_skills {
        relevant.skills = match_relevant_skills(
            &tokens,
            skills,
            config.relevant_skills_max_count,
            config.relevant_skills_max_chars,
        );
    }

    let occupied_tokens: HashSet<String> = relevant
        .skills
        .iter()
        .map(|skill| skill.token.to_ascii_lowercase())
        .collect();

    if config.include_relevant_plugins {
        relevant.plugins = match_relevant_plugins(
            &tokens,
            plugins,
            &occupied_tokens,
            config.relevant_plugins_max_count,
            config.relevant_plugins_max_chars,
        );
    }

    let mut occupied_tokens = occupied_tokens;
    occupied_tokens.extend(
        relevant
            .plugins
            .iter()
            .map(|plugin| plugin.token.to_ascii_lowercase()),
    );

    if config.include_relevant_apps {
        relevant.apps = match_relevant_apps(
            &tokens,
            apps,
            &occupied_tokens,
            config.relevant_apps_max_count,
            config.relevant_apps_max_chars,
        );
    }

    relevant
}

pub(crate) fn build_resolution_context(
    snapshot: Option<&ProjectContextSnapshot>,
    skills: &[SkillMetadata],
    plugins: &[PluginCapabilitySummary],
    apps: &[AppInfo],
) -> RepromptResolutionContext {
    let mut files_by_path = BTreeMap::new();
    if let Some(snapshot) = snapshot {
        for entry in &snapshot.entries {
            files_by_path.insert(
                entry.relative_path.clone(),
                ResolvedMentionInput {
                    name: entry.relative_path.clone(),
                    path: entry.absolute_path.to_string_lossy().into_owned(),
                },
            );
        }
    }

    let mut skills_by_token = BTreeMap::new();
    for skill in skills {
        skills_by_token
            .entry(skill.name.to_ascii_lowercase())
            .or_insert_with(|| ResolvedSkillInput {
                name: skill.name.clone(),
                path: skill.path_to_skills_md.clone(),
            });
    }

    let mut plugins_by_token = BTreeMap::new();
    for plugin in plugins {
        let token = plugin_token(plugin).to_ascii_lowercase();
        if skills_by_token.contains_key(token.as_str()) {
            continue;
        }
        plugins_by_token
            .entry(token)
            .or_insert_with(|| ResolvedMentionInput {
                name: plugin.display_name.clone(),
                path: format!("plugin://{}", plugin.config_name),
            });
    }

    let mut apps_by_token = BTreeMap::new();
    for app in apps
        .iter()
        .filter(|app| app.is_enabled && app.is_accessible)
    {
        let token = connector_mention_slug(app).to_ascii_lowercase();
        if skills_by_token.contains_key(token.as_str())
            || plugins_by_token.contains_key(token.as_str())
        {
            continue;
        }
        apps_by_token.entry(token).or_insert_with(|| {
            let app_id = app.id.as_str();
            ResolvedMentionInput {
                name: connector_display_label(app),
                path: format!("app://{app_id}"),
            }
        });
    }

    RepromptResolutionContext {
        files_by_path,
        skills_by_token,
        plugins_by_token,
        apps_by_token,
    }
}

fn match_relevant_files(
    tokens: &[String],
    snapshot: &ProjectContextSnapshot,
    max_count: usize,
    max_chars: usize,
) -> Vec<String> {
    let mut scored = snapshot
        .entries
        .iter()
        .filter_map(|entry| {
            let score = score_file_entry(entry, tokens);
            (score > 0).then_some((score, entry.relative_path.clone()))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(left_score, left_path), (right_score, right_path)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_path.cmp(right_path))
    });

    cap_formatted_items(
        scored.into_iter().map(|(_, path)| format!("@{path}")),
        max_count,
        max_chars,
    )
}

fn match_relevant_skills(
    tokens: &[String],
    skills: &[SkillMetadata],
    max_count: usize,
    max_chars: usize,
) -> Vec<RelevantSkillPrompt> {
    let mut scored = skills
        .iter()
        .filter_map(|skill| {
            let score = score_tool_candidate(
                &skill.name,
                skill_display_name(skill),
                skill_description(skill),
                tokens,
            );
            (score > 0).then_some((
                score,
                RelevantSkillPrompt {
                    token: skill.name.clone(),
                    display_name: skill_display_name(skill).to_string(),
                    description: Some(skill_description(skill).to_string())
                        .filter(|description| !description.trim().is_empty()),
                    path: skill.path_to_skills_md.clone(),
                },
            ))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.token.cmp(&right.token))
    });

    cap_prompt_candidates(
        scored.into_iter().map(|(_, skill)| skill),
        max_count,
        max_chars,
    )
}

fn match_relevant_plugins(
    tokens: &[String],
    plugins: &[PluginCapabilitySummary],
    occupied_tokens: &HashSet<String>,
    max_count: usize,
    max_chars: usize,
) -> Vec<RelevantPluginPrompt> {
    let mut scored = plugins
        .iter()
        .filter_map(|plugin| {
            let token = plugin_token(plugin);
            if occupied_tokens.contains(&token.to_ascii_lowercase()) {
                return None;
            }
            let description = plugin_description(plugin);
            let score = score_tool_candidate(
                &token,
                &plugin.display_name,
                description.as_deref().unwrap_or_default(),
                tokens,
            );
            (score > 0).then_some((
                score,
                RelevantPluginPrompt {
                    token,
                    display_name: plugin.display_name.clone(),
                    description,
                    path: format!("plugin://{}", plugin.config_name),
                },
            ))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.token.cmp(&right.token))
    });

    cap_prompt_candidates(
        scored.into_iter().map(|(_, plugin)| plugin),
        max_count,
        max_chars,
    )
}

fn match_relevant_apps(
    tokens: &[String],
    apps: &[AppInfo],
    occupied_tokens: &HashSet<String>,
    max_count: usize,
    max_chars: usize,
) -> Vec<RelevantAppPrompt> {
    let mut scored = apps
        .iter()
        .filter(|app| app.is_enabled && app.is_accessible)
        .filter_map(|app| {
            let token = connector_mention_slug(app);
            if occupied_tokens.contains(&token.to_ascii_lowercase()) {
                return None;
            }
            let display_name = connector_display_label(app);
            let description = app
                .description
                .as_deref()
                .map(str::trim)
                .filter(|description| !description.is_empty())
                .map(str::to_string);
            let score = score_tool_candidate(
                &token,
                &display_name,
                description.as_deref().unwrap_or_default(),
                tokens,
            );
            (score > 0).then_some((
                score,
                RelevantAppPrompt {
                    token,
                    display_name,
                    description,
                    path: format!("app://{}", app.id),
                },
            ))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.token.cmp(&right.token))
    });

    cap_prompt_candidates(scored.into_iter().map(|(_, app)| app), max_count, max_chars)
}

fn render_tool_prompt_line(token: &str, display_name: &str, description: Option<&str>) -> String {
    let mut line = format!("- ${token}");
    if display_name != token {
        line.push_str(&format!(" — {display_name}"));
    }
    if let Some(description) = description
        && !description.trim().is_empty()
    {
        if display_name == token {
            line.push_str(" — ");
        } else {
            line.push_str(": ");
        }
        line.push_str(&truncate_text(description.trim(), DESCRIPTION_TRUNCATE_LEN));
    }
    line
}

fn cap_prompt_candidates<T>(
    items: impl IntoIterator<Item = T>,
    max_count: usize,
    max_chars: usize,
) -> Vec<T>
where
    T: Clone,
    T: RenderPromptLine,
{
    let mut kept = Vec::new();
    let mut total_chars = 0usize;
    for item in items {
        if max_count > 0 && kept.len() >= max_count {
            break;
        }
        let line_len = item.render_line().len();
        if !kept.is_empty() && max_chars > 0 && total_chars + 1 + line_len > max_chars {
            break;
        }
        if kept.is_empty() && max_chars > 0 && line_len > max_chars {
            break;
        }
        total_chars += usize::from(!kept.is_empty()) + line_len;
        kept.push(item);
    }
    kept
}

fn cap_formatted_items(
    items: impl IntoIterator<Item = String>,
    max_count: usize,
    max_chars: usize,
) -> Vec<String> {
    let mut kept = Vec::new();
    let mut total_chars = 0usize;
    for item in items {
        if max_count > 0 && kept.len() >= max_count {
            break;
        }
        if !kept.is_empty() && max_chars > 0 && total_chars + 1 + item.len() > max_chars {
            break;
        }
        if kept.is_empty() && max_chars > 0 && item.len() > max_chars {
            break;
        }
        total_chars += usize::from(!kept.is_empty()) + item.len();
        kept.push(item);
    }
    kept
}

trait RenderPromptLine {
    fn render_line(&self) -> String;
}

impl RenderPromptLine for RelevantSkillPrompt {
    fn render_line(&self) -> String {
        RelevantSkillPrompt::render_line(self)
    }
}

impl RenderPromptLine for RelevantPluginPrompt {
    fn render_line(&self) -> String {
        RelevantPluginPrompt::render_line(self)
    }
}

impl RenderPromptLine for RelevantAppPrompt {
    fn render_line(&self) -> String {
        RelevantAppPrompt::render_line(self)
    }
}

fn score_file_entry(
    entry: &crate::reprompt::project_context::ProjectContextEntry,
    tokens: &[String],
) -> i32 {
    let mut score = 0;
    for token in tokens {
        if token == &entry.normalized_relative_path {
            score += 220;
        }
        if token == &entry.basename_lower {
            score += 180;
        }
        if token == &entry.basename_stem_lower {
            score += 160;
        }
        if entry
            .normalized_segments
            .iter()
            .any(|segment| segment == token)
        {
            score += 90;
        }
        if entry.basename_lower.contains(token) {
            score += 60;
        }
        if entry.normalized_relative_path.contains(token) {
            score += 30;
        }
    }
    score
}

fn score_tool_candidate(
    token: &str,
    display_name: &str,
    description: &str,
    tokens: &[String],
) -> i32 {
    let token_lower = token.to_ascii_lowercase();
    let display_lower = display_name.to_ascii_lowercase();
    let description_lower = description.to_ascii_lowercase();
    let search_terms = split_search_terms(&format!(
        "{token_lower} {display_lower} {description_lower}"
    ));

    let mut score = 0;
    for query in tokens {
        if query == &token_lower {
            score += 220;
        }
        if query == &display_lower {
            score += 170;
        }
        if search_terms.iter().any(|term| term == query) {
            score += 110;
        }
        if token_lower.contains(query) {
            score += 70;
        }
        if display_lower.contains(query) {
            score += 45;
        }
        if description_lower.contains(query) {
            score += 25;
        }
    }
    score
}

fn extract_match_tokens(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut tokens = Vec::new();

    for raw in text.split_whitespace() {
        let trimmed = raw.trim_matches(|ch: char| {
            !ch.is_alphanumeric() && !matches!(ch, '/' | '\\' | '.' | '_' | '-' | ':' | '@' | '$')
        });
        if trimmed.is_empty() {
            continue;
        }

        push_match_token(trimmed, &mut seen, &mut tokens);
        for part in split_search_terms(trimmed) {
            push_match_token(part.as_str(), &mut seen, &mut tokens);
        }
    }

    tokens
}

fn push_match_token(raw: &str, seen: &mut HashSet<String>, tokens: &mut Vec<String>) {
    let normalized = raw
        .trim_matches(['@', '$'])
        .trim_matches(|ch: char| matches!(ch, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}'))
        .to_ascii_lowercase();
    if normalized.is_empty() {
        return;
    }

    let is_path_like = raw.contains('/')
        || raw.contains('\\')
        || raw.contains('.')
        || raw.contains(':')
        || raw.contains('_')
        || raw.contains('-');
    if !is_path_like && normalized.len() < 3 {
        return;
    }
    if !is_path_like && STOPWORDS.contains(&normalized.as_str()) {
        return;
    }
    if seen.insert(normalized.clone()) {
        tokens.push(normalized);
    }
}

fn split_search_terms(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn parse_path_token(text: &str, at_index: usize) -> (Option<String>, usize) {
    parse_token(
        text,
        at_index,
        |ch| !ch.is_whitespace(),
        &['.', ',', ';', ':', '!', '?', ')', ']', '}'],
    )
}

fn parse_tool_token(text: &str, dollar_index: usize) -> (Option<String>, usize) {
    parse_token(
        text,
        dollar_index,
        |ch| !ch.is_whitespace(),
        &['.', ',', ';', '!', '?', ')', ']', '}'],
    )
}

fn parse_token(
    text: &str,
    start: usize,
    keep_char: impl Fn(char) -> bool,
    trailing_trim: &[char],
) -> (Option<String>, usize) {
    let mut index = start + 1;
    let mut token = String::new();
    for ch in text[index..].chars() {
        if !keep_char(ch) {
            break;
        }
        token.push(ch);
        index += ch.len_utf8();
    }
    let token = token.trim_end_matches(trailing_trim).to_string();
    if token.is_empty() {
        (None, index.max(start + 1))
    } else {
        (Some(token), index.max(start + 1))
    }
}

fn plugin_token(plugin: &PluginCapabilitySummary) -> String {
    plugin
        .config_name
        .split_once('@')
        .map_or_else(|| plugin.config_name.clone(), |(name, _)| name.to_string())
}

fn plugin_description(plugin: &PluginCapabilitySummary) -> Option<String> {
    if let Some(description) = plugin
        .description
        .as_deref()
        .map(str::trim)
        .filter(|description| !description.is_empty())
    {
        return Some(description.to_string());
    }

    let mut capabilities = Vec::new();
    if plugin.has_skills {
        capabilities.push("skills".to_string());
    }
    if !plugin.mcp_server_names.is_empty() {
        capabilities.push(if plugin.mcp_server_names.len() == 1 {
            "1 MCP server".to_string()
        } else {
            format!("{} MCP servers", plugin.mcp_server_names.len())
        });
    }
    if !plugin.app_connector_ids.is_empty() {
        capabilities.push(if plugin.app_connector_ids.len() == 1 {
            "1 app".to_string()
        } else {
            format!("{} apps", plugin.app_connector_ids.len())
        });
    }

    (!capabilities.is_empty()).then(|| format!("Plugin · {}", capabilities.join(" · ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reprompt::ProjectContextSnapshot;
    use crate::reprompt::project_context::ProjectContextEntry;
    use pretty_assertions::assert_eq;

    fn project_snapshot(paths: &[&str]) -> ProjectContextSnapshot {
        ProjectContextSnapshot {
            entries: paths
                .iter()
                .map(|path| ProjectContextEntry::new_for_test(path))
                .collect(),
        }
    }

    fn skill(name: &str, display_name: &str, description: &str) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            description: description.to_string(),
            short_description: None,
            interface: Some(codex_core::skills::model::SkillInterface {
                display_name: Some(display_name.to_string()),
                short_description: None,
                icon_small: None,
                icon_large: None,
                brand_color: None,
                default_prompt: None,
            }),
            dependencies: None,
            policy: None,
            path_to_skills_md: PathBuf::from(format!("/tmp/{name}/SKILL.md")),
            scope: codex_protocol::protocol::SkillScope::Repo,
        }
    }

    fn plugin(config_name: &str, display_name: &str, description: &str) -> PluginCapabilitySummary {
        PluginCapabilitySummary {
            config_name: config_name.to_string(),
            display_name: display_name.to_string(),
            description: Some(description.to_string()),
            has_skills: true,
            mcp_server_names: vec!["plugin-mcp".to_string()],
            app_connector_ids: Vec::new(),
        }
    }

    fn app(id: &str, name: &str, description: &str) -> AppInfo {
        AppInfo {
            id: id.to_string(),
            name: name.to_string(),
            description: Some(description.to_string()),
            logo_url: None,
            logo_url_dark: None,
            distribution_channel: None,
            branding: None,
            app_metadata: None,
            labels: None,
            install_url: None,
            is_accessible: true,
            is_enabled: true,
            plugin_display_names: Vec::new(),
        }
    }

    #[test]
    fn file_matching_prefers_exact_basename_and_path_segments() {
        let matches = match_relevant_files(
            &extract_match_tokens("fix the auth bug in token.rs"),
            &project_snapshot(&[
                "src/auth/token.rs",
                "src/auth/middleware.rs",
                "src/http/client.rs",
            ]),
            8,
            600,
        );

        assert_eq!(
            matches,
            vec![
                "@src/auth/token.rs".to_string(),
                "@src/auth/middleware.rs".to_string(),
            ]
        );
    }

    #[test]
    fn file_matching_respects_count_and_char_caps() {
        let matches = match_relevant_files(
            &extract_match_tokens("token auth"),
            &project_snapshot(&[
                "src/auth/token.rs",
                "src/auth/middleware.rs",
                "src/auth/session.rs",
            ]),
            2,
            24,
        );

        assert_eq!(matches, vec!["@src/auth/token.rs".to_string()]);
    }

    #[test]
    fn skill_matching_uses_name_and_description_terms() {
        let matches = match_relevant_skills(
            &extract_match_tokens("run the linter skill before submit"),
            &[
                skill("repo:linter", "Repo Linter", "Run linters across the repo"),
                skill(
                    "google-calendar:availability",
                    "Google Calendar",
                    "Check availability",
                ),
            ],
            5,
            600,
        );

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].token, "repo:linter");
    }

    #[test]
    fn plugin_and_app_matching_skip_exact_skill_token_collisions() {
        let config = RepromptConfig::default();
        let relevant = build_relevant_prompt_context(
            "google calendar",
            Some(&project_snapshot(&[])),
            &[skill(
                "google-calendar",
                "Google Calendar",
                "Find availability",
            )],
            &[plugin(
                "google-calendar@debug",
                "Google Calendar",
                "Plugin for calendar tasks",
            )],
            &[app(
                "google_calendar",
                "Google Calendar",
                "Connector for calendar tasks",
            )],
            &config,
        );

        assert_eq!(relevant.skills.len(), 1);
        assert!(relevant.plugins.is_empty());
        assert!(relevant.apps.is_empty());
    }

    #[test]
    fn resolve_text_recovers_files_skills_plugins_and_apps() {
        let context = build_resolution_context(
            Some(&project_snapshot(&["src/auth/token.rs"])),
            &[skill("repo:linter", "Repo Linter", "Run linters")],
            &[plugin(
                "calendar-plugin@debug",
                "Calendar Plugin",
                "Plugin for calendar tasks",
            )],
            &[app(
                "google_calendar",
                "Google Calendar",
                "Connector for calendar tasks",
            )],
        );

        let resolved = context.resolve_text(
            "Check @src/auth/token.rs, then run $repo:linter with $calendar-plugin and $google-calendar.",
        );

        assert_eq!(
            resolved.skills,
            vec![ResolvedSkillInput {
                name: "repo:linter".to_string(),
                path: PathBuf::from("/tmp/repo:linter/SKILL.md"),
            }]
        );
        assert_eq!(
            resolved.mentions,
            vec![
                ResolvedMentionInput {
                    name: "src/auth/token.rs".to_string(),
                    path: "/tmp/project/src/auth/token.rs".to_string(),
                },
                ResolvedMentionInput {
                    name: "Calendar Plugin".to_string(),
                    path: "plugin://calendar-plugin@debug".to_string(),
                },
                ResolvedMentionInput {
                    name: "Google Calendar".to_string(),
                    path: "app://google_calendar".to_string(),
                },
            ]
        );
    }

    #[test]
    fn resolve_text_ignores_unresolved_tokens() {
        let context = build_resolution_context(
            Some(&project_snapshot(&["src/auth/token.rs"])),
            &[],
            &[],
            &[],
        );

        let resolved = context.resolve_text("Check @auth and $unknown.");

        assert_eq!(resolved, ResolvedRepromptInput::default());
    }
}
