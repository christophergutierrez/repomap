//! Generic AST symbol extractor using tree-sitter.
//!
//! Entry point: `parse_file(content, filename, language) -> Vec<Symbol>`

use std::collections::HashMap;

use super::languages::{get_spec, LanguageSpec};
use super::symbols::{compute_content_hash, make_symbol_id, Symbol};

/// Parse source code and extract symbols using tree-sitter.
pub fn parse_file(
    content: &str,
    filename: &str,
    language: &str,
    ts_language: tree_sitter::Language,
) -> Vec<Symbol> {
    let spec = match get_spec(language) {
        Some(s) => s,
        None => return Vec::new(),
    };

    let source_bytes = content.as_bytes();

    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&ts_language).is_err() {
        return Vec::new();
    }

    let tree = match parser.parse(source_bytes, None) {
        Some(t) => t,
        None => return Vec::new(),
    };

    let mut symbols = Vec::new();
    walk_tree(
        tree.root_node(),
        spec,
        source_bytes,
        filename,
        language,
        &mut symbols,
        None,
    );

    disambiguate_overloads(&mut symbols);
    symbols
}

/// Lightweight parent info to avoid borrow issues with Vec<Symbol>.
#[derive(Clone)]
struct ParentInfo {
    id: String,
    name: String,
}

/// Recursively walk the AST and accumulate Symbol objects.
fn walk_tree(
    node: tree_sitter::Node,
    spec: &LanguageSpec,
    source: &[u8],
    filename: &str,
    language: &str,
    symbols: &mut Vec<Symbol>,
    parent: Option<&ParentInfo>,
) {
    let node_type = node.kind();

    // Dart: function_signature inside method_signature is already consumed by parent.
    if node_type == "function_signature" {
        if let Some(p) = node.parent() {
            if p.kind() == "method_signature" {
                return;
            }
        }
    }

    let mut current_parent_owned: Option<ParentInfo> = parent.cloned();

    // If this node represents a symbol type, extract and record it.
    if spec.symbol_node_types.contains_key(node_type) {
        if let Some(sym) = extract_symbol(node, spec, source, filename, language, parent) {
            current_parent_owned = Some(ParentInfo {
                id: sym.id.clone(),
                name: sym.name.clone(),
            });
            symbols.push(sym);
        }
    }

    // Constants only at module scope (no parent).
    if spec.constant_patterns.contains(&node_type) && parent.is_none() {
        if let Some(sym) = extract_constant(node, spec, source, filename, language) {
            symbols.push(sym);
        }
    }

    // Recurse into children.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_tree(child, spec, source, filename, language, symbols, current_parent_owned.as_ref());
    }
}

/// Extract a Symbol from a single AST node.
fn extract_symbol(
    node: tree_sitter::Node,
    spec: &LanguageSpec,
    source: &[u8],
    filename: &str,
    language: &str,
    parent: Option<&ParentInfo>,
) -> Option<Symbol> {
    let mut kind = spec.symbol_node_types.get(node.kind())?.to_string();

    if node.has_error() {
        return None;
    }

    let name = extract_name(node, spec, source)?;

    let (qualified_name, effective_kind) = if let Some(p) = parent {
        let qn = format!("{}.{}", p.name, name);
        let k = if kind == "function" {
            "method".to_string()
        } else {
            kind
        };
        (qn, k)
    } else {
        (name.clone(), kind)
    };
    kind = effective_kind;

    let signature = build_signature(node, spec, source);
    let docstring = extract_docstring(node, spec, source);
    let decorators = extract_decorators(node, spec, source);

    // Dart: extend end_byte to include function_body sibling.
    let mut end_byte = node.end_byte();
    let mut end_line = node.end_position().row as u32 + 1;
    if node.kind() == "function_signature" || node.kind() == "method_signature" {
        if let Some(next) = node.next_named_sibling() {
            if next.kind() == "function_body" {
                end_byte = next.end_byte();
                end_line = next.end_position().row as u32 + 1;
            }
        }
    }

    let symbol_bytes = &source[node.start_byte()..end_byte];
    let content_hash = compute_content_hash(symbol_bytes);

    Some(Symbol {
        id: make_symbol_id(filename, &qualified_name, &kind),
        file: filename.to_string(),
        name,
        qualified_name,
        kind,
        language: language.to_string(),
        signature,
        docstring,
        summary: String::new(),
        decorators,
        parent: parent.map(|p| p.id.clone()),  // ParentInfo.id
        line: node.start_position().row as u32 + 1,
        end_line,
        byte_offset: node.start_byte() as u32,
        byte_length: (end_byte - node.start_byte()) as u32,
        content_hash,
    })
}

