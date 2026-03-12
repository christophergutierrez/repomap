//! Three-tier symbol summarization: docstring > AI (Haiku/Gemini/local) > signature fallback.
//!
//! Tier 1 — Docstring extraction (free, no API call)
//! Tier 2 — AI batch summarization (requires API key or local LLM)
//! Tier 3 — Signature fallback (always works)

use serde_json::json;
use tracing::{info, warn};

use crate::parser::symbols::Symbol;

// ---------------------------------------------------------------------------
// Tier 1 and Tier 3 helpers (no dependencies)
// ---------------------------------------------------------------------------

/// Extract a one-sentence summary from a docstring (Tier 1).
///
/// Takes the first non-empty line and truncates at the first period.
/// Costs zero tokens.
pub fn extract_summary_from_docstring(docstring: &str) -> String {
    if docstring.is_empty() {
        return String::new();
    }

    let mut first_line = docstring
        .trim()
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    // Truncate at first period to get just the leading sentence.
    if let Some(pos) = first_line.find('.') {
        first_line.truncate(pos + 1);
    }

    if first_line.len() > 120 {
        first_line.truncate(first_line.floor_char_boundary(120));
    }

    first_line
}

/// Generate a minimal summary from the symbol's kind and name (Tier 3).
pub fn signature_fallback(symbol: &Symbol) -> String {
    match symbol.kind.as_str() {
        "class" => format!("Class {}", symbol.name),
        "constant" => format!("Constant {}", symbol.name),
        "type" => format!("Type definition {}", symbol.name),
        _ => {
            if !symbol.signature.is_empty() {
                let sig = &symbol.signature;
                if sig.len() > 120 {
                    sig[..sig.floor_char_boundary(120)].to_string()
                } else {
                    sig.clone()
                }
            } else {
                format!("{} {}", symbol.kind, symbol.name)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tier 2 — AI summarizers via HTTP
// ---------------------------------------------------------------------------

/// Which AI provider to use for batch summarization.
enum Provider {
    Anthropic {
        api_key: String,
        base_url: String,
    },
    Gemini {
        api_key: String,
    },
    OpenAI {
        api_base: String,
        api_key: String,
        model: String,
    },
}

/// Detect the appropriate provider from environment variables.
///
/// Priority: Anthropic > Gemini > OpenAI-compatible > None.
fn detect_provider() -> Option<Provider> {
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        let base_url = std::env::var("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".to_string());
        return Some(Provider::Anthropic {
            api_key: key,
            base_url,
        });
    }

    if let Ok(key) = std::env::var("GOOGLE_API_KEY") {
        return Some(Provider::Gemini { api_key: key });
    }

    if let Ok(base) = std::env::var("OPENAI_API_BASE") {
        let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| "local-llm".to_string());
        let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "qwen3-coder".to_string());
        return Some(Provider::OpenAI {
            api_base: base.trim_end_matches('/').to_string(),
            api_key,
            model,
        });
    }

    None
}

/// Build the shared summarization prompt for a batch of symbols.
fn build_prompt(symbols: &[&mut Symbol]) -> String {
    let mut lines = vec![
        "Summarize each code symbol in ONE short sentence (max 15 words).".to_string(),
        "Focus on what it does, not how.".to_string(),
        String::new(),
        "Input:".to_string(),
    ];

    for (i, sym) in symbols.iter().enumerate() {
        lines.push(format!("{}. {}: {}", i + 1, sym.kind, sym.signature));
    }

    lines.push(String::new());
    lines.push("Output format: NUMBER. SUMMARY".to_string());
    lines.push("Example: 1. Authenticates users with username and password.".to_string());
    lines.push(String::new());
    lines.push("Summaries:".to_string());

    lines.join("\n")
}

/// Parse numbered summary lines from an AI response.
fn parse_response(text: &str, expected_count: usize) -> Vec<String> {
    let mut summaries = vec![String::new(); expected_count];

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(dot_pos) = line.find('.') {
            if let Ok(num) = line[..dot_pos].trim().parse::<usize>() {
                if num >= 1 && num <= expected_count {
                    summaries[num - 1] = line[dot_pos + 1..].trim().to_string();
                }
            }
        }
    }

    summaries
}

