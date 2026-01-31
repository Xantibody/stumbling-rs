# stumbling-rs

An MCP server for AI to quickly read, write, and search local Markdown directories (like Obsidian vaults).

## Name Origin

Named after **Gagagigo** from Yu-Gi-Oh! — a creature who stumbles through hardship yet ultimately awakens as a hero. This tool helps you dig up new insights from your stumbling thoughts.

## Features

- **Fast search**: Parallel regex search using Rayon
- **Frontmatter-aware**: Parses YAML metadata separately from body
- **Safe delete**: Moves to `.trash` by default (recoverable)
- **Atomic writes**: Prevents data corruption

## Claude Desktop Setup

Add to `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "stumbling-rs": {
      "command": "/path/to/stumbling-rs",
      "env": {
        "STUMBLING_ROOT": "/path/to/your/notes",
        "STUMBLING_PARSE_FRONTMATTER": "true"
      }
    }
  }
}
```

## Environment Variables

| Variable | Description |
|----------|-------------|
| `STUMBLING_ROOT` | Absolute path to your notes directory |
| `STUMBLING_PARSE_FRONTMATTER` | Set `true` to parse YAML frontmatter as structured data |

## MCP Tools

| Tool | Description |
|------|-------------|
| `read_note` | Read note content (with optional metadata separation) |
| `search_notes` | Regex search across all `.md` files |
| `write_note` | Create or overwrite notes (supports `metadata` param for frontmatter) |
| `delete_note` | Move to `.trash` or permanently delete |

## Build

```bash
cargo build --release
```

Binary will be at `target/release/stumbling-rs`.
