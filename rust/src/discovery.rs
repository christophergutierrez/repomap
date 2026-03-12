use anyhow::Result;
use std::path::{Path, PathBuf};

/// Maximum file size to index (500KB).
const MAX_FILE_SIZE: u64 = 500 * 1024;

/// Discover all indexable source files under `root`, respecting .gitignore
/// and skipping noise directories, secrets, binaries, and oversized files.
pub fn discover_files(root: &Path) -> Result<Vec<PathBuf>> {
    let resolved_root = root.canonicalize()?;
    let mut files = Vec::new();

    let walker = ignore::WalkBuilder::new(&resolved_root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|entry| !is_skipped_dir(entry.path()))
        .build();

    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }

        let path = entry.into_path();

        // Symlink escape check.
        if path.is_symlink() {
            if let Ok(resolved) = path.canonicalize() {
                if !resolved.starts_with(&resolved_root) {
                    continue;
                }
            } else {
                continue;
            }
        }

        // Must have an indexable extension.
        if !is_indexable(&path) {
            continue;
        }

        // Secret file check.
        if let Some(rel) = path.strip_prefix(&resolved_root).ok() {
            if is_secret_file(&rel.to_string_lossy()) {
                continue;
            }
        }

        // Binary extension check.
        if is_binary_extension(&path) {
            continue;
        }

        // File size check.
        if let Ok(meta) = path.metadata() {
            if meta.len() > MAX_FILE_SIZE {
                continue;
            }
        } else {
            continue;
        }

        files.push(path);
    }

    Ok(files)
}

// --- Directory skip list ---

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "vendor",
    "__pycache__",
    ".git",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    "dist",
    "build",
    ".venv",
    "venv",
    "target",
    "generated",
    ".next",
    ".nuxt",
];

fn is_skipped_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| SKIP_DIRS.contains(&name))
}

// --- Indexable extensions ---

pub const INDEXABLE_EXTENSIONS: &[&str] = &[
    "py", "ts", "tsx", "js", "jsx", "go", "rs", "java", "php", "dart", "cs", "c", "h", "proto", "lua", "sql",
];

const SKIP_FILES: &[&str] = &[
    "go.sum",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "Cargo.lock",
    "poetry.lock",
    "Pipfile.lock",
];

fn is_indexable(path: &Path) -> bool {
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    if SKIP_FILES.contains(&file_name) {
        return false;
    }

    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| INDEXABLE_EXTENSIONS.contains(&ext))
}

// --- Secret file detection ---

const SECRET_PATTERNS: &[&str] = &[
    ".env",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "*.credentials",
    "*.keystore",
    "*.jks",
    "*.token",
    "id_rsa",
    "id_ed25519",
    "id_dsa",
    "id_ecdsa",
    ".htpasswd",
    ".netrc",
    ".npmrc",
    ".pypirc",
    "credentials.json",
    "*.secrets",
];

pub fn is_secret_file(rel_path: &str) -> bool {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    let name_lower = name.to_lowercase();
    let path_lower = rel_path.to_lowercase();

    // Direct name matches.
    if name_lower.contains("secret") {
        return true;
    }

    for pattern in SECRET_PATTERNS {
        if pattern.starts_with("*.") {
            let suffix = &pattern[1..]; // e.g. ".pem"
            if name_lower.ends_with(suffix) || path_lower.ends_with(suffix) {
                return true;
            }
        } else if name_lower == *pattern || path_lower == *pattern {
            return true;
        }
        // .env.* pattern
        if name_lower.starts_with(".env") {
            return true;
        }
    }

    false
}

// --- Binary extension detection ---

const BINARY_EXTENSIONS: &[&str] = &[
    "exe", "dll", "so", "dylib", "bin", "out",
    "o", "obj", "a", "lib",
    "zip", "tar", "gz", "bz2", "xz", "7z", "rar",
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "svg", "webp", "tiff", "tif",
    "mp3", "mp4", "avi", "mov", "mkv", "wav", "flac", "ogg", "webm",
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
    "pyc", "pyo", "class", "wasm",
    "db", "sqlite", "sqlite3",
    "ttf", "otf", "woff", "woff2", "eot",
    "jar", "war", "ear",
];

