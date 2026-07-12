use crate::paths::{config_path, expand_host_tilde};
use anyhow::{bail, Context, Result};
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
    let arr = match v.get("mounts") {
        Some(v) => v
            .as_array()
            .context("`mounts` must be an array of strings")?,
        None => return Ok(Vec::new()),
    };
    string_array(arr, "mounts")
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
    let arr = match v.get("project-roots") {
        Some(v) => v
            .as_array()
            .context("`project-roots` must be an array of strings")?,
        None => return Ok(Vec::new()),
    };
    string_array(arr, "project-roots")?
        .iter()
        .map(|s| expand_host_tilde(s))
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
    let table = match v.get("mcp") {
        Some(v) => v.as_table().context("`mcp` must be a table")?,
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
    let v: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let table = match v.get("aliases") {
        Some(v) => v.as_table().context("`aliases` must be a table")?,
        None => return Ok(Vec::new()),
    };
    let mut out: Vec<(String, String)> = Vec::with_capacity(table.len());
    for (k, val) in table {
        if !valid_alias_name(k) {
            bail!(
                "invalid alias name `{k}`: expected a letter or '_' followed by letters, digits, '_', or '-'"
            );
        }
        let s = val
            .as_str()
            .with_context(|| format!("`aliases.{k}` must be a string"))?;
        out.push((k.clone(), s.to_string()));
    }
    Ok(out)
}

fn valid_alias_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

// The container command launched when none is given on the CLI — applies
// to plain `maudebox` and `maudebox new` alike. An array so a multi-word
// command needs no shell-splitting; empty (the default) leaves the image's
// own CMD, an interactive shell, in place.
pub fn read_default_command() -> Result<Vec<String>> {
    let path = config_path()?;
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context(format!("reading {}", path.display())),
    };
    let v: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let arr = match v.get("default-command") {
        Some(v) => v
            .as_array()
            .context("`default-command` must be an array of strings")?,
        None => return Ok(Vec::new()),
    };
    string_array(arr, "default-command")
}

/// Read the optional `network` key — one or more Docker network names that
/// maudebox containers join at launch (`docker run --network`, repeatable).
/// Lets containers reach a service running under its own compose project
/// without publishing any port to the host: attach to the compose-created
/// network and address the service by its compose service name. Accepts
/// either a bare string (single network) or an array of strings (several,
/// e.g. spanning two compose stacks). Returns an empty vec when unset.
pub fn read_network() -> Result<Vec<String>> {
    let path = config_path()?;
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context(format!("reading {}", path.display())),
    };
    let v: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(match v.get("network") {
        Some(toml::Value::String(s)) => vec![s.clone()],
        Some(toml::Value::Array(a)) => string_array(a, "network")?,
        Some(_) => bail!("`network` must be a string or an array of strings"),
        None => Vec::new(),
    })
}

fn string_array(arr: &[toml::Value], key: &str) -> Result<Vec<String>> {
    arr.iter()
        .enumerate()
        .map(|(i, value)| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .with_context(|| format!("`{key}[{i}]` must be a string"))
        })
        .collect()
}

fn emit_mounts_block(specs: &[String]) -> String {
    if specs.is_empty() {
        return "mounts = []\n".to_string();
    }
    let mut out = String::from("mounts = [\n");
    for s in specs {
        out.push_str(&format!("    \"{}\",\n", escape_toml_string(s)));
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
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    // Append `.tmp.<pid>` rather than calling `with_extension`, which would
    // replace the existing `.toml` suffix.
    let tmp = std::path::PathBuf::from(format!("{}.tmp.{}", path.display(), std::process::id()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_block_is_valid_toml_for_quotes_and_backslashes() {
        let specs = vec![r#"/tmp/a\b:/root/\"quoted\":ro"#.to_string()];
        let text = emit_mounts_block(&specs);
        let parsed: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(parsed["mounts"][0].as_str(), Some(specs[0].as_str()));
    }

    #[test]
    fn string_array_rejects_non_strings_with_an_index() {
        let values = vec![toml::Value::String("ok".into()), toml::Value::Integer(2)];
        let error = string_array(&values, "mounts").unwrap_err().to_string();
        assert!(error.contains("mounts[1]"));
    }

    #[test]
    fn assignment_match_does_not_accept_prefixes() {
        assert!(line_starts_assignment(" mounts = []", "mounts"));
        assert!(!line_starts_assignment("mountain = []", "mounts"));
    }
}