/// Extract the symbol name from an AST node.
fn extract_name(node: tree_sitter::Node, spec: &LanguageSpec, source: &[u8]) -> Option<String> {
    let node_type = node.kind();

    // Arrow functions are anonymous.
    if node_type == "arrow_function" {
        return None;
    }

    // Go: type_declaration wraps type_spec children.
    if node_type == "type_declaration" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "type_spec" {
                if let Some(name_node) = child.child_by_field_name("name") {
                    return Some(node_text(name_node, source));
                }
            }
        }
        return None;
    }

    // Dart: mixin_declaration has identifier as direct child.
    if node_type == "mixin_declaration" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "identifier" {
                return Some(node_text(child, source));
            }
        }
        return None;
    }

    // Dart: method_signature wraps function_signature/getter_signature.
    if node_type == "method_signature" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "function_signature" || child.kind() == "getter_signature" {
                if let Some(name_node) = child.child_by_field_name("name") {
                    return Some(node_text(name_node, source));
                }
            }
        }
        return None;
    }

    // Dart: type_alias — name is first type_identifier child.
    if node_type == "type_alias" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "type_identifier" {
                return Some(node_text(child, source));
            }
        }
        return None;
    }

    // SQL: CREATE TABLE/VIEW — name is first object_reference > identifier.
    if matches!(node_type, "create_table" | "create_view") {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "object_reference" {
                let mut inner_cursor = child.walk();
                for id_child in child.named_children(&mut inner_cursor) {
                    if id_child.kind() == "identifier" {
                        return Some(node_text(id_child, source));
                    }
                }
            }
        }
        return None;
    }

    // SQL: CREATE INDEX — name is direct identifier child.
    if node_type == "create_index" {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "identifier" {
                return Some(node_text(child, source));
            }
        }
        return None;
    }

    // Proto: message/enum/service/rpc use dedicated *_name child nodes.
    if matches!(node_type, "message" | "enum" | "service" | "rpc") {
        let name_child_type = format!("{node_type}_name");
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == name_child_type {
                let mut inner_cursor = child.walk();
                for id_child in child.named_children(&mut inner_cursor) {
                    if id_child.kind() == "identifier" {
                        return Some(node_text(id_child, source));
                    }
                }
            }
        }
        return None;
    }

    // Generic field-based path.
    let field_name = spec.name_fields.get(node_type)?;
    let mut name_node = node.child_by_field_name(field_name)?;

    // C: unwrap function_declarator / pointer_declarator.
    while matches!(name_node.kind(), "function_declarator" | "pointer_declarator") {
        match name_node.child_by_field_name("declarator") {
            Some(inner) => name_node = inner,
            None => break,
        }
    }

    Some(node_text(name_node, source))
}

/// Build a signature string for a symbol (declaration without body).
fn build_signature(node: tree_sitter::Node, _spec: &LanguageSpec, source: &[u8]) -> String {
    let node_type = node.kind();

    // Proto message/enum: body is *_body named child.
    if matches!(node_type, "message" | "enum") {
        let body_type = format!("{node_type}_body");
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == body_type {
                let sig = &source[node.start_byte()..child.start_byte()];
                return String::from_utf8_lossy(sig).trim().trim_end_matches(|c: char| "{  \n\t".contains(c)).to_string();
            }
        }
    } else if node_type == "service" {
        // Proto service: body delimited by anonymous `{`.
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "{" {
                let sig = &source[node.start_byte()..child.start_byte()];
                return String::from_utf8_lossy(sig).trim().to_string();
            }
        }
    }

    // General: signature ends where body starts.
    let end_byte = node
        .child_by_field_name("body")
        .map(|b| b.start_byte())
        .unwrap_or(node.end_byte());

    let sig = String::from_utf8_lossy(&source[node.start_byte()..end_byte]);
    sig.trim().trim_end_matches(|c: char| "{: \n\t".contains(c)).to_string()
}

