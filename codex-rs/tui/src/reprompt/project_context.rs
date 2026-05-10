//! Project structure context for `/reprompt`.
//!
//! Builds a compact, deterministic directory tree from a working directory,
//! respecting ignore files and explicit exclusion rules. The rendered context
//! is cached with a TTL so frequent reprompt turns do not re-walk the tree.

use ignore::WalkBuilder;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

const HARD_EXCLUDE_NAMES: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    "coverage",
    "__pycache__",
    ".gitignore",
    ".ignore",
    ".venv",
    "venv",
    "out",
];

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProjectContextOptions {
    pub enabled: bool,
    pub max_depth: usize,
    pub max_chars: usize,
    pub cache_ttl: Duration,
    pub extra_excludes: Vec<String>,
}

impl Default for ProjectContextOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            max_depth: 4,
            max_chars: 2000,
            cache_ttl: Duration::from_secs(30),
            extra_excludes: vec![],
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ProjectContextResult {
    pub context: Option<String>,
    pub snapshot: Option<ProjectContextSnapshot>,
    pub cache_hit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ProjectContextSnapshot {
    pub entries: Vec<ProjectContextEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectContextEntry {
    pub relative_path: String,
    pub absolute_path: PathBuf,
    pub basename: String,
    pub basename_lower: String,
    pub basename_stem_lower: String,
    pub normalized_relative_path: String,
    pub normalized_segments: Vec<String>,
}

impl ProjectContextEntry {
    fn new(cwd: &Path, rel: &Path) -> Self {
        let relative_path = rel.to_string_lossy().replace('\\', "/");
        let basename = rel
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| relative_path.clone());
        let basename_lower = basename.to_ascii_lowercase();
        let basename_stem_lower = rel
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_else(|| basename_lower.clone());
        let normalized_relative_path = relative_path.to_ascii_lowercase();

        Self {
            absolute_path: cwd.join(rel),
            normalized_segments: collect_normalized_segments(&relative_path),
            relative_path,
            basename,
            basename_lower,
            basename_stem_lower,
            normalized_relative_path,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(relative_path: &str) -> Self {
        Self::new(Path::new("/tmp/project"), Path::new(relative_path))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    cwd: PathBuf,
    max_depth: usize,
    max_chars: usize,
    extra_excludes: Vec<String>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    generated_at: Instant,
    context: Option<String>,
    snapshot: Option<ProjectContextSnapshot>,
}

#[derive(Default)]
pub(crate) struct ProjectContextCache {
    entries: HashMap<CacheKey, CacheEntry>,
}

impl ProjectContextCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn get_or_build(
        &mut self,
        cwd: &Path,
        options: &ProjectContextOptions,
    ) -> ProjectContextResult {
        if !options.enabled {
            return ProjectContextResult {
                context: None,
                snapshot: None,
                cache_hit: false,
            };
        }

        let key = CacheKey {
            cwd: canonicalize_lossy(cwd),
            max_depth: options.max_depth,
            max_chars: options.max_chars,
            extra_excludes: normalized_extra_excludes(&options.extra_excludes),
        };

        let now = Instant::now();
        if let Some(entry) = self.entries.get(&key)
            && now.saturating_duration_since(entry.generated_at) < options.cache_ttl
        {
            return ProjectContextResult {
                context: entry.context.clone(),
                snapshot: entry.snapshot.clone(),
                cache_hit: true,
            };
        }

        let scan = scan_project_context(&key.cwd, options.max_depth, &key.extra_excludes);
        let context = scan
            .as_ref()
            .and_then(|scan| render_project_context(&scan.tree, options.max_chars));
        let snapshot = scan.map(|scan| ProjectContextSnapshot {
            entries: scan.entries,
        });

        self.entries.insert(
            key,
            CacheEntry {
                generated_at: now,
                context: context.clone(),
                snapshot: snapshot.clone(),
            },
        );

        ProjectContextResult {
            context,
            snapshot,
            cache_hit: false,
        }
    }
}

#[derive(Debug)]
struct ProjectContextScan {
    tree: TreeNode,
    entries: Vec<ProjectContextEntry>,
}

#[cfg(test)]
fn build_project_context(
    cwd: &Path,
    max_depth: usize,
    max_chars: usize,
    extra_excludes: &[String],
) -> Option<String> {
    scan_project_context(cwd, max_depth, extra_excludes)
        .and_then(|scan| render_project_context(&scan.tree, max_chars))
}

#[cfg(test)]
fn build_project_context_snapshot(
    cwd: &Path,
    max_depth: usize,
    extra_excludes: &[String],
) -> Option<ProjectContextSnapshot> {
    scan_project_context(cwd, max_depth, extra_excludes).map(|scan| ProjectContextSnapshot {
        entries: scan.entries,
    })
}

fn scan_project_context(
    cwd: &Path,
    max_depth: usize,
    extra_excludes: &[String],
) -> Option<ProjectContextScan> {
    if max_depth == 0 {
        return None;
    }

    let mut tree = TreeNode::new_dir();
    let mut entries = Vec::new();
    let mut has_any_entry = false;
    let mut walk_builder = WalkBuilder::new(cwd);
    walk_builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .require_git(false)
        .max_depth(Some(max_depth));

    for result in walk_builder.build() {
        let entry = match result {
            Ok(entry) => entry,
            Err(err) => {
                tracing::debug!("REPROMPT: project context walker error: {err}");
                continue;
            }
        };

        let path = entry.path();
        if path == cwd {
            continue;
        }
        let rel = match path.strip_prefix(cwd) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if rel.as_os_str().is_empty() || should_exclude(rel, extra_excludes) {
            continue;
        }

        has_any_entry = true;
        let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
        insert_path(&mut tree, rel, is_dir);
        if !is_dir {
            entries.push(ProjectContextEntry::new(cwd, rel));
        }
    }

    has_any_entry.then_some(ProjectContextScan { tree, entries })
}

fn render_project_context(tree: &TreeNode, max_chars: usize) -> Option<String> {
    if max_chars == 0 {
        return None;
    }

    let mut lines = Vec::new();
    render_tree(tree, 0, &mut lines);
    let mut rendered = lines.join("\n");
    if rendered.len() > max_chars {
        rendered = truncate_to_chars(&rendered, max_chars);
    }
    if rendered.trim().is_empty() {
        None
    } else {
        Some(rendered)
    }
}

fn canonicalize_lossy(cwd: &Path) -> PathBuf {
    std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf())
}

fn normalized_extra_excludes(excludes: &[String]) -> Vec<String> {
    excludes
        .iter()
        .map(|e| normalize_exclude(e))
        .filter(|e| !e.is_empty())
        .collect()
}

fn normalize_exclude(exclude: &str) -> String {
    exclude.trim().trim_matches('/').replace('\\', "/")
}

fn should_exclude(rel: &Path, extra_excludes: &[String]) -> bool {
    if rel.components().any(|component| {
        HARD_EXCLUDE_NAMES.contains(&component.as_os_str().to_string_lossy().as_ref())
    }) {
        return true;
    }

    if extra_excludes.is_empty() {
        return false;
    }

    let rel_norm = rel.to_string_lossy().replace('\\', "/");
    for raw_exclude in extra_excludes {
        let exclude = normalize_exclude(raw_exclude);
        if exclude.is_empty() {
            continue;
        }
        if !exclude.contains('/') {
            if rel
                .components()
                .any(|component| component.as_os_str().to_string_lossy() == exclude)
            {
                return true;
            }
            continue;
        }
        if rel_norm == exclude || rel_norm.starts_with(&format!("{exclude}/")) {
            return true;
        }
    }
    false
}

#[derive(Debug, Default)]
struct TreeNode {
    children: BTreeMap<String, TreeNode>,
    is_file: bool,
}

impl TreeNode {
    fn new_dir() -> Self {
        Self {
            children: BTreeMap::new(),
            is_file: false,
        }
    }

    fn new_file() -> Self {
        Self {
            children: BTreeMap::new(),
            is_file: true,
        }
    }
}

fn insert_path(root: &mut TreeNode, rel: &Path, is_dir: bool) {
    let mut node = root;
    let mut components = rel.components().peekable();
    while let Some(component) = components.next() {
        let name = component.as_os_str().to_string_lossy().to_string();
        if components.peek().is_none() {
            if is_dir {
                node.children.entry(name).or_insert_with(TreeNode::new_dir);
            } else {
                node.children.insert(name, TreeNode::new_file());
            }
        } else {
            node = node.children.entry(name).or_insert_with(TreeNode::new_dir);
        }
    }
}

fn render_tree(node: &TreeNode, depth: usize, lines: &mut Vec<String>) {
    for (name, child) in &node.children {
        let indent = "  ".repeat(depth);
        if child.is_file {
            lines.push(format!("{indent}{name}"));
        } else {
            lines.push(format!("{indent}{name}/"));
            render_tree(child, depth + 1, lines);
        }
    }
}

fn truncate_to_chars(input: &str, max_chars: usize) -> String {
    if input.len() <= max_chars {
        return input.to_string();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }
    let target = max_chars - 3;
    let mut end = target;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &input[..end])
}

fn collect_normalized_segments(path: &str) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    for piece in path
        .replace('\\', "/")
        .to_ascii_lowercase()
        .split('/')
        .flat_map(|segment| segment.split(['.', '_', '-']))
    {
        if !piece.is_empty() {
            seen.insert(piece.to_string());
        }
    }
    seen.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::fs;

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(path, contents).expect("write file");
    }

    #[test]
    fn respects_gitignore_entries() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_file(&tmp.path().join(".gitignore"), "ignored_dir/\n*.secret\n");
        write_file(&tmp.path().join("ignored_dir/leak.txt"), "x");
        write_file(&tmp.path().join("visible.txt"), "x");
        write_file(&tmp.path().join("credentials.secret"), "x");

        let context = build_project_context(tmp.path(), 4, 2000, &[]).expect("context");
        assert!(context.contains("visible.txt"));
        assert!(!context.contains("ignored_dir"));
        assert!(!context.contains("credentials.secret"));
        assert!(!context.contains(".gitignore"));
    }

    #[test]
    fn excludes_hardcoded_blocklist_directories() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_file(&tmp.path().join("src/main.rs"), "fn main() {}");
        write_file(
            &tmp.path().join("node_modules/pkg/index.js"),
            "module.exports = {}",
        );
        write_file(&tmp.path().join("target/debug/app"), "bin");

        let context = build_project_context(tmp.path(), 4, 2000, &[]).expect("context");
        assert!(context.contains("src/"));
        assert!(!context.contains("node_modules/"));
        assert!(!context.contains("target/"));
    }

    #[test]
    fn supports_extra_excludes_for_paths_and_names() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_file(&tmp.path().join("src/app/main.rs"), "x");
        write_file(&tmp.path().join("src/generated/client.rs"), "x");
        write_file(&tmp.path().join("docs/generated.md"), "x");

        let excludes = vec!["src/generated".to_string(), "docs".to_string()];
        let context = build_project_context(tmp.path(), 6, 2000, &excludes).expect("context");
        assert!(context.contains("src/"));
        assert!(context.contains("app/"));
        assert!(!context.contains("generated/client.rs"));
        assert!(!context.contains("docs/"));
    }

    #[test]
    fn max_depth_and_max_chars_are_applied() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_file(&tmp.path().join("a/b/c/d/e/file.txt"), "x");
        write_file(&tmp.path().join("root.txt"), "x");

        let depth_limited = build_project_context(tmp.path(), 3, 2000, &[]).expect("context");
        assert!(depth_limited.contains("a/"));
        assert!(!depth_limited.contains("file.txt"));

        let char_limited = build_project_context(tmp.path(), 8, 20, &[]).expect("context");
        assert!(char_limited.len() <= 20);
    }

    #[test]
    fn formatting_is_deterministic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_file(&tmp.path().join("src/z.rs"), "x");
        write_file(&tmp.path().join("src/a.rs"), "x");
        write_file(&tmp.path().join("README.md"), "x");

        let context = build_project_context(tmp.path(), 4, 2000, &[]).expect("context");
        let expected = "README.md\nsrc/\n  a.rs\n  z.rs";
        assert_eq!(context, expected);
    }

    #[test]
    fn cache_hits_and_refreshes_after_ttl() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_file(&tmp.path().join("src/main.rs"), "x");

        let mut cache = ProjectContextCache::new();
        let options = ProjectContextOptions {
            cache_ttl: Duration::from_secs(1),
            ..ProjectContextOptions::default()
        };

        let first = cache.get_or_build(tmp.path(), &options);
        assert!(!first.cache_hit);
        let second = cache.get_or_build(tmp.path(), &options);
        assert!(second.cache_hit);
        assert_eq!(first.context, second.context);
        assert_eq!(first.snapshot, second.snapshot);

        write_file(&tmp.path().join("src/new.rs"), "x");
        std::thread::sleep(Duration::from_millis(1_100));
        let third = cache.get_or_build(tmp.path(), &options);
        assert!(!third.cache_hit);
        let third_context = third.context.expect("context");
        assert!(third_context.contains("new.rs"));
        let third_snapshot = third.snapshot.expect("snapshot");
        assert!(
            third_snapshot
                .entries
                .iter()
                .any(|entry| entry.relative_path == "src/new.rs")
        );
    }

    #[test]
    fn snapshot_contains_normalized_file_entries() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_file(&tmp.path().join("src/auth/token_handler.rs"), "x");

        let snapshot = build_project_context_snapshot(tmp.path(), 4, &[]).expect("snapshot");
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(
            snapshot.entries[0].relative_path,
            "src/auth/token_handler.rs"
        );
        assert_eq!(snapshot.entries[0].basename, "token_handler.rs");
        assert_eq!(snapshot.entries[0].basename_lower, "token_handler.rs");
        assert_eq!(snapshot.entries[0].basename_stem_lower, "token_handler");
        assert_eq!(
            snapshot.entries[0].normalized_segments,
            vec![
                "auth".to_string(),
                "handler".to_string(),
                "rs".to_string(),
                "src".to_string(),
                "token".to_string(),
            ]
        );
    }
}
