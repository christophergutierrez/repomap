# repomap

![repomap overview](repomap.jpg)

An MCP server that gives AI assistants surgical access to large codebases —
returning only the symbols and source they need instead of entire files.

---

## Why it exists

When an AI assistant needs to understand a function buried inside a 600-line
file, it has two choices: load the whole file (burning context tokens on code
it doesn't need), or have a smarter interface that can answer "show me just
this function."

This server provides that interface.  It parses source files with
[tree-sitter](https://tree-sitter.github.io/tree-sitter/) into a per-repo
SQLite symbol index, then exposes MCP tools that let Claude navigate
codebases at the symbol level:

- Browse the file tree
- Get a file's outline (all classes, functions, and methods with signatures)
- Search symbols by name, kind, or language
- Fetch the exact source of one symbol by ID
- Search raw file text when symbol search isn't enough
- Query cross-file relationships (proto field references, parent/child nesting)

---

## Supported languages

Python · TypeScript · JavaScript · Go · Rust · Java · PHP · Dart · C# · C · Lua · SQL · Protocol Buffers

---

## Installation

### Quick install

```sh
curl -fsSL https://raw.githubusercontent.com/christophergutierrez/repomap/main/install.sh | sh
```

### Homebrew

```sh
brew install christophergutierrez/repomap/repomap
```

### Cargo

```sh
cargo install --git https://github.com/christophergutierrez/repomap
```

### Build from source

```bash
cargo build --release
```

The binary is at `target/release/repomap`.

---

## Setup

### 1. Configure Claude Code

Add repomap to your user-level MCP config in `~/.claude.json`:

```json
{
  "mcpServers": {
    "repomap": {
      "type": "stdio",
      "command": "/path/to/repomap"
    }
  }
}
```

Replace `/path/to/repomap` with the absolute path to the built binary.

Start a new Claude Code session after saving.  The MCP tools appear automatically.

### 2. Build the initial index

```bash
repomap index /path/to/your/repo --no-ai
```

Or use the `index_repo` MCP tool from within Claude Code — it takes a local path and indexes from disk.

The index persists on disk (`~/.code-index/`) and survives across sessions.

Add `--no-ai` to skip AI-generated symbol summaries; omit it if you have an
`ANTHROPIC_API_KEY` set and want richer search results.

### 3. Set up the git hook for automatic updates

Add this to `.git/hooks/post-merge` in your repo (create the file if it
doesn't exist, and make it executable with `chmod +x`):

```sh
#!/bin/sh
REPO_ROOT="$(git rev-parse --show-toplevel)"
if command -v repomap >/dev/null 2>&1; then
  repomap index "$REPO_ROOT" --incremental --no-ai &
fi
```

After every `git pull`, this runs an incremental re-index in the background.
Only files changed by the pull are re-parsed.

### 4. Optional environment variables

| Variable | Purpose | Default |
|---|---|---|
| `ANTHROPIC_API_KEY` | AI symbol summaries | — |
| `GOOGLE_API_KEY` | Gemini Flash symbol summaries (fallback) | — |
| `CODE_INDEX_PATH` | Where indexes are stored | `~/.code-index/` |
| `REPOMAP_LOG_LEVEL` | `DEBUG` / `INFO` / `WARNING` / `ERROR` | `WARNING` |

---

## Available MCP tools

| Tool | What it does |
|---|---|
| `index_repo` | Index a local git repository by path |
| `index_folder` | Index a local directory |
| `list_repos` | List all indexed repos |
| `get_repo_outline` | High-level overview: directories, languages, symbol kinds |
| `get_file_tree` | Nested file tree, optionally filtered by path prefix |
| `get_file_outline` | All symbols in one file, hierarchically structured |
| `get_symbol` | Full source of a specific symbol by ID |
| `get_symbols` | Full source of multiple symbols in one call |
| `search_symbols` | Search by name/signature/summary; filterable by kind, language, file pattern |
| `search_text` | Raw text search across file contents |
| `find_dependents` | Find all symbols that reference a given symbol |
| `find_implementations` | Find symbols that implement a given symbol |
| `graph_query` | Execute a Cypher query on the relationship graph |
| `invalidate_cache` | Delete an index to force a full re-index |

---

## How indexing works

1. **Discover** — walks the directory, skips noise (vendor, generated, lock
   files, secrets, binaries), respects `.gitignore` and size limits.
2. **Parse** — runs each file through the tree-sitter grammar for its language.
3. **Extract** — walks the CST collecting functions, classes, methods, types,
   and constants along with their signatures, docstrings, byte offsets, and
   parent relationships.
4. **Summarize** — optionally calls an AI to generate a one-line summary for
   each symbol that lacks a docstring.
5. **Store** — writes everything to a SQLite database with FTS5 for fast text
   search and byte-offset symbol retrieval.

Symbol source retrieval is O(1): one SQL row lookup for the byte offset, then
a single file seek.

---

## Incremental re-indexing

After the first full index, re-indexing only re-parses files that changed.

```bash
repomap index /path/to/repo --incremental --no-ai
```

The server computes SHA-256 hashes of all current files, diffs them against
the stored hashes, and re-parses only the changed and new files.  Deleted
files have their symbols removed.

---

## Development

```bash
cargo build --release
cargo test
```
