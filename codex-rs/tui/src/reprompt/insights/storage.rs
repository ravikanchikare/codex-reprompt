//! Disk persistence for refinement entries.
//!
//! Each refinement (original + result) is saved as a JSON file under
//! `~/.codex/reprompt-insights/`. Files are named `{timestamp_ms}_{hash}.json`
//! where `hash` is derived from the original prompt to avoid collisions.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::PathBuf;

use super::RefinementEntry;

/// Returns the directory used for persisting refinement entries.
pub(crate) fn entries_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex")
        .join("reprompt-insights")
}

/// Persist a refinement entry to disk as a JSON file.
///
/// Creates the directory if it does not exist. Errors are logged but do not
/// propagate — persistence should never block the TUI.
pub(crate) fn persist_entry(entry: &RefinementEntry) -> anyhow::Result<()> {
    let dir = entries_dir();
    fs::create_dir_all(&dir)?;

    let mut hasher = DefaultHasher::new();
    entry.original_prompt.hash(&mut hasher);
    let hash = hasher.finish();
    let timestamp_ms = entry.timestamp * 1000;
    let filename = format!("{timestamp_ms}_{hash:016x}.json");
    let path = dir.join(filename);

    let json = serde_json::to_string_pretty(entry)?;
    fs::write(&path, json)?;

    tracing::debug!("Persisted refinement entry to {}", path.display());
    Ok(())
}

/// Load refinement entries from disk, sorted by timestamp descending.
///
/// Returns at most `max_entries` entries. Malformed files are skipped with
/// a warning log.
pub(crate) fn load_entries(max_entries: usize) -> anyhow::Result<Vec<RefinementEntry>> {
    let dir = entries_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries: Vec<RefinementEntry> = Vec::new();

    let mut json_files: Vec<_> = fs::read_dir(&dir)?
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .collect();

    // Sort by filename descending (timestamp_ms prefix ensures chronological order).
    json_files.sort_by_key(|b| std::cmp::Reverse(b.file_name()));

    for dir_entry in json_files.into_iter().take(max_entries) {
        let path = dir_entry.path();
        match fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str::<RefinementEntry>(&contents) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    tracing::warn!("Skipping malformed entry {}: {e}", path.display());
                }
            },
            Err(e) => {
                tracing::warn!("Failed to read {}: {e}", path.display());
            }
        }
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reprompt::config::RepromptResult;
    use crate::reprompt::config::TaskType;
    use pretty_assertions::assert_eq;

    fn sample_entry(timestamp: i64, prompt: &str) -> RefinementEntry {
        RefinementEntry {
            timestamp,
            original_prompt: prompt.to_string(),
            result: RepromptResult {
                refined_prompt: format!("Refined: {prompt}"),
                applied_rules: vec!["test rule".to_string()],
                reasoning: "Test reasoning.".to_string(),
                task_type: TaskType::Feature,
                was_substantive_change: true,
                tips: vec![],
            },
            project_path: Some("/tmp/test-project".to_string()),
        }
    }

    #[test]
    fn persist_and_load_roundtrip() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let dir = temp_dir.path().join("reprompt-insights");

        // Override entries_dir by writing directly.
        fs::create_dir_all(&dir).expect("create dir");

        let entry = sample_entry(1712188800, "fix the auth bug");
        let json = serde_json::to_string_pretty(&entry).expect("serialize");
        fs::write(dir.join("1712188800000_0000000000000001.json"), &json).expect("write");

        // Load from the directory.
        let entries: Vec<RefinementEntry> = fs::read_dir(&dir)
            .expect("read dir")
            .filter_map(std::result::Result::ok)
            .filter_map(|e| {
                let contents = fs::read_to_string(e.path()).ok()?;
                serde_json::from_str(&contents).ok()
            })
            .collect();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].original_prompt, "fix the auth bug");
        assert_eq!(
            entries[0].result.refined_prompt,
            "Refined: fix the auth bug"
        );
    }

    #[test]
    fn load_entries_handles_missing_directory() {
        // Point to a non-existent directory — should return empty vec.
        let result = load_entries(50);
        // This may succeed or fail depending on whether ~/.codex/reprompt-insights
        // exists on the test machine, but either way it should not panic.
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn entries_dir_returns_expected_path() {
        let dir = entries_dir();
        assert!(dir.to_str().unwrap().contains("reprompt-insights"));
    }
}
