//! Import statement extraction from source ASTs.
//!
//! Extracts raw import path strings from Python, Go, TypeScript, and JavaScript
//! files using tree-sitter. Returns unresolved paths.

use tree_sitter::Node;

use super::languages;

/// Extract raw import path strings from source code via tree-sitter.
///
/// Supports python, go, typescript, javascript. Returns empty vec for others.
pub fn extract_imports(content: &str, language: &str) -> Vec<String> {
    let supported = ["python", "go", "typescript", "javascript"];
    if !supported.contains(&language) {
        return Vec::new();
    }

    let ts_lang = match languages::ts_language_for(language) {
        Some(l) => l,
        None => return Vec::new(),
    };

    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&ts_lang).ok();

    let source_bytes = content.as_bytes();
    let tree = match parser.parse(source_bytes, None) {
        Some(t) => t,
        None => return Vec::new(),
    };

    match language {
        "python" => python_imports(tree.root_node(), source_bytes),
        "go" => go_imports(tree.root_node(), source_bytes),
        "typescript" | "javascript" => ts_imports(tree.root_node(), source_bytes),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn node_text<'a>(source: &'a [u8], node: &Node) -> &'a str {
    std::str::from_utf8(&source[node.start_byte()..node.end_byte()]).unwrap_or("")
}

fn iter_nodes(node: Node, callback: &mut impl FnMut(Node)) {
    callback(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        iter_nodes(child, callback);
    }
}

// ---------------------------------------------------------------------------
// Python
// ---------------------------------------------------------------------------

fn python_imports(root: Node, source: &[u8]) -> Vec<String> {
    let mut results = Vec::new();

    iter_nodes(root, &mut |node| {
        match node.kind() {
            "import_statement" => {
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    match child.kind() {
                        "dotted_name" => {
                            results.push(node_text(source, &child).to_string());
                        }
                        "aliased_import" => {
                            if let Some(name_node) = child.child_by_field_name("name") {
                                results.push(node_text(source, &name_node).to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
            "import_from_statement" => {
                let module = python_from_module(node, source);
                if !module.is_empty() {
                    results.push(module);
                }
            }
            _ => {}
        }
    });

    results
}

fn python_from_module(node: Node, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "dotted_name" => return node_text(source, &child).to_string(),
            "relative_import" => {
                let mut prefix = String::new();
                let mut module = String::new();
                let mut sub_cursor = child.walk();
                for sub in child.children(&mut sub_cursor) {
                    match sub.kind() {
                        "import_prefix" => prefix = node_text(source, &sub).to_string(),
                        "dotted_name" => module = node_text(source, &sub).to_string(),
                        _ => {}
                    }
                }
                return format!("{prefix}{module}");
            }
            _ => {}
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// Go
// ---------------------------------------------------------------------------

fn go_imports(root: Node, source: &[u8]) -> Vec<String> {
    let mut results = Vec::new();

    iter_nodes(root, &mut |node| {
        if node.kind() == "import_spec" {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "interpreted_string_literal" {
                    let raw = node_text(source, &child);
                    results.push(raw.trim_matches('"').to_string());
                    break;
                }
            }
        }
    });

    results
}

// ---------------------------------------------------------------------------
// TypeScript / JavaScript
// ---------------------------------------------------------------------------

fn ts_imports(root: Node, source: &[u8]) -> Vec<String> {
    let mut results = Vec::new();

    iter_nodes(root, &mut |node| {
        if node.kind() == "import_statement" {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "string" {
                    let raw = node_text(source, &child);
                    let trimmed = raw.trim_matches(|c| c == '"' || c == '\'' || c == '`');
                    results.push(trimmed.to_string());
                    break;
                }
            }
        }
    });

    results
}
