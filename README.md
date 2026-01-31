# stumbling-rs

An MCP server for AI to quickly read, write, and search local Markdown directories (like Obsidian vaults).

## Name Origin

Named after **Gagagigo** from Yu-Gi-Oh! — a creature who stumbles through hardship yet ultimately awakens as a hero. This tool helps you dig up new insights from your stumbling thoughts.

## Features

- **Fast search**: Leverages Rust's parallel processing for instant retrieval from large note collections
- **Markdown-native operations**: Search, create, update, and replace notes
- **Frontmatter-aware**: Safely updates content while preserving YAML metadata

## Configuration

| Variable | Description |
|----------|-------------|
| `STUMBLING_ROOT` | Absolute path to your notes directory |
| `STUMBLING_PARSE_FRONTMATTER` | Set `true` to parse YAML frontmatter as structured data |

## MCP Tools

| Tool | Description |
|------|-------------|
| `list_files` | Browse directory structure |
| `search_notes` | Full-text search with keywords or regex |
| `read_note` | Read note content (with metadata separation) |
| `write_note` | Create or overwrite notes |
| `update_lines` | Precise line/section-level editing |
