//! Graph relationship queries using SQLite.
//!
//! Instead of kuzu, we store graph edges in SQLite tables and query them
//! with SQL. The three relationship types are:
//!   DEFINES  (file → symbol)  — derived from symbols.file
//!   CONTAINS (symbol → symbol) — derived from symbols.parent
//!   REFERENCES (symbol → symbol) — derived from proto_refs + symbol resolution

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::time::Instant;

use crate::storage::IndexStore;

/// Find symbols that reference the given symbol via REFERENCES edges.
///
/// Uses the proto_refs table to find messages that have fields of the target type.
pub fn find_dependents(repo: &str, symbol_id: &str, store: &IndexStore) -> Value {
    let start = Instant::now();

    let (owner, name) = match store.resolve_repo(repo) {
        Ok(r) => r,
        Err(e) => return json!({"error": e.to_string()}),
    };

    let db_path = match store.index_path_pub(&owner, &name) {
        Ok(p) if p.exists() => p,
        _ => return json!({"error": format!("Repository not indexed: {owner}/{name}")}),
    };

    let conn = match Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return json!({"error": e.to_string()}),
    };

    // Get the target symbol's name (unqualified) for proto_refs lookup.
    let target_name: Option<String> = conn
        .query_row("SELECT name FROM symbols WHERE id = ?", [symbol_id], |r| r.get(0))
        .ok();

    if target_name.is_none() {
        return json!({"error": format!("Symbol not found: {symbol_id}")});
    }
    let target_name = target_name.unwrap();

    // Method 1: Direct parent lookup (CONTAINS relationship reversed).
    // Find all symbols whose parent is this symbol.
    let mut results = Vec::new();

    // Method 2: Proto REFERENCES — find all symbols that reference this type name.
    let mut stmt = conn
        .prepare(
            "SELECT s.id, s.name, s.kind, s.language
             FROM proto_refs pr
             JOIN symbols s ON s.id = pr.from_symbol_id
             WHERE pr.to_type_name = ?
               AND pr.from_symbol_id != ?",
        )
        .unwrap();

    let rows = stmt
        .query_map(rusqlite::params![target_name, symbol_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .unwrap();

    for row in rows.flatten() {
        results.push(json!({
            "id": row.0,
            "name": row.1,
            "kind": row.2,
            "language": row.3,
        }));
    }

    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    json!({
        "repo": format!("{owner}/{name}"),
        "symbol_id": symbol_id,
        "results": results,
        "_meta": {"timing_ms": elapsed, "result_count": results.len()}
    })
}

/// Find symbols that implement a given symbol (trait, interface, base class).
///
/// Queries the impl_refs table for types that extend/implement the target.
/// Works with explicit-syntax languages: Rust (`impl Trait for Type`),
/// Java/C#/TypeScript (`extends`/`implements`), Python (class inheritance),
/// PHP (`implements`/`use`), Dart (`extends`/`implements`/`with`),
/// JavaScript (`extends`).
pub fn find_implementations(repo: &str, symbol_id: &str, store: &IndexStore) -> Value {
    let start = Instant::now();

    let (owner, name) = match store.resolve_repo(repo) {
        Ok(r) => r,
        Err(e) => return json!({"error": e.to_string()}),
    };

    let db_path = match store.index_path_pub(&owner, &name) {
        Ok(p) if p.exists() => p,
        _ => return json!({"error": format!("Repository not indexed: {owner}/{name}")}),
    };

    let conn = match Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return json!({"error": e.to_string()}),
    };

    // Get the target symbol's name for impl_refs lookup.
    let target_name: Option<String> = conn
        .query_row("SELECT name FROM symbols WHERE id = ?", [symbol_id], |r| r.get(0))
        .ok();

    if target_name.is_none() {
        return json!({"error": format!("Symbol not found: {symbol_id}")});
    }
    let target_name = target_name.unwrap();

    let mut results = Vec::new();

    // Query impl_refs: find all types that implement/extend this type name.
    let mut stmt = conn
        .prepare(
            "SELECT ir.from_symbol_id, ir.kind, s.name, s.kind, s.language, s.file
             FROM impl_refs ir
             JOIN symbols s ON s.id = ir.from_symbol_id
             WHERE ir.to_type_name = ?",
        )
        .unwrap();

    let rows = stmt
        .query_map(rusqlite::params![target_name], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })
        .unwrap();

    for row in rows.flatten() {
        results.push(json!({
            "id": row.0,
            "relationship": row.1,
            "name": row.2,
            "kind": row.3,
            "language": row.4,
            "file": row.5,
        }));
    }

    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    json!({
        "repo": format!("{owner}/{name}"),
        "symbol_id": symbol_id,
        "results": results,
        "_meta": {"timing_ms": elapsed, "result_count": results.len()}
    })
}

