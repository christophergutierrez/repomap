# Contributing

## Building

```bash
cargo build --release
```

The binary is at `target/release/repomap`.

## Running Tests

```bash
cargo test                      # all tests
cargo test -- --nocapture       # with stdout output
cargo test test_parse_fixture   # specific test
```

Tests are embedded in source files using `#[cfg(test)]` modules.  Key test
locations:

| File | What's tested |
|---|---|
| `rust/src/parser/extractor.rs` | Symbol extraction for all languages |
| `rust/src/storage.rs` | SQLite operations, incremental indexing, FTS5 |
| `rust/src/discovery.rs` | File discovery, filtering, security checks |
| `rust/src/summarizer.rs` | Three-tier summarization pipeline |

Test fixtures live in `tests/fixtures/{language}/sample.{ext}`.  Each sample
file contains representative code for that language (classes, functions,
methods, constants) so the parser tests can verify extraction.

---

## Project Layout

```
rust/src/
├── main.rs             # Entry point: CLI args → index or MCP server
├── mcp.rs              # JSON-RPC 2.0 protocol: read stdin, dispatch tools, write stdout
├── tools.rs            # All 14 MCP tool implementations
├── storage.rs          # SQLite index store: save, query, incremental update
├── graph.rs            # Knowledge graph queries (DEFINES, CONTAINS, REFERENCES)
├── discovery.rs        # Filesystem walk + filtering (extensions, secrets, binaries)
├── summarizer.rs       # Docstring → AI → signature fallback summarization
├── config.rs           # Environment variable configuration
└── parser/
    ├── mod.rs          # Orchestrator: iterate files → parse_file()
    ├── extractor.rs    # tree-sitter AST walk → Symbol extraction
    ├── symbols.rs      # Symbol struct + ID generation
    ├── languages.rs    # Per-language specs (13 languages)
    ├── imports.rs      # Import path extraction
    ├── hierarchy.rs    # Parent-child symbol resolution
    └── proto_refs.rs   # Protobuf field type references
```

---

## Adding a New Language

### 1. Add the tree-sitter grammar dependency

In `Cargo.toml`:

```toml
tree-sitter-newlang = "0.X"
```

### 2. Register the file extension

In `rust/src/parser/languages.rs`, add to `language_for_extension()`:

```rust
"nl" => Some("newlang"),
```

### 3. Register the tree-sitter language

In the same file, add to `ts_language_for()`:

```rust
"newlang" => Some(tree_sitter_newlang::LANGUAGE.into()),
```

### 4. Define the language spec

Add an entry to the `LANGUAGE_REGISTRY` in the same file.  This tells the
extractor which AST node types to extract and how to find names, parameters,
return types, and docstrings.

```rust
m.insert("newlang", LanguageSpec {
    ts_language: "newlang",
    symbol_node_types: hm(&[
        ("function_declaration", "function"),
        ("class_declaration", "class"),
        ("method_definition", "method"),
    ]),
    name_fields: hm(&[
        ("function_declaration", "name"),
        ("class_declaration", "name"),
        ("method_definition", "name"),
    ]),
    param_fields: hm(&[
        ("function_declaration", "parameters"),
    ]),
    return_type_fields: hm(&[
        ("function_declaration", "return_type"),
    ]),
    docstring_strategy: "preceding_comment",
    decorator_node_type: None,
    container_node_types: vec!["class_declaration"],
    constant_patterns: vec![],
    type_patterns: vec![],
    decorator_from_children: false,
});
```

**Key fields:**

| Field | Purpose |
|---|---|
| `symbol_node_types` | Maps AST node type → symbol kind.  Only these nodes become symbols. |
| `name_fields` | Which child field holds the symbol's name for each node type. |
| `param_fields` | Where to find function parameters (for signatures). |
| `return_type_fields` | Where to find return type annotations. |
| `docstring_strategy` | `"preceding_comment"` (most languages) or `"next_sibling_string"` (Python). |
| `decorator_node_type` | AST node type for decorators/attributes, if the language has them. |
| `container_node_types` | Node types that can contain other symbols (classes, impl blocks). |
| `constant_patterns` | Top-level assignment patterns to extract as constants. |
| `type_patterns` | Node types that represent type definitions. |

**Tip:** Use `tree-sitter parse` on a sample file to see the AST node types
for your language.  The node type names in the grammar are what you put in
`symbol_node_types`.

### 5. Add the extension to discovery

In `rust/src/discovery.rs`, add the file extension to `INDEXABLE_EXTENSIONS`:

```rust
"nl",
```

### 6. Create a test fixture

Create `tests/fixtures/newlang/sample.nl` with representative code covering
the symbol types you registered (functions, classes, methods, etc.).

Then add it to the `test_parse_all_supported_fixtures` test in
`rust/src/parser/extractor.rs`:

```rust
("tests/fixtures/newlang/sample.nl", "newlang"),
```

### 7. Verify

```bash
cargo test test_parse_all_supported_fixtures
```

---

## Adding a New MCP Tool

### 1. Define the tool schema

In `rust/src/mcp.rs`, add a JSON schema to the `tool_definitions()` function:

```rust
json!({
    "name": "my_new_tool",
    "description": "What this tool does — one sentence for the AI to understand when to use it.",
    "inputSchema": {
        "type": "object",
        "properties": {
            "repo": { "type": "string", "description": "Repository identifier" },
            "my_param": { "type": "string", "description": "What this param controls" }
        },
        "required": ["repo", "my_param"]
    }
})
```

### 2. Add the dispatch

In the same file, add a match arm in `handle_call_tool()`:

```rust
"my_new_tool" => {
    let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("");
    let my_param = args.get("my_param").and_then(|v| v.as_str()).unwrap_or("");
    tools::my_new_tool(repo, my_param, store)
}
```

### 3. Implement the tool

In `rust/src/tools.rs`:

```rust
pub fn my_new_tool(repo: &str, my_param: &str, store: &IndexStore) -> Value {
    let start = Instant::now();

    let (owner, name) = match resolve_repo(repo, store) {
        Ok(r) => r,
        Err(e) => return e,
    };

    // Your logic here — query the database, process results

    json!({
        "repo": format!("{owner}/{name}"),
        "result": "...",
        "_meta": { "timing_ms": elapsed_ms(start) }
    })
}
```

**Conventions:**
- Always include `_meta.timing_ms` in the response.
- Use `resolve_repo()` to handle both `owner/repo` and bare repo name inputs.
- Return errors as `json!({"error": "message"})`.
- For async tools (network calls), use `pub async fn` and `.await` in the
  dispatch.

---

## Code Style

- No external formatter enforced — match the style of surrounding code.
- All public functions need a brief doc comment (`///`).
- Prefer returning `serde_json::Value` from tool functions.
- Use `anyhow::Result` for fallible internal functions.
- String truncation must use `floor_char_boundary()` to avoid panics on
  multi-byte UTF-8 characters.
