use sha2::{Digest, Sha256};

/// A code symbol extracted from source via tree-sitter.
///
/// Core data structure for all parsed symbols stored in the index.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Symbol {
    /// Unique ID: "file_path::QualifiedName#kind"
    pub id: String,
    /// Source file path (relative to repo root)
    pub file: String,
    /// Symbol name (e.g., "login")
    pub name: String,
    /// Fully qualified (e.g., "MyClass.login")
    pub qualified_name: String,
    /// "function" | "class" | "method" | "constant" | "type"
    pub kind: String,
    /// "python" | "javascript" | "typescript" | "go" | "rust" | "java" | "c" | etc.
    pub language: String,
    /// Full signature line(s)
    pub signature: String,
    /// Extracted docstring (language-specific)
    pub docstring: String,
    /// One-line AI summary
    pub summary: String,
    /// Decorators/attributes
    pub decorators: Vec<String>,
    /// Parent symbol ID (for methods -> class)
    pub parent: Option<String>,
    /// Start line number (1-indexed)
    pub line: u32,
    /// End line number (1-indexed)
    pub end_line: u32,
    /// Start byte offset in raw file
    pub byte_offset: u32,
    /// Byte length of full source
    pub byte_length: u32,
    /// SHA-256 of symbol source bytes (drift detection)
    pub content_hash: String,
}

/// Generate unique symbol ID.
///
/// Format: `{file_path}::{qualified_name}#{kind}`
pub fn make_symbol_id(file_path: &str, qualified_name: &str, kind: &str) -> String {
    if kind.is_empty() {
        format!("{file_path}::{qualified_name}")
    } else {
        format!("{file_path}::{qualified_name}#{kind}")
    }
}

/// Compute SHA-256 hash of source bytes.
pub fn compute_content_hash(source_bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_bytes);
    format!("{:x}", hasher.finalize())
}
