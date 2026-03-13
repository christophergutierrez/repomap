use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{bail, Context};

const MARKER: &str = "# Installed by repomap — do not edit";

const HOOK_NAMES: &[&str] = &["post-merge", "post-checkout"];

fn hook_script(repomap_bin: &str, repo_path: &str) -> String {
    format!(
        "#!/bin/sh\n\
         {MARKER}\n\
         \"{repomap_bin}\" index-repo \"{repo_path}\" --incremental --no-ai >/dev/null 2>&1 &\n"
    )
}

/// Install post-merge and post-checkout hooks that trigger incremental reindexing.
pub fn install_hooks(repo_path: &Path) -> anyhow::Result<()> {
    let repo_path = repo_path
        .canonicalize()
        .context("Failed to resolve repository path")?;

    let git_dir = repo_path.join(".git");
    if !git_dir.is_dir() {
        bail!(
            "Not a git repository: {}\n\
             Run this command from inside a git repo, or pass the repo path as an argument.",
            repo_path.display()
        );
    }

    let hooks_dir = git_dir.join("hooks");
    fs::create_dir_all(&hooks_dir).context("Failed to create .git/hooks directory")?;

    let repomap_bin = std::env::current_exe()
        .context("Failed to determine repomap binary path")?
        .canonicalize()
        .context("Failed to resolve repomap binary path")?;
    let repomap_bin_str = repomap_bin.to_string_lossy();
    let repo_path_str = repo_path.to_string_lossy();

    let script = hook_script(&repomap_bin_str, &repo_path_str);

    for hook_name in HOOK_NAMES {
        let hook_path = hooks_dir.join(hook_name);
        fs::write(&hook_path, &script)
            .with_context(|| format!("Failed to write {hook_name} hook"))?;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("Failed to set permissions on {hook_name} hook"))?;
    }

    println!(
        "Installed repomap hooks in {}\n  post-merge\n  post-checkout",
        repo_path.display()
    );
    Ok(())
}

/// Remove repomap-installed hooks (identified by marker comment).
pub fn remove_hooks(repo_path: &Path) -> anyhow::Result<()> {
    let repo_path = repo_path
        .canonicalize()
        .context("Failed to resolve repository path")?;

    let hooks_dir = repo_path.join(".git").join("hooks");
    if !hooks_dir.is_dir() {
        bail!("No .git/hooks directory found at {}", repo_path.display());
    }

    let mut removed = 0u32;

    for hook_name in HOOK_NAMES {
        let hook_path = hooks_dir.join(hook_name);
        if !hook_path.exists() {
            continue;
        }
        match fs::read_to_string(&hook_path) {
            Ok(contents) if contents.contains(MARKER) => {
                fs::remove_file(&hook_path)
                    .with_context(|| format!("Failed to remove {hook_name} hook"))?;
                println!("  Removed {hook_name}");
                removed += 1;
            }
            _ => {
                // Not ours — leave it alone.
            }
        }
    }

    if removed == 0 {
        println!("No repomap hooks found in {}", repo_path.display());
    } else {
        println!(
            "Removed {} repomap hook(s) from {}",
            removed,
            repo_path.display()
        );
    }
    Ok(())
}
