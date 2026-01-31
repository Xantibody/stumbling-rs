use anyhow::{Context, Result};
use ignore::WalkBuilder;
use markdown::{mdast::Node, Constructs, ParseOptions};
use rayon::prelude::*;
use serde::Serialize;
use std::{
    fs,
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub path: String,
    pub line_number: usize,
    pub line: String,
}

#[derive(Debug, Serialize)]
pub struct MetadataSearchResult {
    pub path: String,
    pub value: serde_json::Value,
}

/// Parse frontmatter from markdown content using markdown-rs AST.
/// Returns (yaml_string, body) if frontmatter is present.
fn parse_frontmatter(content: &str) -> Option<(String, String)> {
    let options = ParseOptions {
        constructs: Constructs {
            frontmatter: true,
            ..Constructs::default()
        },
        ..ParseOptions::default()
    };

    let ast = markdown::to_mdast(content, &options).ok()?;

    if let Node::Root(root) = ast {
        for child in &root.children {
            if let Node::Yaml(yaml) = child {
                // Get the end position of frontmatter to extract body
                let body = if let Some(pos) = &yaml.position {
                    let end_offset = pos.end.offset;
                    content[end_offset..].trim_start().to_string()
                } else {
                    String::new()
                };
                return Some((yaml.value.clone(), body));
            }
        }
    }
    None
}

/// Read a note from the given path.
/// If `should_parse` is true, separates YAML frontmatter from body.
pub fn read_note(path: &Path, should_parse: bool) -> Result<String> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {}", path.display()))?;

    if !should_parse {
        return Ok(content);
    }

    // Parse frontmatter using markdown-rs AST
    if let Some((yaml_str, body)) = parse_frontmatter(&content) {
        if let Ok(meta) = serde_yaml_ng::from_str::<serde_json::Value>(&yaml_str) {
            let output = serde_json::json!({
                "metadata": meta,
                "body": body
            });
            return Ok(serde_json::to_string_pretty(&output)?);
        }
    }

    Ok(content)
}

/// Search for notes matching the query using parallel processing.
pub fn search_notes(root: &Path, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let regex = grep::regex::RegexMatcher::new(query)
        .with_context(|| format!("Invalid regex pattern: {}", query))?;

    let results: Mutex<Vec<SearchResult>> = Mutex::new(Vec::new());

    // Collect all markdown files first
    let files: Vec<_> = WalkBuilder::new(root)
        .hidden(true) // Skip hidden files/dirs
        .filter_entry(|e| {
            // Skip .obsidian and other common ignored directories
            let name = e.file_name().to_string_lossy();
            !name.starts_with('.')
        })
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        .map(|e| e.into_path())
        .collect();

    // Search files in parallel using rayon
    files.par_iter().for_each(|path| {
        if let Ok(content) = fs::read_to_string(path) {
            let relative_path = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            for (line_num, line) in content.lines().enumerate() {
                if grep::matcher::Matcher::is_match(&regex, line.as_bytes()).unwrap_or(false) {
                    let mut results = results
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if results.len() < limit {
                        results.push(SearchResult {
                            path: relative_path.clone(),
                            line_number: line_num + 1,
                            line: line.to_string(),
                        });
                    }
                }
            }
        }
    });

    Ok(results
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner()))
}

/// Get a nested field value from JSON using dot notation (e.g., "author.name").
fn get_nested_field<'a>(
    value: &'a serde_json::Value,
    field: &str,
) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for part in field.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

/// Check if a JSON value matches a regex pattern.
fn value_matches_pattern(value: &serde_json::Value, regex: &regex::Regex) -> bool {
    match value {
        serde_json::Value::String(s) => regex.is_match(s),
        serde_json::Value::Number(n) => regex.is_match(&n.to_string()),
        serde_json::Value::Bool(b) => regex.is_match(&b.to_string()),
        serde_json::Value::Array(arr) => arr.iter().any(|v| value_matches_pattern(v, regex)),
        _ => false,
    }
}

