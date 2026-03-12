pub mod extractor;
pub mod impl_refs;
pub mod imports;
pub mod languages;
pub mod proto_refs;
pub mod symbols;

use anyhow::Result;
use std::path::{Path, PathBuf};
use symbols::Symbol;

/// Results from parsing all files in a repo.
pub struct ParseResult {
    pub symbols: Vec<Symbol>,
    pub imports: Vec<(String, Vec<String>)>,        // (rel_path, import_paths)
    pub proto_refs: Vec<proto_refs::ProtoRef>,
    pub impl_refs: Vec<impl_refs::ImplRef>,
}

/// Parse all files and extract symbols, imports, proto refs, and impl refs.
pub fn parse_files(files: &[PathBuf], repo_root: &Path) -> Result<ParseResult> {
    let mut all_symbols = Vec::new();
    let mut all_imports = Vec::new();
    let mut all_proto_refs = Vec::new();
    let mut all_impl_refs = Vec::new();

    for path in files {
        let language = match languages::language_for_extension(path) {
            Some(l) => l,
            None => continue,
        };

        let ts_lang = match languages::ts_language_for(language) {
            Some(l) => l,
            None => continue,
        };

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(?path, error = %e, "failed to read file, skipping");
                continue;
            }
        };

        let rel_path = path
            .strip_prefix(repo_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        // Extract implementation/inheritance refs (before parse_file consumes ts_lang).
        let irefs = impl_refs::extract_impl_refs(&content, &rel_path, language, ts_lang.clone());
        all_impl_refs.extend(irefs);

        // Extract symbols.
        let syms = extractor::parse_file(&content, &rel_path, language, ts_lang);
        all_symbols.extend(syms);

        // Extract imports (Python, Go, TS, JS).
        let file_imports = imports::extract_imports(&content, language);
        if !file_imports.is_empty() {
            all_imports.push((rel_path.clone(), file_imports));
        }

        // Extract proto refs (.proto files).
        if language == "protobuf" {
            let refs = proto_refs::extract_proto_refs(&content, &rel_path);
            all_proto_refs.extend(refs);
        }
    }

    Ok(ParseResult {
        symbols: all_symbols,
        imports: all_imports,
        proto_refs: all_proto_refs,
        impl_refs: all_impl_refs,
    })
}
