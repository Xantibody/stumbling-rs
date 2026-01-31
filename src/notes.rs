use anyhow::{Context, Result};
use ignore::WalkBuilder;
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

/// Read a note from the given path.
/// If `parse_frontmatter` is true, separates YAML frontmatter from body.
pub fn read_note(path: &Path, parse_frontmatter: bool) -> Result<String> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {}", path.display()))?;

    if !parse_frontmatter {
        return Ok(content);
    }

    // Parse frontmatter if present
    if let Some(rest) = content.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            let frontmatter = rest[..end].trim();
            let body = rest[end + 4..].trim_start();

            // Parse YAML frontmatter
            if let Ok(meta) = serde_yaml_ng::from_str::<serde_json::Value>(frontmatter) {
                let output = serde_json::json!({
                    "metadata": meta,
                    "body": body
                });
                return Ok(serde_json::to_string_pretty(&output)?);
            }
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
                    let mut results = results.lock().unwrap();
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

    Ok(results.into_inner().unwrap())
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
}