/// Extract docstring based on language strategy.
fn extract_docstring(node: tree_sitter::Node, spec: &LanguageSpec, source: &[u8]) -> String {
    match spec.docstring_strategy {
        "next_sibling_string" => extract_python_docstring(node, source),
        "preceding_comment" => extract_preceding_comments(node, source),
        _ => String::new(),
    }
}

/// Python docstring: first expression_statement/string in body.
fn extract_python_docstring(node: tree_sitter::Node, source: &[u8]) -> String {
    let body = match node.child_by_field_name("body") {
        Some(b) => b,
        None => return String::new(),
    };

    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() == "expression_statement" {
            // Try "expression" field first (older grammars).
            if let Some(expr) = child.child_by_field_name("expression") {
                if expr.kind() == "string" {
                    return strip_quotes(&node_text(expr, source));
                }
            }
            // Try first child directly.
            if let Some(first) = child.child(0) {
                if first.kind() == "string" || first.kind() == "concatenated_string" {
                    return strip_quotes(&node_text(first, source));
                }
            }
        } else if child.kind() == "string" {
            return strip_quotes(&node_text(child, source));
        }
    }

    String::new()
}

/// Extract preceding comment blocks.
fn extract_preceding_comments(node: tree_sitter::Node, source: &[u8]) -> String {
    let mut comments = Vec::new();

    // Walk backwards, skipping annotations.
    let mut prev = node.prev_named_sibling();
    while let Some(p) = prev {
        if p.kind() == "annotation" || p.kind() == "marker_annotation" {
            prev = p.prev_named_sibling();
        } else {
            break;
        }
    }
    while let Some(p) = prev {
        if matches!(p.kind(), "comment" | "line_comment" | "block_comment" | "documentation_comment") {
            comments.push(node_text(p, source));
            prev = p.prev_named_sibling();
        } else {
            break;
        }
    }

    if comments.is_empty() {
        return String::new();
    }

    comments.reverse();
    clean_comment_markers(&comments.join("\n"))
}

/// Strip comment delimiters.
fn clean_comment_markers(text: &str) -> String {
    let cleaned: Vec<String> = text
        .lines()
        .map(|line| {
            let mut l = line.trim().to_string();
            // Strip leading markers (longest first).
            if l.starts_with("/**") {
                l = l[3..].to_string();
            } else if l.starts_with("/*") {
                l = l[2..].to_string();
            } else if l.starts_with("///") {
                l = l[3..].to_string();
            } else if l.starts_with("//!") {
                l = l[3..].to_string();
            } else if l.starts_with("//") {
                l = l[2..].to_string();
            } else if l.starts_with('*') {
                l = l[1..].to_string();
            }
            if l.ends_with("*/") {
                l = l[..l.len() - 2].to_string();
            }
            l.trim().to_string()
        })
        .collect();
    cleaned.join("\n").trim().to_string()
}

/// Extract decorators/attributes.
fn extract_decorators(node: tree_sitter::Node, spec: &LanguageSpec, source: &[u8]) -> Vec<String> {
    let dec_type = match spec.decorator_node_type {
        Some(t) => t,
        None => return Vec::new(),
    };

    let mut decorators = Vec::new();

    if spec.decorator_from_children {
        // C#: attribute lists are children of the declaration.
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == dec_type {
                decorators.push(node_text(child, source).trim().to_string());
            }
        }
    } else {
        // Walk backwards through preceding siblings.
        let mut prev = node.prev_named_sibling();
        while let Some(p) = prev {
            if p.kind() == dec_type {
                decorators.push(node_text(p, source).trim().to_string());
                prev = p.prev_named_sibling();
            } else {
                break;
            }
        }
        decorators.reverse();
    }

    decorators
}

