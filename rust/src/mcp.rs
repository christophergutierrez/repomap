//! MCP server over stdin/stdout using JSON-RPC 2.0.

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::storage::IndexStore;
use crate::tools;

/// Run the MCP server over stdio.
pub async fn serve_stdio() -> Result<()> {
    let store = IndexStore::open_store(None)?;

    let stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut lines = stdin.lines();

    tracing::info!("MCP server ready on stdio");

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {"code": -32700, "message": format!("Parse error: {e}")}
                });
                send(&mut stdout, &resp).await?;
                continue;
            }
        };

        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("");

        let response = match method {
            "initialize" => handle_initialize(&id),
            "initialized" => continue, // notification, no response
            "tools/list" => handle_list_tools(&id),
            "tools/call" => {
                let params = request.get("params").cloned().unwrap_or(json!({}));
                handle_call_tool(&id, &params, &store).await
            }
            "notifications/cancelled" | "notifications/initialized" => continue,
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": format!("Method not found: {method}")}
            }),
        };

        send(&mut stdout, &response).await?;
    }

    Ok(())
}

async fn send(stdout: &mut tokio::io::Stdout, msg: &Value) -> Result<()> {
    let s = serde_json::to_string(msg)?;
    stdout.write_all(s.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
}

fn handle_initialize(id: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "repomap",
                "version": env!("CARGO_PKG_VERSION")
            }
        }
    })
}

fn handle_list_tools(id: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "tools": tool_definitions()
        }
    })
}

async fn handle_call_tool(id: &Value, params: &Value, store: &IndexStore) -> Value {
    let tool_name = params
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    let result = match tool_name {
        "list_repos" => tools::list_repos(store),
        "get_repo_outline" => {
            let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("");
            tools::get_repo_outline(repo, store)
        }
        "get_file_tree" => {
            let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("");
            let prefix = args.get("path_prefix").and_then(|v| v.as_str()).unwrap_or("");
            tools::get_file_tree(repo, prefix, store)
        }
        "get_file_outline" => {
            let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("");
            let file = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            tools::get_file_outline(repo, file, store)
        }
        "get_symbol" => {
            let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("");
            let sid = args.get("symbol_id").and_then(|v| v.as_str()).unwrap_or("");
            tools::get_symbol(repo, sid, store)
        }
        "get_symbols" => {
            let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("");
            let ids: Vec<String> = args
                .get("symbol_ids")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            tools::get_symbols(repo, &ids, store)
        }
        "search_symbols" => {
            let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("");
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let kind = args.get("kind").and_then(|v| v.as_str());
            let language = args.get("language").and_then(|v| v.as_str());
            let max = args.get("max_results").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            tools::search_symbols(repo, query, kind, language, max, store)
        }
        "search_text" => {
            let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("");
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let file_pattern = args.get("file_pattern").and_then(|v| v.as_str());
            let max = args.get("max_results").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
            tools::search_text(repo, query, file_pattern, max, store)
        }
        "invalidate_cache" => {
            let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("");
            tools::invalidate_cache(repo, store)
        }
        "index_folder" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            tools::index_folder(path, store).await
        }
        "index_repo" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let use_ai = args.get("use_ai").and_then(|v| v.as_bool()).unwrap_or(true);
            tools::index_repo(path, use_ai, store).await
        }
        "find_dependents" => {
            let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("");
            let sid = args.get("symbol_id").and_then(|v| v.as_str()).unwrap_or("");
            crate::graph::find_dependents(repo, sid, store)
        }
        "find_implementations" => {
            let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("");
            let sid = args.get("symbol_id").and_then(|v| v.as_str()).unwrap_or("");
            crate::graph::find_implementations(repo, sid, store)
        }
        "graph_query" => {
            let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("");
            let cypher = args.get("cypher").and_then(|v| v.as_str()).unwrap_or("");
            crate::graph::graph_query(repo, cypher, store)
        }
        _ => json!({"error": format!("Unknown tool: {tool_name}")}),
    };

    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{
                "type": "text",
                "text": serde_json::to_string_pretty(&result).unwrap_or_default()
            }]
        }
    })
}

fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "list_repos",
            "description": "List all indexed repositories.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "get_repo_outline",
            "description": "Get a high-level overview of an indexed repository: directories, languages, symbol kinds.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": {"type": "string", "description": "Repository identifier (owner/repo or just repo name)"}
                },
                "required": ["repo"]
            }
        }),
        json!({
            "name": "get_file_tree",
            "description": "Get the file tree of an indexed repository, optionally filtered by path prefix.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": {"type": "string", "description": "Repository identifier"},
                    "path_prefix": {"type": "string", "description": "Optional path prefix filter", "default": ""}
                },
                "required": ["repo"]
            }
        }),
        json!({
            "name": "get_file_outline",
            "description": "Get all symbols in a file with signatures and summaries.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": {"type": "string", "description": "Repository identifier"},
                    "file_path": {"type": "string", "description": "Path to file within the repository"}
                },
                "required": ["repo", "file_path"]
            }
        }),
        json!({
            "name": "get_symbol",
            "description": "Get the full source code of a specific symbol by ID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": {"type": "string", "description": "Repository identifier"},
                    "symbol_id": {"type": "string", "description": "Symbol ID from get_file_outline or search_symbols"}
                },
                "required": ["repo", "symbol_id"]
            }
        }),
        json!({
            "name": "get_symbols",
            "description": "Get full source code of multiple symbols in one call.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": {"type": "string", "description": "Repository identifier"},
                    "symbol_ids": {"type": "array", "items": {"type": "string"}, "description": "List of symbol IDs"}
                },
                "required": ["repo", "symbol_ids"]
            }
        }),
        json!({
            "name": "search_symbols",
            "description": "Search for symbols matching a query. Returns matches with signatures and summaries.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": {"type": "string", "description": "Repository identifier"},
                    "query": {"type": "string", "description": "Search query"},
                    "kind": {"type": "string", "enum": ["function", "class", "method", "constant", "type"]},
                    "language": {"type": "string"},
                    "max_results": {"type": "integer", "default": 10}
                },
                "required": ["repo", "query"]
            }
        }),
        json!({
            "name": "search_text",
            "description": "Full-text search across indexed file contents.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": {"type": "string", "description": "Repository identifier"},
                    "query": {"type": "string", "description": "Text to search for"},
                    "file_pattern": {"type": "string", "description": "Optional glob pattern"},
                    "max_results": {"type": "integer", "default": 20}
                },
                "required": ["repo", "query"]
            }
        }),
        json!({
            "name": "invalidate_cache",
            "description": "Delete the index for a repository, forcing a full re-index.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": {"type": "string", "description": "Repository identifier"}
                },
                "required": ["repo"]
            }
        }),
        json!({
            "name": "index_folder",
            "description": "Index a local directory for code search. Discovers files, parses symbols, and stores the index.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Absolute path to the directory to index"}
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "index_repo",
            "description": "Index a local git repository. Reads files from disk, parses symbols, and stores the index. Derives owner/repo from the git remote URL.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Absolute path to the local repository"},
                    "use_ai": {"type": "boolean", "description": "Use AI for symbol summaries (default true)", "default": true}
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "find_dependents",
            "description": "Find all symbols that reference a given symbol. For proto types, returns messages with fields of that type.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": {"type": "string", "description": "Repository identifier"},
                    "symbol_id": {"type": "string", "description": "Symbol ID to find dependents of"}
                },
                "required": ["repo", "symbol_id"]
            }
        }),
        json!({
            "name": "find_implementations",
            "description": "Find types that implement a trait, interface, or base class. Works with explicit-syntax languages (Rust, Java, C#, TypeScript, Python, PHP, Dart, JavaScript).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": {"type": "string", "description": "Repository identifier"},
                    "symbol_id": {"type": "string", "description": "Symbol ID to find implementations of"}
                },
                "required": ["repo", "symbol_id"]
            }
        }),
        json!({
            "name": "graph_query",
            "description": "Query relationships between symbols: DEFINES (file→symbol), CONTAINS (parent→child), REFERENCES (proto field refs), IMPLEMENTS (type→base/trait/interface). Mention the relationship type in your query, or pass raw SQL SELECT.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": {"type": "string", "description": "Repository identifier"},
                    "cypher": {"type": "string", "description": "Query mentioning DEFINES, CONTAINS, or REFERENCES, or a raw SQL SELECT"}
                },
                "required": ["repo", "cypher"]
            }
        }),
    ]
}
