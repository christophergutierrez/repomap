# Architecture

repomap is a Rust MCP server that indexes source code repositories into a
SQLite-backed symbol store, then exposes query tools over the Model Context
Protocol (MCP).  AI assistants use these tools to navigate codebases at the
symbol level — retrieving only what they need instead of loading entire files.

---

## Key Concepts

**Symbol** — A named code element extracted from source: a function, class,
method, struct, type, constant, enum, interface, or protobuf message.  Each
symbol has a unique ID, its source location (file, line, byte offset), a
signature (the declaration line), and optionally a docstring and AI-generated
summary.  Symbols are the fundamental unit repomap operates on — indexing
produces them, queries return them.

**tree-sitter** — A parser generator that produces concrete syntax trees (CSTs)
from source code.  Unlike regex-based extraction, tree-sitter understands the
actual grammar of each language, so it correctly handles nested structures,
multi-line signatures, and edge cases.  repomap uses tree-sitter grammars for
all 13 supported languages.

**MCP (Model Context Protocol)** — A protocol that lets AI assistants call
external tools.  repomap runs as an MCP server over stdin/stdout using
JSON-RPC 2.0.  Claude Code connects to it automatically and can call any of
the 14 exposed tools.

**FTS5 (Full-Text Search 5)** — SQLite's built-in full-text search engine.
repomap maintains an FTS5 virtual table that mirrors symbol names, signatures,
summaries, and docstrings.  This enables ranked text search across all symbols
without scanning every row — queries like "find all symbols matching
'authenticate'" return in milliseconds even on large indexes.

**Byte-offset retrieval** — Instead of re-parsing a file every time a symbol's
source is requested, repomap stores each symbol's exact byte position and
length in the original file.  Retrieving a symbol is then O(1): one database
lookup for the offset, one file seek, done.  This is what makes `get_symbol`
fast regardless of file size.

**Knowledge graph** — A set of typed relationships between files and symbols,
stored in SQLite tables.  Four edge types exist: DEFINES (file → symbol),
CONTAINS (parent symbol → child symbol), REFERENCES (protobuf field → type),
and IMPLEMENTS (type → base type/trait/interface).  These enable queries like
"what symbols does this file define?", "what messages reference this type?",
or "what classes implement this interface?" without scanning source code.

**Index** — The SQLite database plus raw source files stored on disk for a
given repository.  Once built, the index persists across sessions — all query
tools read from it with no re-parsing.  Incremental re-indexing updates only
changed files by comparing SHA-256 hashes.

---

## System Overview

```mermaid
graph TB
    subgraph "Claude Code"
        CC[AI Assistant]
    end

    subgraph "repomap (MCP Server)"
        MCP["MCP Protocol Handler<br/>(stdio JSON-RPC 2.0)"]
        Tools["Tool Dispatcher<br/>(14 tools)"]

        subgraph "Indexing Pipeline"
            Disc[Discovery]
            Parser[Parser<br/>tree-sitter]
            Summ[Summarizer<br/>3-tier]
        end

        subgraph "Query Engine"
            FTS[FTS5 Search]
            Graph[Graph Queries]
            ByteO["Byte-Offset<br/>Retrieval"]
        end
    end

    subgraph "Storage (~/.code-index/)"
        DB["SQLite DB<br/>per repo"]
        Raw["Raw Source Files"]
    end

    CC <-->|"JSON-RPC / stdio"| MCP
    MCP --> Tools
    Tools --> FTS
    Tools --> Graph
    Tools --> ByteO
    Tools --> Disc
    Disc --> Parser
    Parser --> Summ
    Summ --> DB
    FTS --> DB
    Graph --> DB
    ByteO --> Raw
    Parser --> Raw
```

---

## Indexing Pipeline

When a repository is indexed, files flow through four stages: discovery,
parsing, summarization, and storage.

```mermaid
sequenceDiagram
    participant User
    participant CLI as CLI / MCP Tool
    participant Disc as Discovery
    participant Parser as Parser
    participant Summ as Summarizer
    participant Store as Storage

    User->>CLI: index_repo("/path/to/repo")
    CLI->>Disc: discover_files(path)
    Note over Disc: Walk filesystem<br/>Filter by extension, size<br/>Skip secrets, binaries<br/>Respect .gitignore
    Disc-->>CLI: Vec<PathBuf>

    CLI->>Parser: parse_files(files)
    loop Each source file
        Parser->>Parser: Detect language from extension
        Parser->>Parser: tree-sitter parse → AST
        Parser->>Parser: Walk AST, extract symbols
    end
    Parser-->>CLI: Vec<Symbol>

    CLI->>Summ: summarize_symbols(symbols)
    Note over Summ: Tier 1: Docstring extraction<br/>Tier 2: AI batch (optional)<br/>Tier 3: Signature fallback
    Summ-->>CLI: Symbols with summaries

    CLI->>Store: save_index(symbols, files)
    Note over Store: Write SQLite (.db.tmp)<br/>Build FTS5 index<br/>Save raw files to disk<br/>Atomic rename → .db
    Store-->>CLI: Success
    CLI-->>User: 784 symbols, 60 files, 3.6s
```