/// Try to extract a top-level constant.
fn extract_constant(
    node: tree_sitter::Node,
    _spec: &LanguageSpec,
    source: &[u8],
    filename: &str,
    language: &str,
) -> Option<Symbol> {
    let name = match node.kind() {
        "assignment" => {
            let left = node.child_by_field_name("left")?;
            if left.kind() != "identifier" {
                return None;
            }
            let n = node_text(left, source);
            // Convention: UPPER_CASE or Capitalized_With_Underscores.
            if !n.chars().next()?.is_uppercase() || (!n.chars().all(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit())
                && !n.contains('_'))
            {
                return None;
            }
            n
        }
        "preproc_def" => {
            let name_node = node.child_by_field_name("name")?;
            let n = node_text(name_node, source);
            if !n.chars().next()?.is_uppercase() || (!n.chars().all(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit())
                && !n.contains('_'))
            {
                return None;
            }
            n
        }
        _ => return None,
    };

    let sig_text = node_text(node, source);
    let sig = if sig_text.len() > 100 { &sig_text[..sig_text.floor_char_boundary(100)] } else { &sig_text };
    let symbol_bytes = &source[node.start_byte()..node.end_byte()];

    Some(Symbol {
        id: make_symbol_id(filename, &name, "constant"),
        file: filename.to_string(),
        name: name.clone(),
        qualified_name: name,
        kind: "constant".to_string(),
        language: language.to_string(),
        signature: sig.trim().to_string(),
        docstring: String::new(),
        summary: String::new(),
        decorators: Vec::new(),
        parent: None,
        line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
        byte_offset: node.start_byte() as u32,
        byte_length: (node.end_byte() - node.start_byte()) as u32,
        content_hash: compute_content_hash(symbol_bytes),
    })
}

/// Disambiguate symbols with duplicate IDs by appending ~N suffix.
fn disambiguate_overloads(symbols: &mut Vec<Symbol>) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for sym in symbols.iter() {
        *counts.entry(sym.id.clone()).or_default() += 1;
    }

    let duplicated: std::collections::HashSet<String> = counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(id, _)| id)
        .collect();

    if duplicated.is_empty() {
        return;
    }

    let mut ordinals: HashMap<String, usize> = HashMap::new();
    for sym in symbols.iter_mut() {
        if duplicated.contains(&sym.id) {
            let ord = ordinals.entry(sym.id.clone()).or_default();
            *ord += 1;
            sym.id = format!("{}~{}", sym.id, ord);
        }
    }
}

/// Remove triple/single quote delimiters from a docstring.
fn strip_quotes(text: &str) -> String {
    let t = text.trim();
    if t.starts_with("\"\"\"") && t.ends_with("\"\"\"") && t.len() >= 6 {
        return t[3..t.len() - 3].trim().to_string();
    }
    if t.starts_with("'''") && t.ends_with("'''") && t.len() >= 6 {
        return t[3..t.len() - 3].trim().to_string();
    }
    if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
        return t[1..t.len() - 1].trim().to_string();
    }
    if t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2 {
        return t[1..t.len() - 1].trim().to_string();
    }
    t.to_string()
}