/// Call the Anthropic Messages API.
async fn call_anthropic(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    prompt: &str,
) -> Result<String, reqwest::Error> {
    let resp = client
        .post(format!("{base_url}/v1/messages"))
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&json!({
            "model": "claude-haiku-4-5-20251001",
            "max_tokens": 500,
            "temperature": 0.0,
            "messages": [{"role": "user", "content": prompt}]
        }))
        .send()
        .await?
        .error_for_status()?;

    let body: serde_json::Value = resp.json().await?;
    Ok(body["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string())
}

/// Call the Google Gemini API.
async fn call_gemini(
    client: &reqwest::Client,
    api_key: &str,
    prompt: &str,
) -> Result<String, reqwest::Error> {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-flash:generateContent?key={api_key}"
    );

    let resp = client
        .post(&url)
        .header("content-type", "application/json")
        .json(&json!({
            "contents": [{"parts": [{"text": prompt}]}]
        }))
        .send()
        .await?
        .error_for_status()?;

    let body: serde_json::Value = resp.json().await?;
    Ok(body["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string())
}

/// Call an OpenAI-compatible chat completions endpoint.
async fn call_openai(
    client: &reqwest::Client,
    api_base: &str,
    api_key: &str,
    model: &str,
    prompt: &str,
) -> Result<String, reqwest::Error> {
    let resp = client
        .post(format!("{api_base}/chat/completions"))
        .header("authorization", format!("Bearer {api_key}"))
        .header("content-type", "application/json")
        .json(&json!({
            "model": model,
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": 500,
            "temperature": 0.0
        }))
        .send()
        .await?
        .error_for_status()?;

    let body: serde_json::Value = resp.json().await?;
    Ok(body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string())
}

/// Summarize a batch of symbols using the detected AI provider.
async fn summarize_batch_ai(
    provider: &Provider,
    client: &reqwest::Client,
    symbols: &mut [&mut Symbol],
) {
    let prompt = build_prompt(symbols);
    let count = symbols.len();

    let result = match provider {
        Provider::Anthropic { api_key, base_url } => {
            call_anthropic(client, api_key, base_url, &prompt).await
        }
        Provider::Gemini { api_key } => call_gemini(client, api_key, &prompt).await,
        Provider::OpenAI {
            api_base,
            api_key,
            model,
        } => call_openai(client, api_base, api_key, model, &prompt).await,
    };

    match result {
        Ok(text) => {
            let summaries = parse_response(&text, count);
            for (sym, summary) in symbols.iter_mut().zip(summaries.iter()) {
                if !summary.is_empty() {
                    sym.summary = summary.clone();
                } else {
                    sym.summary = signature_fallback(sym);
                }
            }
        }
        Err(e) => {
            warn!("AI summarization failed: {e}");
            for sym in symbols.iter_mut() {
                if sym.summary.is_empty() {
                    sym.summary = signature_fallback(sym);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Tier 1 + Tier 3 only: docstring extraction then signature fallback.
/// No AI or network calls.
pub fn summarize_symbols_simple(symbols: &mut [Symbol]) {
    for sym in symbols.iter_mut() {
        if !sym.summary.is_empty() {
            continue;
        }

        if !sym.docstring.is_empty() {
            sym.summary = extract_summary_from_docstring(&sym.docstring);
        }

        if sym.summary.is_empty() {
            sym.summary = signature_fallback(sym);
        }
    }
}

/// Full three-tier summarization pipeline.
///
/// Tier 1: Docstring extraction (free).
/// Tier 2: AI batch summarization (Anthropic → Gemini → local LLM).
/// Tier 3: Signature fallback for any remaining gaps.
pub async fn summarize_symbols(symbols: &mut Vec<Symbol>, use_ai: bool) {
    // --- Tier 1: extract from existing docstrings ---
    for sym in symbols.iter_mut() {
        if !sym.docstring.is_empty() && sym.summary.is_empty() {
            sym.summary = extract_summary_from_docstring(&sym.docstring);
        }
    }

    // --- Tier 2: AI batch summarization ---
    if use_ai {
        if let Some(provider) = detect_provider() {
            let provider_name = match &provider {
                Provider::Anthropic { .. } => "Anthropic Claude Haiku",
                Provider::Gemini { .. } => "Google Gemini Flash",
                Provider::OpenAI { model, .. } => model.as_str(),
            };
            info!("Using AI summarizer: {provider_name}");

            let timeout = std::env::var("OPENAI_TIMEOUT")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(60);

            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(timeout))
                .build()
                .unwrap_or_default();

            // Collect indices of symbols that still need summarization.
            let needs_summary: Vec<usize> = symbols
                .iter()
                .enumerate()
                .filter(|(_, s)| s.summary.is_empty() && s.docstring.is_empty())
                .map(|(i, _)| i)
                .collect();

            let batch_size = 10;
            for chunk in needs_summary.chunks(batch_size) {
                // Gather mutable references to the symbols in this batch.
                // We need to work around the borrow checker since we can't have
                // multiple mutable references into the same Vec simultaneously
                // through normal indexing. Use unsafe-free index swapping approach.
                let mut batch_symbols: Vec<Symbol> = chunk
                    .iter()
                    .map(|&i| std::mem::take(&mut symbols[i]))
                    .collect();

                let mut refs: Vec<&mut Symbol> = batch_symbols.iter_mut().collect();
                summarize_batch_ai(&provider, &client, &mut refs).await;

                // Put them back.
                for (&idx, sym) in chunk.iter().zip(batch_symbols.into_iter()) {
                    symbols[idx] = sym;
                }
            }
        }
    }

    // --- Tier 3: signature fallback for any remaining gaps ---
    for sym in symbols.iter_mut() {
        if sym.summary.is_empty() {
            sym.summary = signature_fallback(sym);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sym(kind: &str, name: &str, sig: &str, docstring: &str) -> Symbol {
        Symbol {
            id: format!("test::{name}#{kind}"),
            file: "test.py".to_string(),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: kind.to_string(),
            language: "python".to_string(),
            signature: sig.to_string(),
            docstring: docstring.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_extract_summary_from_docstring_simple() {
        assert_eq!(
            extract_summary_from_docstring("Do something cool.\n\nMore details here."),
            "Do something cool."
        );
    }

    #[test]
    fn test_extract_summary_from_docstring_no_period() {
        assert_eq!(
            extract_summary_from_docstring("Do something cool"),
            "Do something cool"
        );
    }

    #[test]
    fn test_extract_summary_from_docstring_empty() {
        assert_eq!(extract_summary_from_docstring(""), "");
        assert_eq!(extract_summary_from_docstring("   "), "");
    }

    #[test]
    fn test_signature_fallback_function() {
        let sym = make_sym("function", "foo", "def foo(x: int) -> str:", "");
        assert_eq!(signature_fallback(&sym), "def foo(x: int) -> str:");
    }

    #[test]
    fn test_signature_fallback_class() {
        let sym = make_sym("class", "MyClass", "class MyClass(Base):", "");
        assert_eq!(signature_fallback(&sym), "Class MyClass");
    }

    #[test]
    fn test_signature_fallback_constant() {
        let sym = make_sym("constant", "MAX_SIZE", "MAX_SIZE = 100", "");
        assert_eq!(signature_fallback(&sym), "Constant MAX_SIZE");
    }

    #[test]
    fn test_signature_fallback_type() {
        let sym = make_sym("type", "UserID", "type UserID = int", "");
        assert_eq!(signature_fallback(&sym), "Type definition UserID");
    }

    #[test]
    fn test_simple_summarize_uses_docstring() {
        let mut symbols = vec![make_sym(
            "function",
            "foo",
            "def foo():",
            "Does something useful.",
        )];
        summarize_symbols_simple(&mut symbols);
        assert_eq!(symbols[0].summary, "Does something useful.");
    }

    #[test]
    fn test_simple_summarize_fallback_to_signature() {
        let mut symbols = vec![make_sym(
            "function",
            "foo",
            "def foo(x: int) -> str:",
            "",
        )];
        summarize_symbols_simple(&mut symbols);
        assert!(symbols[0].summary.contains("def foo"));
    }

    #[test]
    fn test_simple_summarize_preserves_existing() {
        let mut sym = make_sym("function", "foo", "def foo():", "");
        sym.summary = "Already summarized".to_string();
        let mut symbols = vec![sym];
        summarize_symbols_simple(&mut symbols);
        assert_eq!(symbols[0].summary, "Already summarized");
    }

    #[test]
    fn test_parse_response_numbered() {
        let text = "1. Authenticates users with credentials.\n2. Fetches user data from database.";
        let result = parse_response(text, 2);
        assert_eq!(result[0], "Authenticates users with credentials.");
        assert_eq!(result[1], "Fetches user data from database.");
    }

    #[test]
    fn test_parse_response_missing_numbers() {
        let text = "1. First summary.\n3. Third summary.";
        let result = parse_response(text, 3);
        assert_eq!(result[0], "First summary.");
        assert_eq!(result[1], ""); // Missing
        assert_eq!(result[2], "Third summary.");
    }

    #[test]
    fn test_parse_response_empty() {
        let result = parse_response("", 3);
        assert_eq!(result.len(), 3);
        assert!(result.iter().all(|s| s.is_empty()));
    }
}