/// Execute a graph query.
///
/// Supports a subset of Cypher-like patterns by translating to SQL:
///   - DEFINES: File → Symbol relationships
///   - CONTAINS: Symbol → Symbol parent/child
///   - REFERENCES: Symbol → Symbol via proto_refs
///
/// For arbitrary Cypher, returns an error suggesting specific tools.
pub fn graph_query(repo: &str, cypher: &str, store: &IndexStore) -> Value {
    let start = Instant::now();

    let (owner, name) = match store.resolve_repo(repo) {
        Ok(r) => r,
        Err(e) => return json!({"error": e.to_string()}),
    };

    let db_path = match store.index_path_pub(&owner, &name) {
        Ok(p) if p.exists() => p,
        _ => return json!({"error": format!("Repository not indexed: {owner}/{name}")}),
    };

    let conn = match Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return json!({"error": e.to_string()}),
    };

    // Try to handle common Cypher patterns.
    let cypher_upper = cypher.to_uppercase();

    let rows = if cypher_upper.contains("REFERENCES") {
        query_references(&conn, cypher)
    } else if cypher_upper.contains("IMPLEMENTS") {
        query_implements(&conn, cypher)
    } else if cypher_upper.contains("CONTAINS") {
        query_contains(&conn, cypher)
    } else if cypher_upper.contains("DEFINES") {
        query_defines(&conn, cypher)
    } else {
        Err(anyhow::anyhow!(
            "Unrecognized query. Use keywords DEFINES, CONTAINS, REFERENCES, or IMPLEMENTS, \
             or use find_dependents / find_implementations for common lookups."
        ))
    };

    let elapsed = start.elapsed().as_secs_f64() * 1000.0;

    match rows {
        Ok(rows) => json!({
            "repo": format!("{owner}/{name}"),
            "cypher": cypher,
            "rows": rows,
            "row_count": rows.len(),
            "_meta": {"timing_ms": elapsed}
        }),
        Err(e) => json!({
            "error": format!("Query error: {e}. Use find_dependents or find_implementations for common queries."),
            "_meta": {"timing_ms": elapsed}
        }),
    }
}

// ---------------------------------------------------------------------------
// Query translators
// ---------------------------------------------------------------------------

fn query_references(conn: &Connection, cypher: &str) -> Result<Vec<Vec<Value>>> {
    // Extract optional filter: "REFERENCES User" → filter by to_type_name = "User"
    let filter = extract_arg(cypher, "REFERENCES");

    let (sql, params): (String, Vec<String>) = if let Some(ref name) = filter {
        (
            "SELECT s1.name, s1.kind, pr.field_name, pr.to_type_name, s2.id, s2.name
             FROM proto_refs pr
             JOIN symbols s1 ON s1.id = pr.from_symbol_id
             LEFT JOIN symbols s2 ON s2.name = pr.to_type_name AND s2.kind = 'type'
             WHERE pr.to_type_name = ?
             ORDER BY s1.name".to_string(),
            vec![name.clone()],
        )
    } else {
        (
            "SELECT s1.name, s1.kind, pr.field_name, pr.to_type_name, s2.id, s2.name
             FROM proto_refs pr
             JOIN symbols s1 ON s1.id = pr.from_symbol_id
             LEFT JOIN symbols s2 ON s2.name = pr.to_type_name AND s2.kind = 'type'
             ORDER BY s1.name".to_string(),
            vec![],
        )
    };

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = Vec::new();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
    let mut raw = stmt.query(param_refs.as_slice())?;
    while let Some(r) = raw.next()? {
        rows.push(vec![
            json!(r.get::<_, String>(0).unwrap_or_default()),
            json!(r.get::<_, String>(1).unwrap_or_default()),
            json!(r.get::<_, String>(2).unwrap_or_default()),
            json!(r.get::<_, String>(3).unwrap_or_default()),
            json!(r.get::<_, Option<String>>(4).unwrap_or_default()),
            json!(r.get::<_, Option<String>>(5).unwrap_or_default()),
        ]);
    }
    Ok(rows)
}