### Incremental Re-indexing

After the initial index, only changed files are re-processed:

```mermaid
sequenceDiagram
    participant CLI
    participant Store as Storage
    participant Parser

    CLI->>Store: detect_changes(current_files)
    Note over Store: SHA-256 hash each file<br/>Diff against stored hashes
    Store-->>CLI: changed[], new[], deleted[]

    CLI->>Parser: parse_files(changed + new)
    Parser-->>CLI: Vec<Symbol>

    CLI->>Store: incremental_save()
    Note over Store: DELETE old symbols for changed/deleted<br/>INSERT new symbols<br/>UPDATE repo_meta + hashes
```

---

## Storage Layout

All indexes are stored under `~/.code-index/` (override with `CODE_INDEX_PATH`).

```
~/.code-index/
├── owner-reponame.db          # SQLite database (symbols, metadata, FTS5)
├── owner-reponame/             # Raw source files (for byte-offset retrieval)
│   ├── src/main.rs
│   ├── src/lib.rs
│   └── ...
├── local-anotherrepo.db        # Repos without git remotes use "local" owner
└── local-anotherrepo/
```

When a repo has a git remote, owner and repo name are extracted from the URL
(`git@github.com:owner/repo.git` → `owner/repo`).  Without a remote, it falls
back to `local/<dirname>`.

### SQLite Schema

Each `.db` file contains six tables:

```mermaid
erDiagram
    repo_meta {
        text repo PK "owner/name"
        text owner
        text name
        text indexed_at "ISO-8601"
        int index_version "Current: 4"
        text git_head "Last commit hash"
        text source_files "JSON array"
        text languages "JSON map"
        text file_hashes "JSON map (SHA-256)"
    }

    symbols {
        text id PK "file::QualifiedName#kind"
        text file "Relative path"
        text name
        text qualified_name "Class.method"
        text kind "function|class|method|constant|type"
        text language
        text signature "Full declaration"
        text docstring
        text summary "AI-generated"
        text parent FK "Parent symbol ID"
        int line "1-indexed"
        int end_line
        int byte_offset "Position in raw file"
        int byte_length "Byte count"
        text content_hash "SHA-256"
    }

    symbols_fts {
        text id
        text name
        text signature
        text summary
        text docstring
    }

    proto_refs {
        text from_symbol_id FK
        text to_type_name
        text field_name
    }

    imports {
        text from_file
        text import_path
    }

    impl_refs {
        text from_symbol_id FK "Implementing type"
        text to_type_name "Base type / trait / interface"
        text kind "extends|implements|trait_impl"
    }

    repo_meta ||--o{ symbols : "indexes"
    symbols ||--o| symbols : "parent"
    symbols ||--o{ proto_refs : "references"
    symbols ||--o{ impl_refs : "implements"
```

**Symbol IDs** follow the format `file_path::QualifiedName#kind`.  For example:
- `src/main.rs::main#function`
- `src/parser/extractor.rs::walk_tree#function`
- `models.py::User.save#method`

Overloaded symbols get a `~N` suffix: `handler.go::validate~1#function`.

---

## Symbol Retrieval (O(1))

The key performance feature: retrieving a symbol's full source code requires
no re-parsing.  The index stores the byte offset and length into the raw file.

```mermaid
sequenceDiagram
    participant Client as AI Assistant
    participant Tools
    participant DB as SQLite
    participant Disk as Raw Files

    Client->>Tools: get_symbol("owner/repo", "src/lib.rs::Config#struct")
    Tools->>DB: SELECT byte_offset, byte_length, file<br/>FROM symbols WHERE id = ?
    DB-->>Tools: file="src/lib.rs", offset=1842, length=356
    Tools->>Disk: Read ~/.code-index/owner-repo/src/lib.rs
    Tools->>Tools: Extract bytes [1842..2198]
    Tools-->>Client: Full struct source code + metadata
```