pub fn is_binary_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| BINARY_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Secret detection ----

    #[test]
    fn test_secret_files_detected() {
        let secrets = [
            ".env",
            ".env.local",
            ".env.production",
            "server.pem",
            "private.key",
            "cert.p12",
            "id_rsa",
            "id_ed25519",
            "credentials.json",
            ".htpasswd",
            ".netrc",
            ".npmrc",
            ".pypirc",
            "app.secrets",
        ];
        for f in &secrets {
            assert!(is_secret_file(f), "Expected {f} to be detected as secret");
        }
    }

    #[test]
    fn test_non_secret_files_pass() {
        let safe = ["main.py", "utils.js", "README.md", "config.yaml", "server.go", "package.json"];
        for f in &safe {
            assert!(!is_secret_file(f), "Expected {f} to NOT be secret");
        }
    }

    #[test]
    fn test_secret_in_subdirectory() {
        assert!(is_secret_file("config/.env"));
        assert!(is_secret_file("deploy/certs/server.pem"));
    }

    #[test]
    fn test_secret_case_insensitive() {
        assert!(is_secret_file(".ENV"));
        assert!(is_secret_file("Server.PEM"));
    }

    // ---- Binary extension detection ----

    #[test]
    fn test_binary_extensions_detected() {
        let bins = ["file.exe", "file.dll", "file.so", "file.png", "file.jpg",
                     "file.zip", "file.wasm", "file.pyc", "file.class", "file.pdf",
                     "file.db", "file.sqlite"];
        for f in &bins {
            assert!(is_binary_extension(Path::new(f)), "Expected {f} to be binary");
        }
    }

    #[test]
    fn test_source_extensions_not_binary() {
        let src = ["file.py", "file.js", "file.ts", "file.go", "file.rs", "file.java", "file.md"];
        for f in &src {
            assert!(!is_binary_extension(Path::new(f)), "Expected {f} to NOT be binary");
        }
    }

    // ---- Indexable extensions ----

    #[test]
    fn test_indexable_extensions() {
        assert!(is_indexable(Path::new("main.py")));
        assert!(is_indexable(Path::new("app.ts")));
        assert!(is_indexable(Path::new("lib.go")));
        assert!(is_indexable(Path::new("mod.rs")));
        assert!(is_indexable(Path::new("App.java")));
        assert!(is_indexable(Path::new("main.c")));
        assert!(is_indexable(Path::new("header.h")));
        assert!(is_indexable(Path::new("schema.proto")));
        assert!(!is_indexable(Path::new("README.md")));
        assert!(!is_indexable(Path::new("image.png")));
    }

    #[test]
    fn test_skip_files() {
        assert!(!is_indexable(Path::new("go.sum")));
        assert!(!is_indexable(Path::new("package-lock.json")));
        assert!(!is_indexable(Path::new("Cargo.lock")));
    }

    // ---- Skip directories ----

    #[test]
    fn test_skip_dirs() {
        assert!(is_skipped_dir(Path::new("node_modules")));
        assert!(is_skipped_dir(Path::new("vendor")));
        assert!(is_skipped_dir(Path::new(".git")));
        assert!(is_skipped_dir(Path::new("target")));
        assert!(!is_skipped_dir(Path::new("src")));
        assert!(!is_skipped_dir(Path::new("lib")));
    }

    // ---- File discovery integration ----

    #[test]
    fn test_discover_files_fixtures() {
        let files = discover_files(Path::new("tests/fixtures")).unwrap();
        assert!(!files.is_empty());

        // Should find Python, Go, TS, JS, Rust, Java, C files
        let extensions: Vec<_> = files
            .iter()
            .filter_map(|f| f.extension().and_then(|e| e.to_str()).map(String::from))
            .collect();
        assert!(extensions.contains(&"py".to_string()));
        assert!(extensions.contains(&"go".to_string()));
        assert!(extensions.contains(&"rs".to_string()));
    }
}
