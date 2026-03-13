mod config;
mod discovery;
mod graph;
mod hooks;
mod mcp;
mod parser;
mod stats;
mod storage;
mod summarizer;
mod tools;

use std::collections::HashMap;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "repomap", version, about = "Code knowledge graph — tree-sitter AST parsing, FTS5 search, and byte-offset symbol retrieval")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Index a local directory
    Index {
        /// Path to the directory to index
        path: PathBuf,

        /// Only re-index changed files
        #[arg(long)]
        incremental: bool,

        /// Skip AI-generated symbol summaries
        #[arg(long)]
        no_ai: bool,
    },
    /// Install git hooks for automatic reindexing
    Init {
        /// Path to the git repository (defaults to current directory)
        path: Option<PathBuf>,
    },
    /// Remove repomap git hooks
    Deinit {
        /// Path to the git repository (defaults to current directory)
        path: Option<PathBuf>,
    },
    /// Index a GitHub repository
    IndexRepo {
        /// GitHub URL or owner/repo string
        url: String,

        /// Only re-index changed files
        #[arg(long)]
        incremental: bool,

        /// Skip AI-generated symbol summaries
        #[arg(long)]
        no_ai: bool,

        /// GitHub API token (overrides GITHUB_TOKEN env var)
        #[arg(long)]
        token: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("REPOMAP_LOG_LEVEL")
                .unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Index {
            path,
            incremental,
            no_ai,
        }) => {
            let path = path.canonicalize()?;
            tracing::info!(?path, incremental, no_ai, "starting index");
            println!("Indexing {} ...", path.display());

            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let owner = "local";

            let store = storage::IndexStore::open_store(None)?;

            if incremental && store.index_exists(owner, &name) {
                let files = discovery::discover_files(&path)?;
                let mut current_files: HashMap<String, String> = HashMap::new();
                for fp in &files {
                    let rel = fp.strip_prefix(&path).unwrap_or(fp);
                    let rel_str = rel.to_string_lossy().to_string();
                    if let Ok(content) = std::fs::read_to_string(fp) {
                        current_files.insert(rel_str, content);
                    }
                }

                let (changed, new_files, deleted) =
                    store.detect_changes(owner, &name, &current_files)?;

                let total_affected = changed.len() + new_files.len() + deleted.len();
                if total_affected == 0 {
                    println!("No changes detected.");
                    return Ok(());
                }

                println!(
                    "Incremental: {} changed, {} new, {} deleted",
                    changed.len(),
                    new_files.len(),
                    deleted.len()
                );

                let affected: Vec<PathBuf> = changed
                    .iter()
                    .chain(new_files.iter())
                    .map(|rel| path.join(rel))
                    .collect();

                let mut parsed = parser::parse_files(&affected, &path)?;
                println!("Extracted {} symbols from affected files", parsed.symbols.len());
                let mut symbols = parsed.symbols;

                if no_ai {
                    summarizer::summarize_symbols_simple(&mut symbols);
                } else {
                    summarizer::summarize_symbols(&mut symbols, true).await;
                }

                let languages = parser::languages::count_languages_from_files(&current_files);

                let raw_files: HashMap<String, String> = changed
                    .iter()
                    .chain(new_files.iter())
                    .filter_map(|rel| current_files.remove_entry(rel))
                    .collect();

                store.incremental_save(
                    owner,
                    &name,
                    &changed,
                    &new_files,
                    &deleted,
                    &symbols,
                    &raw_files,
                    &languages,
                    Some(&path),
                    &parsed.imports,
                    &parsed.proto_refs,
                    &parsed.impl_refs,
                )?;
            } else {
                let files = discovery::discover_files(&path)?;
                println!("Found {} files", files.len());

                let mut parsed = parser::parse_files(&files, &path)?;
                println!("Extracted {} symbols", parsed.symbols.len());

                if no_ai {
                    summarizer::summarize_symbols_simple(&mut parsed.symbols);
                } else {
                    summarizer::summarize_symbols(&mut parsed.symbols, true).await;
                }

                let mut raw_files: HashMap<String, String> = HashMap::new();
                let mut source_file_list: Vec<String> = Vec::new();
                for fp in &files {
                    let rel = fp.strip_prefix(&path).unwrap_or(fp);
                    let rel_str = rel.to_string_lossy().to_string();
                    if let Ok(content) = std::fs::read_to_string(fp) {
                        raw_files.insert(rel_str.clone(), content);
                        source_file_list.push(rel_str);
                    }
                }
                source_file_list.sort();

                let languages = parser::languages::count_languages_from_files(&raw_files);

                store.save_index(
                    owner,
                    &name,
                    &source_file_list,
                    &parsed.symbols,
                    &raw_files,
                    &languages,
                    None,
                    Some(&path),
                    &parsed.imports,
                    &parsed.proto_refs,
                    &parsed.impl_refs,
                )?;
            }

            println!("Index complete.");
            Ok(())
        }
        Some(Commands::Init { path }) => {
            let path = path.unwrap_or_else(|| std::env::current_dir().expect("Cannot determine current directory"));
            let path = path.canonicalize()?;

            // Run initial index before installing hooks
            println!("Indexing {} ...", path.display());
            let store = storage::IndexStore::open_store(None)?;
            let result = tools::index_repo(
                &path.to_string_lossy(),
                false,
                &store,
            ).await;
            if let Some(err) = result.get("error") {
                eprintln!("Error indexing: {}", err);
                std::process::exit(1);
            }
            let file_count = result["file_count"].as_u64().unwrap_or(0);
            let symbol_count = result["symbol_count"].as_u64().unwrap_or(0);
            println!("Indexed: {file_count} files, {symbol_count} symbols");

            hooks::install_hooks(&path)?;
            Ok(())
        }
        Some(Commands::Deinit { path }) => {
            let path = path.unwrap_or_else(|| std::env::current_dir().expect("Cannot determine current directory"));
            let path = path.canonicalize()?;
            hooks::remove_hooks(&path)?;
            Ok(())
        }
        Some(Commands::IndexRepo {
            url,
            incremental: _,
            no_ai,
            token: _,
        }) => {
            println!("Indexing local repo: {url} ...");

            let store = storage::IndexStore::open_store(None)?;
            let result = tools::index_repo(
                &url,
                !no_ai,
                &store,
            )
            .await;

            if let Some(err) = result.get("error") {
                eprintln!("Error: {}", err);
                std::process::exit(1);
            }

            let repo = result["repo"].as_str().unwrap_or(&url);
            let file_count = result["file_count"].as_u64().unwrap_or(0);
            let symbol_count = result["symbol_count"].as_u64().unwrap_or(0);
            println!("Indexed {repo}: {file_count} files, {symbol_count} symbols");
            Ok(())
        }
        None => {
            mcp::serve_stdio().await
        }
    }
}
