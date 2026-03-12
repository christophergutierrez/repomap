//! Proto field reference extraction from .proto ASTs.
//!
//! Extracts field-level references: message fields whose type is another
//! message or enum (not a scalar builtin). Used to build REFERENCES edges.
//!
//! Result format:
//!   ProtoRef { from_symbol_id, to_type_name, field_name }

use serde::{Deserialize, Serialize};
use tree_sitter::Node;

/// A proto field reference: a message field whose type is another message/enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtoRef {
    /// Symbol ID of the containing message (e.g., "file.proto::UserRequest#type")
    pub from_symbol_id: String,
    /// Unqualified type name (e.g., "Role", "Address")
    pub to_type_name: String,
    /// Field name (e.g., "role", "address")
    pub field_name: String,
}

/// Extract field-level message/enum type references from a .proto file.
pub fn extract_proto_refs(content: &str, file_path: &str) -> Vec<ProtoRef> {
    let lang: tree_sitter::Language = tree_sitter_proto::LANGUAGE.into();
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&lang).is_err() {
        return Vec::new();
    }

    let source = content.as_bytes();
    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => return Vec::new(),
    };

    let mut results = Vec::new();
    collect_message_refs(tree.root_node(), source, file_path, "", &mut results);
    results
}

/// Collect message refs recursively from a proto AST.
fn collect_message_refs(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_qual: &str,
    results: &mut Vec<ProtoRef>,
) {
    if node.kind() != "message" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            collect_message_refs(child, source, file_path, parent_qual, results);
        }
        return;
    }

    let msg_name = get_message_name(node, source);
    if msg_name.is_empty() {
        return;
    }

    let qual_name = if parent_qual.is_empty() {
        msg_name.clone()
    } else {
        format!("{parent_qual}.{msg_name}")
    };
    let symbol_id = format!("{file_path}::{qual_name}#type");

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "message_body" {
            let mut body_cursor = child.walk();
            for item in child.children(&mut body_cursor) {
                match item.kind() {
                    "field" => {
                        let type_name = get_field_message_type(item, source);
                        let field_name = get_field_name(item, source);
                        if !type_name.is_empty() && !field_name.is_empty() {
                            results.push(ProtoRef {
                                from_symbol_id: symbol_id.clone(),
                                to_type_name: type_name,
                                field_name,
                            });
                        }
                    }
                    "message" => {
                        collect_message_refs(item, source, file_path, &qual_name, results);
                    }
                    _ => {}
                }
            }
        }
    }
}

fn node_text<'a>(source: &'a [u8], node: &Node) -> &'a str {
    std::str::from_utf8(&source[node.start_byte()..node.end_byte()]).unwrap_or("")
}

fn get_message_name(node: Node, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "message_name" {
            let mut inner_cursor = child.walk();
            for id_child in child.named_children(&mut inner_cursor) {
                if id_child.kind() == "identifier" {
                    return node_text(source, &id_child).to_string();
                }
            }
        }
    }
    String::new()
}

fn get_field_message_type(field_node: Node, source: &[u8]) -> String {
    let mut cursor = field_node.walk();
    for child in field_node.children(&mut cursor) {
        if child.kind() == "type" {
            let mut type_cursor = child.walk();
            for type_child in child.children(&mut type_cursor) {
                if type_child.kind() == "message_or_enum_type" {
                    let mut id_cursor = type_child.walk();
                    for id_child in type_child.named_children(&mut id_cursor) {
                        if id_child.kind() == "identifier" {
                            return node_text(source, &id_child).to_string();
                        }
                    }
                }
            }
        }
    }
    String::new()
}

fn get_field_name(field_node: Node, source: &[u8]) -> String {
    let mut cursor = field_node.walk();
    for child in field_node.children(&mut cursor) {
        if child.kind() == "identifier" {
            return node_text(source, &child).to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_proto_refs() {
        let content = std::fs::read_to_string("tests/fixtures/proto/sample.proto").unwrap();
        let refs = extract_proto_refs(&content, "proto/sample.proto");

        let ref_strs: Vec<String> = refs
            .iter()
            .map(|r| format!("{} -[{}]-> {}", r.from_symbol_id, r.field_name, r.to_type_name))
            .collect();

        // UserRequest.role references Role
        assert!(ref_strs.iter().any(|s| s.contains("UserRequest") && s.contains("Role") && s.contains("role")),
            "Should find UserRequest.role -> Role, got: {ref_strs:?}");

        // UserRequest.address references Address
        assert!(ref_strs.iter().any(|s| s.contains("UserRequest") && s.contains("Address") && s.contains("address")),
            "Should find UserRequest.address -> Address, got: {ref_strs:?}");

        // UserResponse.role references Role
        assert!(ref_strs.iter().any(|s| s.contains("UserResponse") && s.contains("Role")),
            "Should find UserResponse.role -> Role, got: {ref_strs:?}");

        // CreateUserRequest.role references Role
        assert!(ref_strs.iter().any(|s| s.contains("CreateUserRequest") && s.contains("Role")),
            "Should find CreateUserRequest.role -> Role, got: {ref_strs:?}");
    }
}