---

## Knowledge Graph

repomap builds a lightweight knowledge graph from four relationship types
stored directly in SQLite (no external graph database required).

```mermaid
graph LR
    subgraph "DEFINES (file → symbol)"
        F1["src/server.rs"] -->|defines| S1["Server#struct"]
        F1 -->|defines| S2["Server.start#method"]
        F1 -->|defines| S3["handle_request#function"]
    end

    subgraph "CONTAINS (parent → child)"
        S1 -->|contains| S2
    end

    subgraph "REFERENCES (proto field → type)"
        M1["CreateRequest#message"] -->|references| M2["User#message"]
        M1 -->|references| M3["Role#enum"]
    end

    subgraph "IMPLEMENTS (type → base/trait/interface)"
        C1["SqlRepository#class"] -->|implements| I1["IRepository#type"]
        C2["User#type"] -->|trait_impl| T1["Display#type"]
        C3["UserService#class"] -->|extends| B1["BaseService#class"]
    end
```

### DEFINES

Every symbol has a `file` column linking it to its source file.  This
relationship answers "what symbols does this file define?"

```
Query: SELECT * FROM symbols WHERE file = 'src/parser/extractor.rs'
→ All functions, structs, types defined in that file
```

### CONTAINS

Symbols with a `parent` column form a hierarchy.  Methods belong to classes,
nested types belong to modules.

```
Query: SELECT * FROM symbols WHERE parent = 'src/models.py::User#class'
→ All methods and attributes of the User class
```

### REFERENCES

The `proto_refs` table tracks protobuf field-level type references.  When a
message has a field of type `User`, that creates a reference edge.

```
Query: find_dependents("myproto.proto::User#message")
→ All messages with fields that reference the User type
```

### IMPLEMENTS

The `impl_refs` table tracks explicit implementation and inheritance
relationships.  This works with languages that use explicit syntax:

| Language | Syntax | Edge kind |
|---|---|---|
| Rust | `impl Trait for Type` | `trait_impl` |
| Java | `class Foo extends Bar implements Baz` | `extends` / `implements` |
| C# | `class Foo : IBar, Baz` | `implements` / `extends` |
| TypeScript | `class Foo extends Bar implements IBaz` | `extends` / `implements` |
| Python | `class Foo(Bar, Baz):` | `extends` |
| PHP | `class Foo implements Bar { use Baz; }` | `implements` / `trait_impl` |
| Dart | `class Foo extends Bar implements Baz` | `extends` / `implements` |
| JavaScript | `class Foo extends Bar` | `extends` |

