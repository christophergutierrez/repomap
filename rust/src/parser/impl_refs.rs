//! Implementation/inheritance relationship extraction from ASTs.
//!
//! Extracts explicit implementation and inheritance edges:
//!   - Rust: `impl Trait for Type`
//!   - Java: `class Foo extends Bar implements Baz`
//!   - C#: `class Foo : IBar, Baz`
//!   - TypeScript: `class Foo extends Bar implements IBaz`
//!   - Python: `class Foo(Bar, Baz):`
//!   - PHP: `class Foo implements Bar { use Baz; }`
//!   - Dart: `class Foo extends Bar implements Baz with Mixin`
//!   - JavaScript: `class Foo extends Bar`
//!
//! Result format:
//!   ImplRef { from_symbol_id, to_type_name, kind }

use serde::{Deserialize, Serialize};
use tree_sitter::Node;

/// An implementation/inheritance reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImplRef {
    /// Symbol ID of the implementing type (e.g., "src/main.rs::User#type")
    pub from_symbol_id: String,
    /// Name of the base/interface type (e.g., "Display", "BaseService")
    pub to_type_name: String,
    /// Relationship kind: "implements", "extends", or "trait_impl"
    pub kind: String,
}

/// Extract implementation/inheritance refs from a parsed file.
pub fn extract_impl_refs(
    content: &str,
    file_path: &str,
    language: &str,
    ts_language: tree_sitter::Language,
) -> Vec<ImplRef> {
    let source = content.as_bytes();
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&ts_language).is_err() {
        return Vec::new();
    }
    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => return Vec::new(),
    };

    let mut results = Vec::new();
    collect_impl_refs(tree.root_node(), source, file_path, language, &mut results, None);
    results
}

fn collect_impl_refs(
    node: Node,
    source: &[u8],
    file_path: &str,
    language: &str,
    results: &mut Vec<ImplRef>,
    _parent_name: Option<&str>,
) {
    match language {
        "rust" => collect_rust(node, source, file_path, results),
        "java" => collect_java(node, source, file_path, results),
        "csharp" => collect_csharp(node, source, file_path, results),
        "typescript" => collect_typescript(node, source, file_path, results),
        "python" => collect_python(node, source, file_path, results),
        "php" => collect_php(node, source, file_path, results),
        "dart" => collect_dart(node, source, file_path, results),
        "javascript" => collect_javascript(node, source, file_path, results),
        _ => {}
    }
}

fn node_text(node: Node, source: &[u8]) -> String {
    String::from_utf8_lossy(&source[node.start_byte()..node.end_byte()]).to_string()
}

fn make_id(file_path: &str, name: &str, kind: &str) -> String {
    format!("{file_path}::{name}#{kind}")
}

// ---------------------------------------------------------------------------
// Rust: `impl Trait for Type` → trait_impl, `impl Type` → skip (no inheritance)
// ---------------------------------------------------------------------------

fn collect_rust(node: Node, source: &[u8], file_path: &str, results: &mut Vec<ImplRef>) {
    if node.kind() == "impl_item" {
        // impl_item has optional `trait` field and required `type` field
        if let (Some(trait_node), Some(type_node)) =
            (node.child_by_field_name("trait"), node.child_by_field_name("type"))
        {
            let trait_name = node_text(trait_node, source);
            let type_name = node_text(type_node, source);
            // The implementing type is the struct/enum
            let from_id = make_id(file_path, &type_name, "type");
            results.push(ImplRef {
                from_symbol_id: from_id,
                to_type_name: trait_name,
                kind: "trait_impl".to_string(),
            });
        }
        // `impl Type { ... }` has no trait field — not an inheritance edge
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_rust(child, source, file_path, results);
    }
}

// ---------------------------------------------------------------------------
// Java: extends (superclass) + implements (super_interfaces > type_list)
// ---------------------------------------------------------------------------