/// Search notes by frontmatter metadata field.
pub fn search_metadata(
    root: &Path,
    field: &str,
    pattern: &str,
    limit: usize,
) -> Result<Vec<MetadataSearchResult>> {
    let regex = regex::Regex::new(pattern)
        .with_context(|| format!("Invalid regex pattern: {}", pattern))?;

    let results: Mutex<Vec<MetadataSearchResult>> = Mutex::new(Vec::new());

    // Collect all markdown files
    let files: Vec<_> = WalkBuilder::new(root)
        .hidden(true)
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !name.starts_with('.')
        })
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        .map(|e| e.into_path())
        .collect();

    // Search files in parallel
    files.par_iter().for_each(|path| {
        if let Ok(content) = fs::read_to_string(path) {
            // Parse frontmatter using markdown-rs AST
            if let Some((yaml_str, _)) = parse_frontmatter(&content) {
                if let Ok(meta) = serde_yaml_ng::from_str::<serde_json::Value>(&yaml_str) {
                    if let Some(value) = get_nested_field(&meta, field) {
                        if value_matches_pattern(value, &regex) {
                            let relative_path = path
                                .strip_prefix(root)
                                .unwrap_or(path)
                                .to_string_lossy()
                                .to_string();

                            let mut results = results
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            if results.len() < limit {
                                results.push(MetadataSearchResult {
                                    path: relative_path,
                                    value: value.clone(),
                                });
                            }
                        }
                    }
                }
            }
        }
    });

    Ok(results
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner()))
}

/// Format content with YAML frontmatter.
///
/// Note: AI tools (e.g., Claude) sometimes serialize metadata as a JSON string
/// `"{\"title\": ...}"` instead of passing a JSON object `{"title": ...}`.
/// This function handles both cases by parsing string values as JSON.
pub fn format_with_frontmatter(metadata: &serde_json::Value, body: &str) -> String {
    let meta = if let serde_json::Value::String(s) = metadata {
        serde_json::from_str(s).unwrap_or_else(|_| metadata.clone())
    } else {
        metadata.clone()
    };

    let yaml = serde_yaml_ng::to_string(&meta).unwrap_or_default();
    // serde_yaml_ng adds a trailing newline, so we trim it
    let yaml = yaml.trim_end();
    format!("---\n{}\n---\n\n{}", yaml, body)
}

/// Write content to a note file.
/// Creates parent directories if they don't exist.
/// Uses atomic write (write to temp, then rename) to prevent data corruption.
pub fn write_note(path: &Path, content: &str) -> Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Atomic write: write to temp file, then rename
    let temp_path = path.with_extension("tmp");
    fs::write(&temp_path, content)
        .with_context(|| format!("Failed to write temp file: {}", temp_path.display()))?;

    fs::rename(&temp_path, path)
        .with_context(|| format!("Failed to rename temp file to: {}", path.display()))?;

    Ok(())
}