Languages with implicit interface satisfaction (like Go's structural typing)
are not supported by `find_implementations` — use `find_dependents` for
proto-level cross-references instead.

```
Query: find_implementations("src/models.rs::Authenticatable#type")
→ All types that implement the Authenticatable trait
```

### Graph Query Examples

The `graph_query` tool accepts relationship-type queries:

| Query | What it returns |
|---|---|
| `DEFINES src/main.rs` | All symbols defined in main.rs |
| `CONTAINS src/lib.rs::Server#struct` | All children (methods) of Server |
| `REFERENCES User` | All symbols referencing the User type |
| `IMPLEMENTS Authenticatable` | All types implementing Authenticatable |

Unrecognized queries return an error suggesting the appropriate structured tool.

All queries support an optional `format: "mermaid"` parameter that returns a
renderable Mermaid diagram instead of raw rows — useful for visualizing
inheritance trees, class structure, or file contents.

### Why the Graph Matters

To see the practical difference, consider a real question an AI assistant
might need to answer: **"What types implement or extend something in this
codebase?"**

**With the graph — 1 tool call, 0.3ms:**

```
graph_query("IMPLEMENTS")
→ 14 precise edges, structured data:

  rust/sample.rs::User          --[trait_impl]--> Authenticatable
  rust/sample.rs::User          --[trait_impl]--> std::fmt::Display
  csharp/sample.cs::SqlRepository --[implements]--> IRepository
  java/Sample.java::Sample      --[extends]----> BaseService
  java/Sample.java::Sample      --[implements]--> Serializable
  java/Sample.java::Sample      --[implements]--> Repository
  typescript/sample.ts::UserService --[extends]--> BaseService
  typescript/sample.ts::UserService --[implements]--> Searchable
  python/sample.py::UserService --[extends]----> BaseService
  php/sample.php::UserService   --[implements]--> Authenticatable
  php/sample.php::UserService   --[trait_impl]--> Timestampable
  dart/sample.dart::UserService --[extends]----> BaseService
  javascript/sample.js::UserService --[extends]--> BaseService
  ...
```

Every relationship, every language, with the edge kind — in one call.

**Without the graph — text search, multiple calls, incomplete:**

An AI assistant without graph edges would need to:

1. `search_symbols("extends implements")` — returns 11 results, but 9 are
   false positives (functions in the extraction code whose *names* contain
   "extends" or "implements").  Only 2 actual classes found.
2. `search_symbols("impl")` — returns 0 results (Rust's `impl` isn't a
   symbol name, it's syntax).
3. For each candidate, call `get_symbol()` to read the source and manually
   parse the signature — another 2–11 tool calls.
4. Still misses: Rust trait impls (`impl Display for User`), Python
   inheritance (`class Foo(Bar):`), PHP `implements`/`use`, C# `: IRepository`,
   Dart `extends` — because none of these put "extends" or "implements" in
   the symbol's searchable text.

**Result:** 2 of 14 relationships found, mixed with 9 false positives, after
3–13 tool calls.

**Impact on AI assistant efficiency:**

| Metric | With graph | Without graph |
|---|---|---|
| Tool calls | 1 | 3–13 |
| Wall time | < 1ms | 5–50ms |
| Tokens consumed | ~200 (structured JSON) | ~3,000–8,000 (reading source, filtering noise) |
| Accuracy | 14/14 relationships | 2/14 found, 9 false positives |

The token savings matter most.  Each unnecessary `get_symbol` call returns
50–200 lines of source code that the assistant must read, understand, and
discard.  In a real codebase with hundreds of classes, the without-graph
approach could easily consume **10,000–30,000 tokens** just to answer one
inheritance question — tokens that count against the context window and slow
down reasoning.  The graph answers the same question in ~200 tokens of
structured data, leaving the context window free for actual work.

This compounds across a session.  An assistant refactoring a class hierarchy
might ask this question 5–10 times as it traces relationships.  The graph
saves **50,000–200,000 tokens per session** on inheritance queries alone,
which is the difference between fitting the task in one conversation and
running out of context halfway through.

---

## Search Architecture

### FTS5 Full-Text Search

Symbol search uses SQLite's FTS5 extension with BM25 ranking, plus a custom
scoring layer tuned for code navigation:

```mermaid
flowchart LR
    Q["search_symbols('authenticate')"] --> FTS5["FTS5 BM25<br/>Initial ranking"]
    FTS5 --> Score["Custom Scorer"]

    subgraph "Scoring Weights"
        Score --> N["Exact name match: +20"]
        Score --> NC["Name contains: +10"]
        Score --> NW["Word in name: +5"]
        Score --> SW["Word in signature: +2"]
        Score --> SU["Word in summary: +1"]
    end

    Score --> Sort["Sort by score desc"]
    Sort --> R["Top N results"]
```

### Text Search

`search_text` searches raw file contents line-by-line — useful for strings,
comments, config values, and anything that isn't a symbol name.

---

## Parser Architecture

repomap uses [tree-sitter](https://tree-sitter.github.io/tree-sitter/) for
all language parsing.  Each language has a `LanguageSpec` that maps AST node
types to symbol extraction rules.

```mermaid
flowchart TB
    File["Source File"] --> Ext["Detect Language<br/>(file extension)"]
    Ext --> Spec["Load LanguageSpec"]
    Spec --> TS["tree-sitter Parse<br/>→ AST"]
    TS --> Walk["Recursive DFS Walk"]

    Walk --> Check{"Node type in<br/>symbol_node_types?"}
    Check -->|Yes| Extract["Extract Symbol<br/>(name, signature, docstring,<br/>decorators, byte offsets)"]
    Check -->|No| Skip["Skip / recurse children"]
    Extract --> Dedup["Disambiguate Overloads<br/>(append ~N)"]
    Skip --> Walk
    Dedup --> Out["Vec<Symbol>"]
```

### Supported Languages (13)

| Language | Symbol Types Extracted |
|---|---|
| Python | functions, classes, methods, constants |
| TypeScript | functions, classes, methods, interfaces, type aliases, enums |
| JavaScript | functions, classes, methods, arrow functions, generators |
| Go | functions, methods, type declarations |
| Rust | functions, structs, enums, traits, impl blocks, type aliases |
| Java | methods, constructors, classes, interfaces, enums |
| PHP | functions, classes, methods, interfaces, traits, enums |
| Dart | functions, classes, mixins, enums, extensions, methods, type aliases |
| C# | classes, records, interfaces, enums, structs, delegates, methods, constructors |
| C | functions, structs, enums, unions, typedefs |
| Lua | functions |
| Protobuf | messages, enums, services, RPCs |
| SQL | CREATE TABLE, CREATE VIEW, CREATE INDEX |

---

## Summarization Pipeline

Summaries are generated in three tiers, from cheapest to most expensive:

```mermaid
flowchart TB
    S["Symbol without summary"]
    S --> T1{"Has docstring?"}
    T1 -->|Yes| D["Tier 1: Extract first sentence<br/>(free, no API call)"]
    T1 -->|No| T2{"AI provider<br/>configured?"}
    T2 -->|Yes| AI["Tier 2: AI batch summarization<br/>(10 symbols per request)"]
    T2 -->|No| T3["Tier 3: Signature fallback<br/>'Class ClassName' / truncated sig"]
    AI -->|Failed/empty| T3

    subgraph "AI Provider Priority"
        P1["ANTHROPIC_API_KEY → Claude Haiku"]
        P2["GOOGLE_API_KEY → Gemini Flash"]
        P3["OPENAI_API_BASE → Local LLM"]
        P1 -.->|fallback| P2
        P2 -.->|fallback| P3
    end
```

---

## File Discovery

Discovery walks the filesystem with multiple filter layers to find indexable
source files while excluding noise and secrets.

```mermaid
flowchart TB
    Root["Repo Root"] --> Walk["Walk filesystem<br/>(respects .gitignore)"]
    Walk --> F1{"Skipped directory?<br/>node_modules, vendor,<br/>__pycache__, .git, target..."}
    F1 -->|Yes| Skip1[Skip]
    F1 -->|No| F2{"Known extension?<br/>py, ts, go, rs, java,<br/>c, proto, lua, sql..."}
    F2 -->|No| Skip2[Skip]
    F2 -->|Yes| F3{"Secret file?<br/>.env, *.pem, *.key,<br/>credentials.json..."}
    F3 -->|Yes| Skip3[Skip]
    F3 -->|No| F4{"Binary extension?<br/>exe, dll, png, pdf,<br/>wasm, pyc..."}
    F4 -->|Yes| Skip4[Skip]
    F4 -->|No| F5{"Size < 500KB?"}
    F5 -->|No| Skip5[Skip]
    F5 -->|Yes| F6{"Symlink escapes<br/>repo root?"}
    F6 -->|Yes| Skip6[Skip]
    F6 -->|No| Accept["Include file"]
```

---

## MCP Protocol

repomap communicates over stdin/stdout using JSON-RPC 2.0, the transport
defined by the Model Context Protocol.

```mermaid
sequenceDiagram
    participant Client as Claude Code
    participant Server as repomap

    Client->>Server: initialize (protocol version, capabilities)
    Server-->>Client: server info, tool capabilities

    Client->>Server: tools/list
    Server-->>Client: 14 tool definitions (name, schema)

    Client->>Server: tools/call { name: "search_symbols", args: {...} }
    Server-->>Client: { content: [{ text: "..." }] }

    Note over Client,Server: Repeat tool calls as needed

    Client->>Server: EOF (close stdin)
    Note over Server: Server exits
```

---

## Project Layout

```
repomap/
├── Cargo.toml                  # Rust project config + dependencies
├── Cargo.lock
├── README.md
├── docs/
│   └── ARCHITECTURE.md         # This file
├── rust/
│   └── src/
│       ├── main.rs             # Entry point: CLI + MCP server
│       ├── mcp.rs              # JSON-RPC protocol handler
│       ├── tools.rs            # 14 MCP tool implementations
│       ├── storage.rs          # SQLite index store
│       ├── graph.rs            # Knowledge graph queries
│       ├── discovery.rs        # File discovery + filtering
│       ├── summarizer.rs       # 3-tier AI summarization
│       ├── config.rs           # Environment-based configuration
│       ├── hooks.rs            # Git hook installation/removal
│       ├── stats.rs            # Usage statistics + token savings tracking
│       └── parser/
│           ├── mod.rs           # Parse orchestrator
│           ├── extractor.rs     # AST walker + symbol extraction
│           ├── symbols.rs       # Symbol data structure
│           ├── languages.rs     # Per-language extraction rules (13 langs)
│           ├── imports.rs       # Import path extraction
│           ├── impl_refs.rs     # Implementation/inheritance extraction
│           └── proto_refs.rs    # Protobuf field references
└── tests/
    └── fixtures/               # Sample files for each language
```