fn collect_java(node: Node, source: &[u8], file_path: &str, results: &mut Vec<ImplRef>) {
    if node.kind() == "class_declaration" {
        let class_name = match node.child_by_field_name("name") {
            Some(n) => node_text(n, source),
            None => {
                recurse_children(node, source, file_path, "java", results);
                return;
            }
        };
        let from_id = make_id(file_path, &class_name, "class");

        // superclass field
        if let Some(sc) = node.child_by_field_name("superclass") {
            // superclass node contains a type_identifier
            let mut cursor = sc.walk();
            for child in sc.children(&mut cursor) {
                if child.kind() == "type_identifier" {
                    results.push(ImplRef {
                        from_symbol_id: from_id.clone(),
                        to_type_name: node_text(child, source),
                        kind: "extends".to_string(),
                    });
                }
            }
        }

        // interfaces field (super_interfaces > type_list > type_identifier)
        if let Some(ifaces) = node.child_by_field_name("interfaces") {
            collect_type_list_children(ifaces, source, &from_id, "implements", results);
        }
    }

    recurse_children(node, source, file_path, "java", results);
}

// ---------------------------------------------------------------------------
// C#: base_list contains all bases (no extends vs implements distinction in AST)
// ---------------------------------------------------------------------------

fn collect_csharp(node: Node, source: &[u8], file_path: &str, results: &mut Vec<ImplRef>) {
    if matches!(node.kind(), "class_declaration" | "struct_declaration" | "record_declaration") {
        let kind_str = if node.kind() == "class_declaration" { "class" } else { "type" };
        let class_name = match node.child_by_field_name("name") {
            Some(n) => node_text(n, source),
            None => {
                recurse_children(node, source, file_path, "csharp", results);
                return;
            }
        };
        let from_id = make_id(file_path, &class_name, kind_str);

        // base_list contains base types
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "base_list" {
                let mut bl_cursor = child.walk();
                for base_child in child.named_children(&mut bl_cursor) {
                    // Each entry is an identifier or generic_name
                    if matches!(base_child.kind(), "identifier" | "generic_name" | "qualified_name") {
                        let base_name = node_text(base_child, source);
                        let rel_kind = if base_name.starts_with('I') && base_name.len() > 1 && base_name.chars().nth(1).unwrap_or('a').is_uppercase() {
                            "implements"
                        } else {
                            "extends"
                        };
                        results.push(ImplRef {
                            from_symbol_id: from_id.clone(),
                            to_type_name: base_name,
                            kind: rel_kind.to_string(),
                        });
                    }
                }
            }
        }
    }

    recurse_children(node, source, file_path, "csharp", results);
}

// ---------------------------------------------------------------------------
// TypeScript: class_heritage > extends_clause / implements_clause
// ---------------------------------------------------------------------------

