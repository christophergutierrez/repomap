use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

/// Specification for extracting symbols from a language's AST.
#[derive(Debug, Clone)]
pub struct LanguageSpec {
    /// tree-sitter language name
    pub ts_language: &'static str,
    /// Node types that represent extractable symbols: node_type -> symbol kind
    pub symbol_node_types: HashMap<&'static str, &'static str>,
    /// How to extract symbol name: node_type -> child field name
    pub name_fields: HashMap<&'static str, &'static str>,
    /// Parameter extraction: node_type -> child field name
    pub param_fields: HashMap<&'static str, &'static str>,
    /// Return type extraction: node_type -> child field name
    pub return_type_fields: HashMap<&'static str, &'static str>,
    /// Docstring strategy: "next_sibling_string" | "preceding_comment"
    pub docstring_strategy: &'static str,
    /// Decorator/attribute node type (None if not applicable)
    pub decorator_node_type: Option<&'static str>,
    /// Node types that indicate nesting (methods inside classes)
    pub container_node_types: Vec<&'static str>,
    /// Node types for constants
    pub constant_patterns: Vec<&'static str>,
    /// Node types for type definitions
    pub type_patterns: Vec<&'static str>,
    /// If true, decorators are direct children of declaration (C#)
    pub decorator_from_children: bool,
}

/// File extension to language name mapping.
pub fn language_for_extension(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?;
    match ext {
        "py" => Some("python"),
        "js" | "jsx" => Some("javascript"),
        "ts" | "tsx" => Some("typescript"),
        "go" => Some("go"),
        "rs" => Some("rust"),
        "java" => Some("java"),
        "php" => Some("php"),
        "dart" => Some("dart"),
        "cs" => Some("csharp"),
        "c" | "h" => Some("c"),
        "proto" => Some("protobuf"),
        "lua" => Some("lua"),
        "sql" => Some("sql"),
        _ => None,
    }
}

/// Get the tree-sitter Language object for a language name.
pub fn ts_language_for(name: &str) -> Option<tree_sitter::Language> {
    match name {
        "python" => Some(tree_sitter_python::LANGUAGE.into()),
        "javascript" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "typescript" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "rust" => Some(tree_sitter_rust::LANGUAGE.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "c" => Some(tree_sitter_c::LANGUAGE.into()),
        "lua" => Some(tree_sitter_lua::LANGUAGE.into()),
        "php" => Some(tree_sitter_php::LANGUAGE_PHP.into()),
        "csharp" => Some(tree_sitter_c_sharp::LANGUAGE.into()),
        "dart" => Some(tree_sitter_dart::LANGUAGE.into()),
        "protobuf" => Some(tree_sitter_proto::LANGUAGE.into()),
        "sql" => Some(tree_sitter_sequel::LANGUAGE.into()),
        _ => None,
    }
}

/// Get the LanguageSpec for a language name.
pub fn get_spec(name: &str) -> Option<&'static LanguageSpec> {
    LANGUAGE_REGISTRY.get(name)
}

// --- Helper to build HashMaps from slices ---

fn hm(pairs: &[(&'static str, &'static str)]) -> HashMap<&'static str, &'static str> {
    pairs.iter().copied().collect()
}

// --- Language Specifications ---

