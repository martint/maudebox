use crate::paths::{config_path, expand_host_tilde};
use anyhow::{Context, Result};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

// We use a real TOML parser on the read side (improvement over the bash
// version's hand-rolled subset) but keep the comment-preserving line-
// streaming approach on the write side so user comments don't disappear.

pub fn read_mounts() -> Result<Vec<String>> {
    let path = config_path()?;
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context(format!("reading {}", path.display())),
    };
    let v: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let arr = match v.get("mounts").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return Ok(Vec::new()),
    };
    Ok(arr
        .iter()
        .filter_map(|x| x.as_str().map(|s| s.to_string()))
        .collect())
}

// Directories searched when a bare name is given instead of a path (to
// `maudebox <name>` or `maudebox new --source <name>`). Each entry is
// host-tilde-expanded so `~/Projects` resolves against the host home.
pub fn read_project_roots() -> Result<Vec<String>> {
    let path = config_path()?;
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context(format!("reading {}", path.display())),
    };
    let v: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let arr = match v.get("project-roots").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return Ok(Vec::new()),
    };
    arr.iter()
        .filter_map(|x| x.as_str())
        .map(expand_host_tilde)
        .collect()
}

// MCP server definitions live in `[mcp.NAME]` tables and map 1:1 to the
// `mcpServers` object of Claude Code's managed-MCP JSON. We pass each entry
// through unchanged — Claude validates the shape itself, and the set of valid
// fields (type, url, command, args, env, headers, …) changes over time.
// Returns None when no `[mcp]` table is present (caller skips the whole code
// path), Some(Map) otherwise. BTreeMap gives stable iteration order for
// reproducible JSON output.
pub fn read_mcp_servers() -> Result<Option<std::collections::BTreeMap<String, toml::Value>>> {
    let path = config_path()?;
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context(format!("reading {}", path.display())),
    };
    let v: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let table = match v.get("mcp").and_then(|v| v.as_table()) {
        Some(t) => t,
        None => return Ok(None),
    };
    if table.is_empty() {
        return Ok(None);
    }
    let mut out = std::collections::BTreeMap::new();
    for (k, val) in table {
        out.insert(k.clone(), val.clone());
    }
    Ok(Some(out))
}

// Aliases preserve insertion order in the file. Use a Vec of (name, value)
// rather than BTreeMap so listing order matches what the user wrote.
pub fn read_aliases() -> Result<Vec<(String, String)>> {
    let path = config_path()?;
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context(format!("reading {}", path.display())),
    };
    let v: toml::Value = toml::from_str(&text)
        .with_context(|| format!("parsing {}", path.display()))?;
    let table = match v.get("aliases").and_then(|v| v.as_table()) {
        Some(t) => t,
        None => return Ok(Vec::new()),
    };
    let mut out: Vec<(String, String)> = Vec::with_capacity(table.len());
    for (k, val) in table {
        if let Some(s) = val.as_str() {
            out.push((k.clone(), s.to_string()));
        }
    }
    Ok(out)
}

fn emit_mounts_block(specs: &[String]) -> String {
    if specs.is_empty() {
        return "mounts = []\n".to_string();
    }
    let mut out = String::from("mounts = [\n");
    for s in specs {
        out.push_str(&format!("    \"{s}\",\n"));
    }
    out.push_str("]\n");
    out
}

fn escape_toml_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn emit_aliases_block(entries: &[(String, String)]) -> String {
    let mut out = String::from("[aliases]\n");
    for (name, value) in entries {
        out.push_str(&format!("{name} = \"{}\"\n", escape_toml_string(value)));
    }
    out
}

fn write_atomic(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    // Append `.tmp.<pid>` rather than calling `with_extension`, which would
    // replace the existing `.toml` suffix.
    let tmp = std::path::PathBuf::from(format!(
        "{}.tmp.{}",
        path.display(),
        std::process::id()
    ));
    fs::write(&tmp, content).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} to {}", tmp.display(), path.display()))?;
    Ok(())
}

// Replace just the `mounts = [...]` block, keep every other line of the
// file untouched. Multi-line array support: once we hit `mounts = [`, swallow
// input lines until the closing `]`. If no mounts assignment exists at all,
// append at the end.
pub fn write_mounts(specs: &[String]) -> Result<()> {
    let path = config_path()?;
    let existing = match fs::read_to_string(&path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => return Err(e).context(format!("reading {}", path.display())),
    };

    let Some(existing) = existing else {
        return write_atomic(&path, &emit_mounts_block(specs));
    };

    let had_trailing = existing.ends_with('\n');
    let lines: Vec<&str> = if had_trailing {
        let mut v: Vec<&str> = existing.split('\n').collect();
        v.pop(); // drop the empty element after the final newline
        v
    } else {
        existing.split('\n').collect()
    };

    let mut out = String::new();
    let mut in_block = false;
    let mut replaced = false;
    for line in lines {
        if !in_block && line_starts_assignment(line, "mounts") {
            out.push_str(&emit_mounts_block(specs));
            replaced = true;
            if line.contains(']') {
                continue; // single-line array: done
            }
            in_block = true;
            continue;
        }
        if in_block {
            if line.contains(']') {
                in_block = false;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        out.push_str(&emit_mounts_block(specs));
    }
    write_atomic(&path, &out)
}

// Replace the `[aliases]` table. The section ends at the next `[section]`
// header (which we then need to print since it belongs to the surrounding
// content) or at EOF.
pub fn write_aliases(entries: &[(String, String)]) -> Result<()> {
    let path = config_path()?;
    let existing = match fs::read_to_string(&path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => return Err(e).context(format!("reading {}", path.display())),
    };

    let Some(existing) = existing else {
        return write_atomic(&path, &emit_aliases_block(entries));
    };

    let had_trailing = existing.ends_with('\n');
    let lines: Vec<&str> = if had_trailing {
        let mut v: Vec<&str> = existing.split('\n').collect();
        v.pop();
        v
    } else {
        existing.split('\n').collect()
    };

    let mut out = String::new();
    let mut in_section = false;
    let mut replaced = false;
    for line in lines {
        if !in_section && line.trim() == "[aliases]" {
            out.push_str(&emit_aliases_block(entries));
            replaced = true;
            in_section = true;
            continue;
        }
        if in_section {
            if line.trim_start().starts_with('[') {
                in_section = false;
                out.push_str(line);
                out.push('\n');
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        out.push_str(&emit_aliases_block(entries));
    }
    write_atomic(&path, &out)
}

// Match a line of the form `<key> = [` (with optional whitespace).
fn line_starts_assignment(line: &str, key: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with(key) {
        return false;
    }
    let rest = trimmed[key.len()..].trim_start();
    let rest = match rest.strip_prefix('=') {
        Some(r) => r.trim_start(),
        None => return false,
    };
    rest.starts_with('[')
}