fn collect_typescript(node: Node, source: &[u8], file_path: &str, results: &mut Vec<ImplRef>) {
    if node.kind() == "class_declaration" {
        let class_name = match node.child_by_field_name("name") {
            Some(n) => node_text(n, source),
            None => {
                recurse_children(node, source, file_path, "typescript", results);
                return;
            }
        };
        let from_id = make_id(file_path, &class_name, "class");

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "class_heritage" {
                let mut hc = child.walk();
                for clause in child.children(&mut hc) {
                    match clause.kind() {
                        "extends_clause" => {
                            collect_identifiers_from(clause, source, &from_id, "extends", results);
                        }
                        "implements_clause" => {
                            collect_identifiers_from(clause, source, &from_id, "implements", results);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    recurse_children(node, source, file_path, "typescript", results);
}

// ---------------------------------------------------------------------------
// Python: class Foo(Bar, Baz): — bases in argument_list field "superclasses"
// ---------------------------------------------------------------------------

fn collect_python(node: Node, source: &[u8], file_path: &str, results: &mut Vec<ImplRef>) {
    if node.kind() == "class_definition" {
        let class_name = match node.child_by_field_name("name") {
            Some(n) => node_text(n, source),
            None => {
                recurse_children(node, source, file_path, "python", results);
                return;
            }
        };
        let from_id = make_id(file_path, &class_name, "class");

        // superclasses field is an argument_list
        if let Some(bases) = node.child_by_field_name("superclasses") {
            let mut cursor = bases.walk();
            for child in bases.named_children(&mut cursor) {
                if child.kind() == "identifier" {
                    results.push(ImplRef {
                        from_symbol_id: from_id.clone(),
                        to_type_name: node_text(child, source),
                        kind: "extends".to_string(),
                    });
                } else if child.kind() == "attribute" {
                    // e.g., abc.ABC
                    results.push(ImplRef {
                        from_symbol_id: from_id.clone(),
                        to_type_name: node_text(child, source),
                        kind: "extends".to_string(),
                    });
                }
            }
        }
    }

    recurse_children(node, source, file_path, "python", results);
}

// ---------------------------------------------------------------------------
// PHP: class Foo implements Bar { use Baz; }
// ---------------------------------------------------------------------------

fn collect_php(node: Node, source: &[u8], file_path: &str, results: &mut Vec<ImplRef>) {
    if node.kind() == "class_declaration" {
        let class_name = match node.child_by_field_name("name") {
            Some(n) => node_text(n, source),
            None => {
                recurse_children(node, source, file_path, "php", results);
                return;
            }
        };
        let from_id = make_id(file_path, &class_name, "class");

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            // class_interface_clause for implements
            if child.kind() == "class_interface_clause" {
                let mut ic = child.walk();
                for name_node in child.named_children(&mut ic) {
                    if name_node.kind() == "name" {
                        results.push(ImplRef {
                            from_symbol_id: from_id.clone(),
                            to_type_name: node_text(name_node, source),
                            kind: "implements".to_string(),
                        });
                    }
                }
            }
            // base_clause for extends
            if child.kind() == "base_clause" {
                let mut bc = child.walk();
                for name_node in child.named_children(&mut bc) {
                    if name_node.kind() == "name" {
                        results.push(ImplRef {
                            from_symbol_id: from_id.clone(),
                            to_type_name: node_text(name_node, source),
                            kind: "extends".to_string(),
                        });
                    }
                }
            }
            // use_declaration inside declaration_list for traits
            if child.kind() == "declaration_list" {
                let mut dl = child.walk();
                for item in child.named_children(&mut dl) {
                    if item.kind() == "use_declaration" {
                        let mut uc = item.walk();
                        for use_child in item.named_children(&mut uc) {
                            if use_child.kind() == "name" {
                                results.push(ImplRef {
                                    from_symbol_id: from_id.clone(),
                                    to_type_name: node_text(use_child, source),
                                    kind: "trait_impl".to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    recurse_children(node, source, file_path, "php", results);
}

// ---------------------------------------------------------------------------
// Dart: class Foo extends Bar implements Baz with Mixin
// ---------------------------------------------------------------------------

fn collect_dart(node: Node, source: &[u8], file_path: &str, results: &mut Vec<ImplRef>) {
    // tree-sitter-dart 0.1 uses "class_declaration"; some versions use "class_definition"
    if node.kind() == "class_declaration" || node.kind() == "class_definition" {
        let class_name = match node.child_by_field_name("name") {
            Some(n) => node_text(n, source),
            None => {
                recurse_children(node, source, file_path, "dart", results);
                return;
            }
        };
        let from_id = make_id(file_path, &class_name, "class");

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "superclass" {
                // superclass has a type_identifier child
                if let Some(type_node) = child.child_by_field_name("type") {
                    results.push(ImplRef {
                        from_symbol_id: from_id.clone(),
                        to_type_name: node_text(type_node, source),
                        kind: "extends".to_string(),
                    });
                } else {
                    // fallback: first type_identifier child
                    let mut sc = child.walk();
                    for sc_child in child.named_children(&mut sc) {
                        if sc_child.kind() == "type_identifier" {
                            results.push(ImplRef {
                                from_symbol_id: from_id.clone(),
                                to_type_name: node_text(sc_child, source),
                                kind: "extends".to_string(),
                            });
                            break;
                        }
                    }
                }
            }
            if child.kind() == "interfaces" {
                collect_type_identifiers(child, source, &from_id, "implements", results);
            }
            if child.kind() == "mixins" {
                collect_type_identifiers(child, source, &from_id, "trait_impl", results);
            }
        }
    }

    recurse_children(node, source, file_path, "dart", results);
}

// ---------------------------------------------------------------------------
// JavaScript: class Foo extends Bar (no implements in JS)
// ---------------------------------------------------------------------------

fn collect_javascript(node: Node, source: &[u8], file_path: &str, results: &mut Vec<ImplRef>) {
    if node.kind() == "class_declaration" {
        let class_name = match node.child_by_field_name("name") {
            Some(n) => node_text(n, source),
            None => {
                recurse_children(node, source, file_path, "javascript", results);
                return;
            }
        };
        let from_id = make_id(file_path, &class_name, "class");

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "class_heritage" {
                let mut hc = child.walk();
                for hchild in child.named_children(&mut hc) {
                    if hchild.kind() == "identifier" {
                        results.push(ImplRef {
                            from_symbol_id: from_id.clone(),
                            to_type_name: node_text(hchild, source),
                            kind: "extends".to_string(),
                        });
                    }
                }
            }
        }
    }

    recurse_children(node, source, file_path, "javascript", results);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn recurse_children(
    node: Node,
    source: &[u8],
    file_path: &str,
    language: &str,
    results: &mut Vec<ImplRef>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_impl_refs(child, source, file_path, language, results, None);
    }
}

/// Collect type_identifier nodes from a parent that contains a type_list or similar.
fn collect_type_list_children(
    node: Node,
    source: &[u8],
    from_id: &str,
    kind: &str,
    results: &mut Vec<ImplRef>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "type_list" {
            let mut tc = child.walk();
            for type_child in child.named_children(&mut tc) {
                if type_child.kind() == "type_identifier" {
                    results.push(ImplRef {
                        from_symbol_id: from_id.to_string(),
                        to_type_name: node_text(type_child, source),
                        kind: kind.to_string(),
                    });
                }
            }
        } else if child.kind() == "type_identifier" {
            results.push(ImplRef {
                from_symbol_id: from_id.to_string(),
                to_type_name: node_text(child, source),
                kind: kind.to_string(),
            });
        }
    }
}

/// Collect identifiers from a clause (extends_clause, implements_clause in TS).
fn collect_identifiers_from(
    node: Node,
    source: &[u8],
    from_id: &str,
    kind: &str,
    results: &mut Vec<ImplRef>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "identifier" | "type_identifier") {
            results.push(ImplRef {
                from_symbol_id: from_id.to_string(),
                to_type_name: node_text(child, source),
                kind: kind.to_string(),
            });
        }
    }
}

/// Collect type_identifier nodes (for Dart interfaces/mixins).
fn collect_type_identifiers(
    node: Node,
    source: &[u8],
    from_id: &str,
    kind: &str,
    results: &mut Vec<ImplRef>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "type_identifier" {
            results.push(ImplRef {
                from_symbol_id: from_id.to_string(),
                to_type_name: node_text(child, source),
                kind: kind.to_string(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::languages::ts_language_for;

    fn extract(content: &str, file_path: &str, language: &str) -> Vec<ImplRef> {
        let ts_lang = ts_language_for(language).unwrap();
        extract_impl_refs(content, file_path, language, ts_lang)
    }

    #[test]
    fn test_rust_trait_impl() {
        let content = std::fs::read_to_string("tests/fixtures/rust/sample.rs").unwrap();
        let refs = extract(&content, "rust/sample.rs", "rust");

        let strs: Vec<String> = refs.iter().map(|r| format!("{} -[{}]-> {}", r.from_symbol_id, r.kind, r.to_type_name)).collect();

        assert!(strs.iter().any(|s| s.contains("User") && s.contains("Authenticatable") && s.contains("trait_impl")),
            "Should find User impl Authenticatable, got: {strs:?}");
        assert!(strs.iter().any(|s| s.contains("User") && s.contains("Display") && s.contains("trait_impl")),
            "Should find User impl Display, got: {strs:?}");
        // Plain `impl User { }` should NOT produce a ref
        assert!(!strs.iter().any(|s| s.contains("-> User")),
            "Should not find self-impl, got: {strs:?}");
    }

    #[test]
    fn test_java_extends_implements() {
        let content = std::fs::read_to_string("tests/fixtures/java/Sample.java").unwrap();
        let refs = extract(&content, "java/Sample.java", "java");

        let strs: Vec<String> = refs.iter().map(|r| format!("{} -[{}]-> {}", r.from_symbol_id, r.kind, r.to_type_name)).collect();

        assert!(strs.iter().any(|s| s.contains("Sample") && s.contains("BaseService") && s.contains("extends")),
            "Should find Sample extends BaseService, got: {strs:?}");
        assert!(strs.iter().any(|s| s.contains("Sample") && s.contains("Serializable") && s.contains("implements")),
            "Should find Sample implements Serializable, got: {strs:?}");
        assert!(strs.iter().any(|s| s.contains("Sample") && s.contains("Repository") && s.contains("implements")),
            "Should find Sample implements Repository, got: {strs:?}");
    }

    #[test]
    fn test_csharp_base_list() {
        let content = std::fs::read_to_string("tests/fixtures/csharp/sample.cs").unwrap();
        let refs = extract(&content, "csharp/sample.cs", "csharp");

        let strs: Vec<String> = refs.iter().map(|r| format!("{} -[{}]-> {}", r.from_symbol_id, r.kind, r.to_type_name)).collect();

        assert!(strs.iter().any(|s| s.contains("SqlRepository") && s.contains("IRepository") && s.contains("implements")),
            "Should find SqlRepository : IRepository, got: {strs:?}");
    }

    #[test]
    fn test_typescript_extends_implements() {
        let content = std::fs::read_to_string("tests/fixtures/typescript/sample.ts").unwrap();
        let refs = extract(&content, "typescript/sample.ts", "typescript");

        let strs: Vec<String> = refs.iter().map(|r| format!("{} -[{}]-> {}", r.from_symbol_id, r.kind, r.to_type_name)).collect();

        assert!(strs.iter().any(|s| s.contains("UserService") && s.contains("BaseService") && s.contains("extends")),
            "Should find UserService extends BaseService, got: {strs:?}");
        assert!(strs.iter().any(|s| s.contains("UserService") && s.contains("Searchable") && s.contains("implements")),
            "Should find UserService implements Searchable, got: {strs:?}");
    }

    #[test]
    fn test_python_inheritance() {
        let content = std::fs::read_to_string("tests/fixtures/python/sample.py").unwrap();
        let refs = extract(&content, "python/sample.py", "python");

        let strs: Vec<String> = refs.iter().map(|r| format!("{} -[{}]-> {}", r.from_symbol_id, r.kind, r.to_type_name)).collect();

        assert!(strs.iter().any(|s| s.contains("UserService") && s.contains("BaseService") && s.contains("extends")),
            "Should find UserService(BaseService), got: {strs:?}");
    }

    #[test]
    fn test_php_implements_use() {
        let content = std::fs::read_to_string("tests/fixtures/php/sample.php").unwrap();
        let refs = extract(&content, "php/sample.php", "php");

        let strs: Vec<String> = refs.iter().map(|r| format!("{} -[{}]-> {}", r.from_symbol_id, r.kind, r.to_type_name)).collect();

        assert!(strs.iter().any(|s| s.contains("UserService") && s.contains("Authenticatable") && s.contains("implements")),
            "Should find UserService implements Authenticatable, got: {strs:?}");
        assert!(strs.iter().any(|s| s.contains("UserService") && s.contains("Timestampable") && s.contains("trait_impl")),
            "Should find UserService use Timestampable, got: {strs:?}");
    }

    #[test]
    fn test_dart_extends() {
        let content = std::fs::read_to_string("tests/fixtures/dart/sample.dart").unwrap();
        let refs = extract(&content, "dart/sample.dart", "dart");

        let strs: Vec<String> = refs.iter().map(|r| format!("{} -[{}]-> {}", r.from_symbol_id, r.kind, r.to_type_name)).collect();

        assert!(strs.iter().any(|s| s.contains("UserService") && s.contains("BaseService") && s.contains("extends")),
            "Should find UserService extends BaseService, got: {strs:?}");
    }

    #[test]
    fn test_javascript_extends() {
        let content = std::fs::read_to_string("tests/fixtures/javascript/sample.js").unwrap();
        let refs = extract(&content, "javascript/sample.js", "javascript");

        let strs: Vec<String> = refs.iter().map(|r| format!("{} -[{}]-> {}", r.from_symbol_id, r.kind, r.to_type_name)).collect();

        assert!(strs.iter().any(|s| s.contains("UserService") && s.contains("BaseService") && s.contains("extends")),
            "Should find UserService extends BaseService, got: {strs:?}");
    }
}