static LANGUAGE_REGISTRY: LazyLock<HashMap<&'static str, LanguageSpec>> = LazyLock::new(|| {
    let mut m = HashMap::new();

    m.insert("python", LanguageSpec {
        ts_language: "python",
        symbol_node_types: hm(&[
            ("function_definition", "function"),
            ("class_definition", "class"),
        ]),
        name_fields: hm(&[
            ("function_definition", "name"),
            ("class_definition", "name"),
        ]),
        param_fields: hm(&[("function_definition", "parameters")]),
        return_type_fields: hm(&[("function_definition", "return_type")]),
        docstring_strategy: "next_sibling_string",
        decorator_node_type: Some("decorator"),
        container_node_types: vec!["class_definition"],
        constant_patterns: vec!["assignment"],
        type_patterns: vec!["type_alias_statement"],
        decorator_from_children: false,
    });

    m.insert("javascript", LanguageSpec {
        ts_language: "javascript",
        symbol_node_types: hm(&[
            ("function_declaration", "function"),
            ("class_declaration", "class"),
            ("method_definition", "method"),
            ("arrow_function", "function"),
            ("generator_function_declaration", "function"),
        ]),
        name_fields: hm(&[
            ("function_declaration", "name"),
            ("class_declaration", "name"),
            ("method_definition", "name"),
        ]),
        param_fields: hm(&[
            ("function_declaration", "parameters"),
            ("method_definition", "parameters"),
            ("arrow_function", "parameters"),
        ]),
        return_type_fields: hm(&[]),
        docstring_strategy: "preceding_comment",
        decorator_node_type: None,
        container_node_types: vec!["class_declaration", "class"],
        constant_patterns: vec!["lexical_declaration"],
        type_patterns: vec![],
        decorator_from_children: false,
    });

    m.insert("typescript", LanguageSpec {
        ts_language: "typescript",
        symbol_node_types: hm(&[
            ("function_declaration", "function"),
            ("class_declaration", "class"),
            ("method_definition", "method"),
            ("arrow_function", "function"),
            ("interface_declaration", "type"),
            ("type_alias_declaration", "type"),
            ("enum_declaration", "type"),
        ]),
        name_fields: hm(&[
            ("function_declaration", "name"),
            ("class_declaration", "name"),
            ("method_definition", "name"),
            ("interface_declaration", "name"),
            ("type_alias_declaration", "name"),
            ("enum_declaration", "name"),
        ]),
        param_fields: hm(&[
            ("function_declaration", "parameters"),
            ("method_definition", "parameters"),
            ("arrow_function", "parameters"),
        ]),
        return_type_fields: hm(&[
            ("function_declaration", "return_type"),
            ("method_definition", "return_type"),
            ("arrow_function", "return_type"),
        ]),
        docstring_strategy: "preceding_comment",
        decorator_node_type: Some("decorator"),
        container_node_types: vec!["class_declaration", "class"],
        constant_patterns: vec!["lexical_declaration"],
        type_patterns: vec!["interface_declaration", "type_alias_declaration", "enum_declaration"],
        decorator_from_children: false,
    });

    m.insert("go", LanguageSpec {
        ts_language: "go",
        symbol_node_types: hm(&[
            ("function_declaration", "function"),
            ("method_declaration", "method"),
            ("type_declaration", "type"),
        ]),
        name_fields: hm(&[
            ("function_declaration", "name"),
            ("method_declaration", "name"),
            ("type_declaration", "name"),
        ]),
        param_fields: hm(&[
            ("function_declaration", "parameters"),
            ("method_declaration", "parameters"),
        ]),
        return_type_fields: hm(&[
            ("function_declaration", "result"),
            ("method_declaration", "result"),
        ]),
        docstring_strategy: "preceding_comment",
        decorator_node_type: None,
        container_node_types: vec![],
        constant_patterns: vec!["const_declaration"],
        type_patterns: vec!["type_declaration"],
        decorator_from_children: false,
    });

    m.insert("rust", LanguageSpec {
        ts_language: "rust",
        symbol_node_types: hm(&[
            ("function_item", "function"),
            ("struct_item", "type"),
            ("enum_item", "type"),
            ("trait_item", "type"),
            ("impl_item", "class"),
            ("type_item", "type"),
        ]),
        name_fields: hm(&[
            ("function_item", "name"),
            ("struct_item", "name"),
            ("enum_item", "name"),
            ("trait_item", "name"),
            ("type_item", "name"),
        ]),
        param_fields: hm(&[("function_item", "parameters")]),
        return_type_fields: hm(&[("function_item", "return_type")]),
        docstring_strategy: "preceding_comment",
        decorator_node_type: Some("attribute_item"),
        container_node_types: vec!["impl_item", "trait_item"],
        constant_patterns: vec!["const_item", "static_item"],
        type_patterns: vec!["struct_item", "enum_item", "trait_item", "type_item"],
        decorator_from_children: false,
    });

    m.insert("java", LanguageSpec {
        ts_language: "java",
        symbol_node_types: hm(&[
            ("method_declaration", "method"),
            ("constructor_declaration", "method"),
            ("class_declaration", "class"),
            ("interface_declaration", "type"),
            ("enum_declaration", "type"),
        ]),
        name_fields: hm(&[
            ("method_declaration", "name"),
            ("constructor_declaration", "name"),
            ("class_declaration", "name"),
            ("interface_declaration", "name"),
            ("enum_declaration", "name"),
        ]),
        param_fields: hm(&[
            ("method_declaration", "parameters"),
            ("constructor_declaration", "parameters"),
        ]),
        return_type_fields: hm(&[("method_declaration", "type")]),
        docstring_strategy: "preceding_comment",
        decorator_node_type: Some("marker_annotation"),
        container_node_types: vec!["class_declaration", "interface_declaration", "enum_declaration"],
        constant_patterns: vec!["field_declaration"],
        type_patterns: vec!["interface_declaration", "enum_declaration"],
        decorator_from_children: false,
    });

    m.insert("php", LanguageSpec {
        ts_language: "php",
        symbol_node_types: hm(&[
            ("function_definition", "function"),
            ("class_declaration", "class"),
            ("method_declaration", "method"),
            ("interface_declaration", "type"),
            ("trait_declaration", "type"),
            ("enum_declaration", "type"),
        ]),
        name_fields: hm(&[
            ("function_definition", "name"),
            ("class_declaration", "name"),
            ("method_declaration", "name"),
            ("interface_declaration", "name"),
            ("trait_declaration", "name"),
            ("enum_declaration", "name"),
        ]),
        param_fields: hm(&[
            ("function_definition", "parameters"),
            ("method_declaration", "parameters"),
        ]),
        return_type_fields: hm(&[
            ("function_definition", "return_type"),
            ("method_declaration", "return_type"),
        ]),
        docstring_strategy: "preceding_comment",
        decorator_node_type: Some("attribute"),
        container_node_types: vec!["class_declaration", "trait_declaration", "interface_declaration"],
        constant_patterns: vec!["const_declaration"],
        type_patterns: vec!["interface_declaration", "trait_declaration", "enum_declaration"],
        decorator_from_children: false,
    });

    m.insert("dart", LanguageSpec {
        ts_language: "dart",
        symbol_node_types: hm(&[
            ("function_signature", "function"),
            ("class_definition", "class"),
            ("mixin_declaration", "class"),
            ("enum_declaration", "type"),
            ("extension_declaration", "class"),
            ("method_signature", "method"),
            ("type_alias", "type"),
        ]),
        name_fields: hm(&[
            ("function_signature", "name"),
            ("class_definition", "name"),
            ("enum_declaration", "name"),
            ("extension_declaration", "name"),
        ]),
        param_fields: hm(&[("function_signature", "parameters")]),
        return_type_fields: hm(&[]),
        docstring_strategy: "preceding_comment",
        decorator_node_type: Some("annotation"),
        container_node_types: vec!["class_definition", "mixin_declaration", "extension_declaration"],
        constant_patterns: vec![],
        type_patterns: vec!["type_alias", "enum_declaration"],
        decorator_from_children: false,
    });

    m.insert("csharp", LanguageSpec {
        ts_language: "csharp",
        symbol_node_types: hm(&[
            ("class_declaration", "class"),
            ("record_declaration", "class"),
            ("interface_declaration", "type"),
            ("enum_declaration", "type"),
            ("struct_declaration", "type"),
            ("delegate_declaration", "type"),
            ("method_declaration", "method"),
            ("constructor_declaration", "method"),
        ]),
        name_fields: hm(&[
            ("class_declaration", "name"),
            ("record_declaration", "name"),
            ("interface_declaration", "name"),
            ("enum_declaration", "name"),
            ("struct_declaration", "name"),
            ("delegate_declaration", "name"),
            ("method_declaration", "name"),
            ("constructor_declaration", "name"),
        ]),
        param_fields: hm(&[
            ("method_declaration", "parameters"),
            ("constructor_declaration", "parameters"),
            ("delegate_declaration", "parameters"),
        ]),
        return_type_fields: hm(&[("method_declaration", "returns")]),
        docstring_strategy: "preceding_comment",
        decorator_node_type: Some("attribute_list"),
        container_node_types: vec![
            "class_declaration", "struct_declaration",
            "record_declaration", "interface_declaration",
        ],
        constant_patterns: vec![],
        type_patterns: vec![
            "interface_declaration", "enum_declaration", "struct_declaration",
            "delegate_declaration", "record_declaration",
        ],
        decorator_from_children: true,
    });

    m.insert("c", LanguageSpec {
        ts_language: "c",
        symbol_node_types: hm(&[
            ("function_definition", "function"),
            ("struct_specifier", "type"),
            ("enum_specifier", "type"),
            ("union_specifier", "type"),
            ("type_definition", "type"),
        ]),
        name_fields: hm(&[
            ("function_definition", "declarator"),
            ("struct_specifier", "name"),
            ("enum_specifier", "name"),
            ("union_specifier", "name"),
            ("type_definition", "declarator"),
        ]),
        param_fields: hm(&[("function_definition", "declarator")]),
        return_type_fields: hm(&[("function_definition", "type")]),
        docstring_strategy: "preceding_comment",
        decorator_node_type: None,
        container_node_types: vec![],
        constant_patterns: vec!["preproc_def"],
        type_patterns: vec!["type_definition", "enum_specifier", "struct_specifier", "union_specifier"],
        decorator_from_children: false,
    });

    m.insert("lua", LanguageSpec {
        ts_language: "lua",
        symbol_node_types: hm(&[
            ("function_declaration", "function"),
        ]),
        name_fields: hm(&[
            ("function_declaration", "name"),
        ]),
        param_fields: hm(&[
            ("function_declaration", "parameters"),
        ]),
        return_type_fields: hm(&[]),
        docstring_strategy: "preceding_comment",
        decorator_node_type: None,
        container_node_types: vec![],
        constant_patterns: vec![],
        type_patterns: vec![],
        decorator_from_children: false,
    });

    m.insert("protobuf", LanguageSpec {
        ts_language: "proto",
        symbol_node_types: hm(&[
            ("message", "type"),
            ("enum", "type"),
            ("service", "class"),
            ("rpc", "method"),
        ]),
        name_fields: hm(&[
            ("message", "message_name"),
            ("enum", "enum_name"),
            ("service", "service_name"),
            ("rpc", "rpc_name"),
        ]),
        param_fields: hm(&[]),
        return_type_fields: hm(&[]),
        docstring_strategy: "preceding_comment",
        decorator_node_type: None,
        container_node_types: vec!["service"],
        constant_patterns: vec![],
        type_patterns: vec!["message", "enum"],
        decorator_from_children: false,
    });

    m.insert("sql", LanguageSpec {
        ts_language: "sql",
        symbol_node_types: hm(&[
            ("create_table", "type"),
            ("create_view", "type"),
            ("create_index", "constant"),
        ]),
        name_fields: hm(&[]),
        param_fields: hm(&[]),
        return_type_fields: hm(&[]),
        docstring_strategy: "preceding_comment",
        decorator_node_type: None,
        container_node_types: vec![],
        constant_patterns: vec![],
        type_patterns: vec!["create_table", "create_view"],
        decorator_from_children: false,
    });

    m
});

/// Count files per language from a map of relative paths to content.
pub fn count_languages_from_files(files: &HashMap<String, String>) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for key in files.keys() {
        let p = Path::new(key);
        if let Some(lang) = language_for_extension(p) {
            *counts.entry(lang.to_string()).or_default() += 1;
        }
    }
    counts
}
