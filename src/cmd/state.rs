use crate::docker;
use crate::state::{self, DEFAULT_NAMESPACE, DEFAULT_VOLUME};
use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Args)]
pub struct StateArgs {
    #[command(subcommand)]
    pub action: StateAction,
}

#[derive(Subcommand)]
pub enum StateAction {
    /// List persistent state namespaces and running instances using them.
    List,

    /// Remove a namespace, forgetting its credentials, sessions, and state.
    Rm {
        /// Namespace to remove, including `default` for the legacy volume.
        name: String,
    },
}

pub fn run(action: StateAction) -> Result<i32> {
    match action {
        StateAction::List => list(),
        StateAction::Rm { name } => rm(&name),
    }
}

fn list() -> Result<i32> {
    let volumes = discover_volumes()?;
    if volumes.is_empty() {
        println!("No maudebox state namespaces found.");
        return Ok(0);
    }
    let instances = running_instances()?;
    println!("STATE\tINSTANCES\tVOLUME");
    for (namespace, volume) in volumes {
        let names = instances.get(&namespace).cloned().unwrap_or_default();
        println!(
            "{namespace}\t{}\t{volume}",
            if names.is_empty() {
                "-".to_string()
            } else {
                names.into_iter().collect::<Vec<_>>().join(", ")
            }
        );
    }
    Ok(0)
}

fn rm(namespace: &str) -> Result<i32> {
    state::validate_namespace(namespace)?;
    let volume = state::volume_name(namespace);
    let discovered = discover_volumes()?;
    if discovered.get(namespace).map(String::as_str) != Some(volume.as_str()) {
        println!("No state namespace found: {namespace}");
        return Ok(0);
    }

    let users = containers_using(&volume)?;
    if !users.is_empty() {
        bail!(
            "state namespace `{namespace}` is used by: {}\nStop those containers before removing it.",
            users.join(", ")
        );
    }

    println!(
        "Removing state namespace `{namespace}`; its credentials, sessions, and tool state will be forgotten."
    );
    docker::capture(&["volume", "rm", &volume])?;
    println!("Removed volume: {volume}");
    Ok(0)
}

/// Discover labeled namespace volumes plus the unlabeled legacy default. Only
/// accept labeled volumes whose deterministic name matches the label.
fn discover_volumes() -> Result<BTreeMap<String, String>> {
    let format = "{{.Name}}\t{{.Label \"maudebox.state\"}}";
    let out = docker::capture(&["volume", "ls", "--format", format])?;
    Ok(parse_volumes(&out))
}

fn parse_volumes(out: &str) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    for line in out.lines() {
        let mut fields = line.splitn(2, '\t');
        let volume = fields.next().unwrap_or("");
        let label = fields.next().unwrap_or("");
        if volume == DEFAULT_VOLUME {
            result.insert(DEFAULT_NAMESPACE.to_string(), volume.to_string());
        } else if !label.is_empty()
            && state::validate_namespace(label).is_ok()
            && state::volume_name(label) == volume
        {
            result.insert(label.to_string(), volume.to_string());
        }
    }
    result
}

fn running_instances() -> Result<BTreeMap<String, BTreeSet<String>>> {
    let format = "{{.Label \"maudebox.state\"}}\t{{.Label \"maudebox.instance\"}}";
    let out = docker::capture(&[
        "ps",
        "--filter",
        "label=maudebox.instance",
        "--format",
        format,
    ])?;
    Ok(parse_running_instances(&out))
}

fn parse_running_instances(out: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut result: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for line in out.lines() {
        let mut fields = line.splitn(2, '\t');
        let namespace = fields.next().unwrap_or("");
        let instance = fields.next().unwrap_or("");
        let namespace = if namespace.is_empty() {
            DEFAULT_NAMESPACE
        } else {
            namespace
        };
        if !instance.is_empty() {
            result
                .entry(namespace.to_string())
                .or_default()
                .insert(instance.to_string());
        }
    }
    result
}

fn containers_using(volume: &str) -> Result<Vec<String>> {
    let filter = format!("volume={volume}");
    let out = docker::capture(&["ps", "-a", "--filter", &filter, "--format", "{{.Names}}"])?;
    let mut names: Vec<String> = out
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_includes_legacy_default_and_owned_named_volumes() {
        let rows = parse_volumes(
            "maudebox-state\t\nmaudebox-state-work\twork\nmaudebox-state-other\t\nmaudebox-state-wrong\twork\nunrelated\tpersonal\n",
        );
        assert_eq!(
            rows.get("default").map(String::as_str),
            Some("maudebox-state")
        );
        assert_eq!(
            rows.get("work").map(String::as_str),
            Some("maudebox-state-work")
        );
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn legacy_containers_are_reported_in_default_namespace() {
        let rows = parse_running_instances("\told\nwork\tnew\nwork\treviewer\n");
        assert!(rows["default"].contains("old"));
        assert_eq!(rows["work"].len(), 2);
    }
}