/// Delete a note file.
/// If permanent is false, moves to .trash directory with timestamp.
/// If permanent is true, permanently deletes the file.
pub fn delete_note(root: &Path, path: &Path, permanent: bool) -> Result<String> {
    if !path.exists() {
        anyhow::bail!("File does not exist: {}", path.display());
    }

    if permanent {
        fs::remove_file(path)
            .with_context(|| format!("Failed to delete file: {}", path.display()))?;
        Ok(format!("Permanently deleted {}", path.display()))
    } else {
        // Move to .trash directory
        let trash_dir = root.join(".trash");
        fs::create_dir_all(&trash_dir).with_context(|| {
            format!("Failed to create trash directory: {}", trash_dir.display())
        })?;

        // Generate unique name with timestamp
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let file_name = path.file_name().unwrap_or_default().to_string_lossy();
        let trash_path = trash_dir.join(format!("{}_{}", timestamp, file_name));

        fs::rename(path, &trash_path)
            .with_context(|| format!("Failed to move file to trash: {}", path.display()))?;

        Ok(format!(
            "Moved to trash: {}",
            trash_path
                .strip_prefix(root)
                .unwrap_or(&trash_path)
                .display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_vault() -> TempDir {
        let dir = TempDir::new().unwrap();

        // Create a note with frontmatter
        let note1 = r#"---
title: Test Note
tags: [rust, mcp]
---

# Hello World

This is a test note about Gagagigo."#;
        fs::write(dir.path().join("test.md"), note1).unwrap();

        // Create a simple note
        let note2 = "# Simple Note\n\nNo frontmatter here.";
        fs::write(dir.path().join("simple.md"), note2).unwrap();

        // Create a subdirectory with a note
        fs::create_dir_all(dir.path().join("daily")).unwrap();
        let note3 = "# Daily Note\n\nGagagigo awakens!";
        fs::write(dir.path().join("daily/2024-01-01.md"), note3).unwrap();

        dir
    }

    #[test]
    fn test_read_note_without_frontmatter() {
        let vault = setup_test_vault();
        let result = read_note(&vault.path().join("simple.md"), false).unwrap();
        assert!(result.contains("# Simple Note"));
    }

    #[test]
    fn test_read_note_with_frontmatter_parsing() {
        let vault = setup_test_vault();
        let result = read_note(&vault.path().join("test.md"), true).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["metadata"]["title"], "Test Note");
        assert!(parsed["body"].as_str().unwrap().contains("Hello World"));
    }

    #[test]
    fn test_search_notes() {
        let vault = setup_test_vault();
        let results = search_notes(vault.path(), "Gagagigo", 10).unwrap();

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_notes_with_limit() {
        let vault = setup_test_vault();
        let results = search_notes(vault.path(), "Gagagigo", 1).unwrap();

        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_search_notes_regex() {
        let vault = setup_test_vault();
        let results = search_notes(vault.path(), r"#\s+\w+", 10).unwrap();

        // Should match headings
        assert!(!results.is_empty());
    }

    #[test]
    fn test_write_note_new() {
        let vault = setup_test_vault();
        let new_path = vault.path().join("new_note.md");

        write_note(&new_path, "# New Note\n\nContent here.").unwrap();

        assert!(new_path.exists());
        let content = fs::read_to_string(&new_path).unwrap();
        assert!(content.contains("# New Note"));
    }

    #[test]
    fn test_write_note_creates_directories() {
        let vault = setup_test_vault();
        let nested_path = vault.path().join("nested/dir/note.md");

        write_note(&nested_path, "# Nested Note").unwrap();

        assert!(nested_path.exists());
    }

    #[test]
    fn test_delete_note_to_trash() {
        let vault = setup_test_vault();
        let note_path = vault.path().join("simple.md");

        let result = delete_note(vault.path(), &note_path, false).unwrap();

        assert!(!note_path.exists());
        assert!(result.contains("Moved to trash"));
        assert!(vault.path().join(".trash").exists());
    }

    #[test]
    fn test_delete_note_permanent() {
        let vault = setup_test_vault();
        let note_path = vault.path().join("simple.md");

        let result = delete_note(vault.path(), &note_path, true).unwrap();

        assert!(!note_path.exists());
        assert!(result.contains("Permanently deleted"));
    }

    // ========================================
    // Boundary & Error Tests (0-1-N)
    // ========================================

    // --- read_note boundaries ---

    #[test]
    fn test_read_note_empty_file() {
        let vault = setup_test_vault();
        let empty_path = vault.path().join("empty.md");
        fs::write(&empty_path, "").unwrap();

        let result = read_note(&empty_path, false).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_read_note_not_found() {
        let vault = setup_test_vault();
        let result = read_note(&vault.path().join("nonexistent.md"), false);

        assert!(result.is_err());
    }

    #[test]
    fn test_read_note_frontmatter_only() {
        let vault = setup_test_vault();
        let path = vault.path().join("frontmatter_only.md");
        fs::write(&path, "---\ntitle: Only FM\n---\n").unwrap();

        let result = read_note(&path, true).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["metadata"]["title"], "Only FM");
        assert_eq!(parsed["body"], "");
    }

    #[test]
    fn test_read_note_invalid_yaml() {
        let vault = setup_test_vault();
        let path = vault.path().join("invalid_yaml.md");
        fs::write(&path, "---\n: invalid yaml [[\n---\n\nBody here").unwrap();

        // Should return raw content when YAML is invalid
        let result = read_note(&path, true).unwrap();
        assert!(result.contains(": invalid yaml"));
    }

    #[test]
    fn test_read_note_unclosed_frontmatter() {
        let vault = setup_test_vault();
        let path = vault.path().join("unclosed.md");
        fs::write(&path, "---\ntitle: Unclosed\n\nNo closing delimiter").unwrap();

        // Should return raw content when frontmatter is unclosed
        let result = read_note(&path, true).unwrap();
        assert!(result.contains("No closing delimiter"));
    }

    #[test]
    fn test_read_note_no_frontmatter_with_parse_flag() {
        let vault = setup_test_vault();
        let result = read_note(&vault.path().join("simple.md"), true).unwrap();

        // Should return raw content when no frontmatter exists
        assert!(result.contains("# Simple Note"));
    }

    // --- search_notes boundaries ---

    #[test]
    fn test_search_notes_empty_vault() {
        let dir = TempDir::new().unwrap();
        let results = search_notes(dir.path(), "anything", 10).unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn test_search_notes_no_matches() {
        let vault = setup_test_vault();
        let results = search_notes(vault.path(), "zzz_no_match_zzz", 10).unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn test_search_notes_invalid_regex() {
        let vault = setup_test_vault();
        let result = search_notes(vault.path(), "[invalid(regex", 10);

        assert!(result.is_err());
    }

    #[test]
    fn test_search_notes_limit_zero() {
        let vault = setup_test_vault();
        let results = search_notes(vault.path(), "Gagagigo", 0).unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn test_search_notes_skips_hidden_dirs() {
        let vault = setup_test_vault();

        // Create a hidden directory with a note
        fs::create_dir_all(vault.path().join(".obsidian")).unwrap();
        fs::write(
            vault.path().join(".obsidian/config.md"),
            "# Hidden Gagagigo",
        )
        .unwrap();

        let results = search_notes(vault.path(), "Hidden Gagagigo", 10).unwrap();

        // Should not find the hidden file
        assert!(results.is_empty());
    }

    // --- write_note boundaries ---

    #[test]
    fn test_write_note_empty_content() {
        let vault = setup_test_vault();
        let path = vault.path().join("empty_write.md");

        write_note(&path, "").unwrap();

        assert!(path.exists());
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }

    #[test]
    fn test_write_note_overwrite_existing() {
        let vault = setup_test_vault();
        let path = vault.path().join("simple.md");

        write_note(&path, "# Overwritten Content").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("Overwritten"));
        assert!(!content.contains("No frontmatter"));
    }

    #[test]
    fn test_write_note_unicode_content() {
        let vault = setup_test_vault();
        let path = vault.path().join("unicode.md");

        let content = "# ガガギゴ 🐉\n\n日本語テスト";
        write_note(&path, content).unwrap();

        let read_back = fs::read_to_string(&path).unwrap();
        assert_eq!(read_back, content);
    }

    // --- delete_note boundaries ---

    #[test]
    fn test_delete_note_not_found() {
        let vault = setup_test_vault();
        let result = delete_note(vault.path(), &vault.path().join("nonexistent.md"), false);

        assert!(result.is_err());
    }

    #[test]
    fn test_delete_note_in_subdirectory() {
        let vault = setup_test_vault();
        let note_path = vault.path().join("daily/2024-01-01.md");

        let result = delete_note(vault.path(), &note_path, false).unwrap();

        assert!(!note_path.exists());
        assert!(result.contains("Moved to trash"));
    }

    // --- format_with_frontmatter ---

    #[test]
    fn test_format_with_frontmatter() {
        let metadata = serde_json::json!({
            "title": "Test Note",
            "tags": ["rust", "mcp"]
        });
        let body = "# Hello\n\nThis is content.";

        let result = format_with_frontmatter(&metadata, body);

        assert!(result.starts_with("---\n"));
        assert!(result.contains("title: Test Note"));
        assert!(result.contains("tags:"));
        assert!(result.contains("---\n\n# Hello"));
    }

    #[test]
    fn test_format_with_frontmatter_empty_metadata() {
        let metadata = serde_json::json!({});
        let body = "Just body content";

        let result = format_with_frontmatter(&metadata, body);

        assert!(result.starts_with("---\n"));
        assert!(result.contains("---\n\nJust body content"));
    }

    #[test]
    fn test_format_with_frontmatter_string_metadata() {
        // AI sometimes passes metadata as JSON string instead of object
        let metadata = serde_json::json!(r#"{"title": "Test", "tags": ["a", "b"]}"#);
        let body = "Body";

        let result = format_with_frontmatter(&metadata, body);

        // Should parse the string and convert to YAML properly
        assert!(result.contains("title: Test"));
        assert!(result.contains("tags:"));
    }

    #[test]
    fn test_write_note_with_frontmatter_roundtrip() {
        let vault = setup_test_vault();
        let path = vault.path().join("roundtrip.md");

        let metadata = serde_json::json!({"title": "Roundtrip Test"});
        let body = "Body content here";
        let content = format_with_frontmatter(&metadata, body);

        write_note(&path, &content).unwrap();

        // Read back and parse
        let result = read_note(&path, true).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["metadata"]["title"], "Roundtrip Test");
        assert!(parsed["body"]
            .as_str()
            .unwrap()
            .contains("Body content here"));
    }

    #[test]
    fn test_format_with_frontmatter_special_chars() {
        let metadata = serde_json::json!({
            "title": "Note: Important!",
            "description": "Line1\nLine2",
            "path": "foo/bar#baz"
        });
        let body = "Content";

        let result = format_with_frontmatter(&metadata, body);

        // Should be valid YAML that can be parsed back
        assert!(result.contains("title:"));
        assert!(result.contains("description:"));

        // Verify roundtrip
        let vault = TempDir::new().unwrap();
        let path = vault.path().join("special.md");
        write_note(&path, &result).unwrap();

        let read_back = read_note(&path, true).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&read_back).unwrap();

        assert_eq!(parsed["metadata"]["title"], "Note: Important!");
        assert_eq!(parsed["metadata"]["description"], "Line1\nLine2");
    }

    #[test]
    fn test_format_roundtrip_preserves_types() {
        let vault = setup_test_vault();
        let path = vault.path().join("types.md");

        let metadata = serde_json::json!({
            "count": 42,
            "ratio": 3.14,
            "active": true,
            "tags": ["a", "b"]
        });
        let content = format_with_frontmatter(&metadata, "Body");

        write_note(&path, &content).unwrap();

        let result = read_note(&path, true).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // Verify types are preserved
        assert_eq!(parsed["metadata"]["count"], 42);
        assert_eq!(parsed["metadata"]["ratio"], 3.14);
        assert_eq!(parsed["metadata"]["active"], true);
        assert!(parsed["metadata"]["tags"].is_array());
    }

    #[test]
    fn test_format_with_frontmatter_nested_objects() {
        let vault = setup_test_vault();
        let path = vault.path().join("nested.md");

        let metadata = serde_json::json!({
            "author": {
                "name": "Gagagigo",
                "level": 4
            }
        });
        let content = format_with_frontmatter(&metadata, "Body");

        write_note(&path, &content).unwrap();

        let result = read_note(&path, true).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["metadata"]["author"]["name"], "Gagagigo");
        assert_eq!(parsed["metadata"]["author"]["level"], 4);
    }

    // --- search_metadata ---

    #[test]
    fn test_search_metadata_by_title() {
        let vault = setup_test_vault();
        let results = search_metadata(vault.path(), "title", "Test", 10).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, "Test Note");
    }

    #[test]
    fn test_search_metadata_by_tags() {
        let vault = setup_test_vault();
        let results = search_metadata(vault.path(), "tags", "rust", 10).unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].value.is_array());
    }

    #[test]
    fn test_search_metadata_no_match() {
        let vault = setup_test_vault();
        let results = search_metadata(vault.path(), "title", "NonExistent", 10).unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn test_search_metadata_nested_field() {
        let vault = setup_test_vault();
        let path = vault.path().join("nested_meta.md");
        let content = format_with_frontmatter(
            &serde_json::json!({"author": {"name": "Gagagigo", "level": 8}}),
            "Body",
        );
        write_note(&path, &content).unwrap();

        let results = search_metadata(vault.path(), "author.name", "Gagagigo", 10).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, "Gagagigo");
    }

    #[test]
    fn test_search_metadata_regex() {
        let vault = setup_test_vault();
        let results = search_metadata(vault.path(), "title", "^Test.*", 10).unwrap();

        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_search_metadata_missing_field() {
        let vault = setup_test_vault();
        let results = search_metadata(vault.path(), "nonexistent_field", ".*", 10).unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn test_search_metadata_limit() {
        let vault = setup_test_vault();

        // Create multiple notes with same tag
        for i in 0..5 {
            let path = vault.path().join(format!("tagged_{}.md", i));
            let content = format_with_frontmatter(
                &serde_json::json!({"tags": ["common"]}),
                &format!("Note {}", i),
            );
            write_note(&path, &content).unwrap();
        }

        let results = search_metadata(vault.path(), "tags", "common", 3).unwrap();

        assert_eq!(results.len(), 3);
    }
}
