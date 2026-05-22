// Resolve a CLI project/source argument to a host directory path.
//
// Shared by the default-run path (`maudebox <name>`) and `maudebox new
// --source <name>` so a bare identifier means the same thing everywhere.

use crate::config::read_project_roots;
use crate::manifest;
use crate::paths::looks_like_path;
use anyhow::{bail, Result};
use std::path::PathBuf;

/// Resolve a project argument to a directory path string.
///
/// Path-shaped inputs (a leading `/`, `.`, `~`, or an embedded `/`) pass
/// through untouched — they're handed straight to the downstream
/// canonicalize step. A bare identifier is looked up in two stores, in
/// order:
///
/// 1. the manifest store — a workspace `maudebox new` created, matched by
///    name, so `maudebox <name>` reattaches without retyping the path;
/// 2. the configured `project-roots` — the first `<root>/<name>` that is a
///    directory, so a repo can be named instead of pathed.
///
/// A miss falls through to the literal input, preserving the downstream
/// "Not a directory" error for typos. An identifier that matches more than
/// one workspace, or sits under more than one root, is reported as
/// ambiguous rather than guessed.
pub fn resolve_project(input: &str) -> Result<String> {
    if looks_like_path(input) {
        return Ok(input.to_string());
    }

    // 1. An existing maudebox-created workspace by name.
    let matches = manifest::find_by_name(input)?;
    if matches.len() > 1 {
        let mut msg = format!("Multiple workspaces named '{input}':\n");
        for (path, _) in &matches {
            msg.push_str(&format!("  {}\n", path.display()));
        }
        msg.push_str("Re-run with the full path.");
        bail!("{msg}");
    }
    if let Some((path, _)) = matches.into_iter().next() {
        return Ok(path.display().to_string());
    }

    // 2. A directory of that name under a configured project root.
    let hits: Vec<PathBuf> = read_project_roots()?
        .iter()
        .map(|root| PathBuf::from(root).join(input))
        .filter(|p| p.is_dir())
        .collect();
    if hits.len() > 1 {
        let mut msg = format!("'{input}' matches multiple project roots:\n");
        for p in &hits {
            msg.push_str(&format!("  {}\n", p.display()));
        }
        msg.push_str("Re-run with the full path.");
        bail!("{msg}");
    }
    if let Some(p) = hits.into_iter().next() {
        return Ok(p.display().to_string());
    }

    // 3. Fall through to the literal input.
    Ok(input.to_string())
}