fn query_contains(conn: &Connection, cypher: &str) -> Result<Vec<Vec<Value>>> {
    // Extract optional filter: "CONTAINS src/lib.rs::Server#struct" → filter by parent ID
    let filter = extract_arg(cypher, "CONTAINS");

    let (sql, params): (String, Vec<String>) = if let Some(ref id) = filter {
        (
            "SELECT p.name AS parent_name, p.kind AS parent_kind,
                    c.name AS child_name, c.kind AS child_kind, c.file
             FROM symbols c
             JOIN symbols p ON p.id = c.parent
             WHERE p.id = ? OR p.name = ?
             ORDER BY p.name, c.name".to_string(),
            vec![id.clone(), id.clone()],
        )
    } else {
        (
            "SELECT p.name AS parent_name, p.kind AS parent_kind,
                    c.name AS child_name, c.kind AS child_kind, c.file
             FROM symbols c
             JOIN symbols p ON p.id = c.parent
             ORDER BY p.name, c.name".to_string(),
            vec![],
        )
    };

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = Vec::new();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
    let mut raw = stmt.query(param_refs.as_slice())?;
    while let Some(r) = raw.next()? {
        rows.push(vec![
            json!(r.get::<_, String>(0).unwrap_or_default()),
            json!(r.get::<_, String>(1).unwrap_or_default()),
            json!(r.get::<_, String>(2).unwrap_or_default()),
            json!(r.get::<_, String>(3).unwrap_or_default()),
            json!(r.get::<_, String>(4).unwrap_or_default()),
        ]);
    }
    Ok(rows)
}

fn query_defines(conn: &Connection, cypher: &str) -> Result<Vec<Vec<Value>>> {
    // Extract optional filter: "DEFINES src/main.rs" → filter by file path
    let filter = extract_arg(cypher, "DEFINES");

    let (sql, params): (String, Vec<String>) = if let Some(ref file) = filter {
        (
            "SELECT file, name, kind, language FROM symbols WHERE file = ? ORDER BY line".to_string(),
            vec![file.clone()],
        )
    } else {
        (
            "SELECT file, name, kind, language FROM symbols ORDER BY file, line".to_string(),
            vec![],
        )
    };

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = Vec::new();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
    let mut raw = stmt.query(param_refs.as_slice())?;
    while let Some(r) = raw.next()? {
        rows.push(vec![
            json!(r.get::<_, String>(0).unwrap_or_default()),
            json!(r.get::<_, String>(1).unwrap_or_default()),
            json!(r.get::<_, String>(2).unwrap_or_default()),
            json!(r.get::<_, String>(3).unwrap_or_default()),
        ]);
    }
    Ok(rows)
}

fn query_implements(conn: &Connection, cypher: &str) -> Result<Vec<Vec<Value>>> {
    let filter = extract_arg(cypher, "IMPLEMENTS");

    let (sql, params): (String, Vec<String>) = if let Some(ref name) = filter {
        (
            "SELECT s1.name, s1.kind, ir.kind, ir.to_type_name, s1.file
             FROM impl_refs ir
             JOIN symbols s1 ON s1.id = ir.from_symbol_id
             WHERE ir.to_type_name = ?
             ORDER BY s1.name".to_string(),
            vec![name.clone()],
        )
    } else {
        (
            "SELECT s1.name, s1.kind, ir.kind, ir.to_type_name, s1.file
             FROM impl_refs ir
             JOIN symbols s1 ON s1.id = ir.from_symbol_id
             ORDER BY ir.to_type_name, s1.name".to_string(),
            vec![],
        )
    };

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = Vec::new();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
    let mut raw = stmt.query(param_refs.as_slice())?;
    while let Some(r) = raw.next()? {
        rows.push(vec![
            json!(r.get::<_, String>(0).unwrap_or_default()),
            json!(r.get::<_, String>(1).unwrap_or_default()),
            json!(r.get::<_, String>(2).unwrap_or_default()),
            json!(r.get::<_, String>(3).unwrap_or_default()),
            json!(r.get::<_, String>(4).unwrap_or_default()),
        ]);
    }
    Ok(rows)
}

/// Extract the argument after a keyword: "DEFINES src/main.rs" → Some("src/main.rs")
fn extract_arg(cypher: &str, keyword: &str) -> Option<String> {
    let upper = cypher.to_uppercase();
    let idx = upper.find(&keyword.to_uppercase())?;
    let rest = cypher[idx + keyword.len()..].trim();
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    }
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