/// Get the text of a node from source bytes.
fn node_text(node: tree_sitter::Node, source: &[u8]) -> String {
    String::from_utf8_lossy(&source[node.start_byte()..node.end_byte()]).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &str, filename: &str, language: &str) -> Vec<Symbol> {
        let ts_lang = crate::parser::languages::ts_language_for(language).unwrap();
        parse_file(content, filename, language, ts_lang)
    }

    // ---- Python ----

    #[test]
    fn test_python_class_method_function_constant() {
        let source = r#"
MAX_SIZE = 100

class MyClass:
    """A sample class."""
    def method(self, x: int) -> str:
        """Do something."""
        return str(x)

def standalone(a, b):
    """Standalone function."""
    return a + b
"#;
        let symbols = parse(source, "test.py", "python");

        let class_syms: Vec<_> = symbols.iter().filter(|s| s.kind == "class").collect();
        assert_eq!(class_syms.len(), 1);
        assert_eq!(class_syms[0].name, "MyClass");
        assert!(class_syms[0].docstring.contains("A sample class"));

        let method_syms: Vec<_> = symbols.iter().filter(|s| s.kind == "method").collect();
        assert_eq!(method_syms.len(), 1);
        assert_eq!(method_syms[0].name, "method");
        assert!(method_syms[0].parent.is_some());

        let func_syms: Vec<_> = symbols
            .iter()
            .filter(|s| s.kind == "function" && s.name == "standalone")
            .collect();
        assert_eq!(func_syms.len(), 1);
        assert!(func_syms[0].docstring.contains("Standalone function"));

        let const_syms: Vec<_> = symbols.iter().filter(|s| s.kind == "constant").collect();
        assert_eq!(const_syms.len(), 1);
        assert_eq!(const_syms[0].name, "MAX_SIZE");
    }

    #[test]
    fn test_symbol_byte_offsets() {
        let source = "class Foo:\n    pass\n\ndef bar():\n    pass\n";
        let symbols = parse(source, "test.py", "python");

        for sym in &symbols {
            assert!(sym.byte_length > 0);
            assert!(sym.line > 0);
            assert!(sym.end_line >= sym.line);
        }
    }

    #[test]
    fn test_symbol_id_format() {
        let symbols = parse("def foo(): pass\n", "src/main.py", "python");
        assert!(!symbols.is_empty());
        assert_eq!(symbols[0].id, "src/main.py::foo#function");
    }

    // ---- Go ----

    #[test]
    fn test_go_struct_function() {
        let source = r#"
package sample

const MaxRetries = 3

type User struct {
    ID   int
    Name string
}

func GetUser(id int) User {
    return User{ID: id}
}

func Authenticate(token string) bool {
    return len(token) > 0
}
"#;
        let symbols = parse(source, "sample.go", "go");

        let types: Vec<_> = symbols.iter().filter(|s| s.kind == "type").collect();
        assert!(types.iter().any(|s| s.name == "User"));

        let funcs: Vec<_> = symbols.iter().filter(|s| s.kind == "function").collect();
        assert!(funcs.iter().any(|s| s.name == "GetUser"));
        assert!(funcs.iter().any(|s| s.name == "Authenticate"));

        // Go constants are not extracted (const_spec not in symbol_node_types,
        // matching Python behavior).
    }

    // ---- TypeScript ----

    #[test]
    fn test_typescript_interface_function() {
        let source = r#"
interface User {
    id: number;
    name: string;
}

function authenticate(token: string): boolean {
    return token.length > 0;
}

class UserService {
    getUser(userId: number): User {
        return { id: userId, name: "" };
    }
}
"#;
        let symbols = parse(source, "sample.ts", "typescript");

        let types: Vec<_> = symbols.iter().filter(|s| s.kind == "type").collect();
        assert!(types.iter().any(|s| s.name == "User"));

        let funcs: Vec<_> = symbols.iter().filter(|s| s.kind == "function").collect();
        assert!(funcs.iter().any(|s| s.name == "authenticate"));

        let classes: Vec<_> = symbols.iter().filter(|s| s.kind == "class").collect();
        assert!(classes.iter().any(|s| s.name == "UserService"));
    }

    // ---- Rust ----

    #[test]
    fn test_rust_struct_fn_impl() {
        let source = r#"
struct User {
    id: u32,
    name: String,
}

/// Create a new user
fn new(id: u32, name: &str) -> User {
    User { id, name: name.to_string() }
}

/// Authenticate a token
fn authenticate(token: &str) -> bool {
    !token.is_empty()
}
"#;
        let symbols = parse(source, "sample.rs", "rust");

        let types: Vec<_> = symbols.iter().filter(|s| s.kind == "type").collect();
        assert!(types.iter().any(|s| s.name == "User"));

        let funcs: Vec<_> = symbols.iter().filter(|s| s.kind == "function").collect();
        assert!(funcs.iter().any(|s| s.name == "authenticate"));
    }

    // ---- Java ----

    #[test]
    fn test_java_class_method() {
        let source = r#"
public class Sample {
    public static boolean authenticate(String token) {
        return token.length() > 0;
    }
}
"#;
        let symbols = parse(source, "Sample.java", "java");

        let classes: Vec<_> = symbols.iter().filter(|s| s.kind == "class").collect();
        assert!(classes.iter().any(|s| s.name == "Sample"));

        let methods: Vec<_> = symbols.iter().filter(|s| s.kind == "method").collect();
        assert!(methods.iter().any(|s| s.name == "authenticate"));
    }

    // ---- C ----

    #[test]
    fn test_c_function_struct() {
        let source = r#"
struct User {
    int id;
    char name[100];
};

/* Authenticate a token string. */
int authenticate(const char *token) {
    return token != NULL;
}
"#;
        let symbols = parse(source, "sample.c", "c");

        let types: Vec<_> = symbols.iter().filter(|s| s.kind == "type").collect();
        assert!(types.iter().any(|s| s.name == "User"));

        let funcs: Vec<_> = symbols.iter().filter(|s| s.kind == "function").collect();
        assert!(funcs.iter().any(|s| s.name == "authenticate"));
    }

    // ---- JavaScript ----

    #[test]
    fn test_javascript_function_class() {
        let source = r#"
function authenticate(token) {
    return token.length > 0;
}

class UserService {
    getUser(userId) {
        return { id: userId };
    }
}
"#;
        let symbols = parse(source, "sample.js", "javascript");

        let funcs: Vec<_> = symbols.iter().filter(|s| s.kind == "function").collect();
        assert!(funcs.iter().any(|s| s.name == "authenticate"));

        let classes: Vec<_> = symbols.iter().filter(|s| s.kind == "class").collect();
        assert!(classes.iter().any(|s| s.name == "UserService"));
    }

    // ---- Overload disambiguation ----

    #[test]
    fn test_overload_disambiguation() {
        let source = r#"
def foo(x): pass
def foo(x, y): pass
"#;
        let symbols = parse(source, "test.py", "python");
        let foo_syms: Vec<_> = symbols.iter().filter(|s| s.name == "foo").collect();
        assert_eq!(foo_syms.len(), 2);
        // IDs should be disambiguated with ~N
        let ids: Vec<_> = foo_syms.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.iter().any(|id| id.contains("~")));
    }

    // ---- Empty / unknown language ----

    #[test]
    fn test_unknown_spec_returns_empty() {
        // "unknown" language has no spec
        let ts_lang = crate::parser::languages::ts_language_for("python").unwrap();
        let result = parse_file("some code", "test.unknown", "unknown", ts_lang);
        assert!(result.is_empty());
    }

    // ---- Fixture files ----

    #[test]
    fn test_parse_fixture_python() {
        let content = std::fs::read_to_string("tests/fixtures/python/sample.py").unwrap();
        let symbols = parse(&content, "python/sample.py", "python");
        // Should have: MAX_RETRIES (constant), UserService (class), get_user (method),
        // delete_user (method), authenticate (function)
        assert!(symbols.len() >= 4);

        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"authenticate"));
        assert!(names.contains(&"MAX_RETRIES"));
    }

    #[test]
    fn test_parse_fixture_go() {
        let content = std::fs::read_to_string("tests/fixtures/go/sample.go").unwrap();
        let symbols = parse(&content, "go/sample.go", "go");
        assert!(symbols.len() >= 3);

        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"User"));
        assert!(names.contains(&"Authenticate"));
    }

    #[test]
    fn test_parse_all_supported_fixtures() {
        let fixtures = [
            ("tests/fixtures/python/sample.py", "python"),
            ("tests/fixtures/typescript/sample.ts", "typescript"),
            ("tests/fixtures/javascript/sample.js", "javascript"),
            ("tests/fixtures/go/sample.go", "go"),
            ("tests/fixtures/rust/sample.rs", "rust"),
            ("tests/fixtures/java/Sample.java", "java"),
            ("tests/fixtures/c/sample.c", "c"),
            ("tests/fixtures/lua/sample.lua", "lua"),
            ("tests/fixtures/sql/sample.sql", "sql"),
        ];

        for (path, lang) in &fixtures {
            let content = std::fs::read_to_string(path).unwrap();
            let symbols = parse(&content, path, lang);
            assert!(
                !symbols.is_empty(),
                "Expected symbols from {path} ({lang}), got none"
            );
        }
    }

    // ---- Lua ----

    #[test]
    fn test_lua_functions() {
        let content = std::fs::read_to_string("tests/fixtures/lua/sample.lua").unwrap();
        let symbols = parse(&content, "sample.lua", "lua");
        assert!(!symbols.is_empty(), "Lua should extract symbols");

        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"authenticate"), "Should find authenticate function");
    }

    // ---- SQL ----

    #[test]
    fn test_sql_ddl() {
        let content = std::fs::read_to_string("tests/fixtures/sql/sample.sql").unwrap();
        let symbols = parse(&content, "sample.sql", "sql");
        assert!(!symbols.is_empty(), "SQL should extract symbols");

        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"users"), "Should find users table");
        assert!(names.contains(&"orders"), "Should find orders table");
        assert!(names.contains(&"active_users"), "Should find active_users view");
        assert!(names.contains(&"idx_users_email"), "Should find index");
    }

}
