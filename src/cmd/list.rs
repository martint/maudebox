use crate::docker;
use crate::manifest;
use crate::volume::{compute_state_dir, OVERLAY_PREFIX};
use anyhow::Result;
use std::path::PathBuf;

#[derive(Debug, Default)]
struct VolumeRow {
    name: String,
    project: String,
}

#[derive(Debug, Default, Clone)]
struct ContainerRow {
    project: String,
    instance: String,
    ephemeral: String,
    state: String,
}

// A row is a *project*: the worktree path plus whatever maudebox keeps for it
// on disk. Instances are not rows — an instance is a running container, and it
// stops existing when that container exits, so the INSTANCES column reports the
// ones running right now and nothing more.
//
// A project shows up if it is running, still owns a volume, or was created by
// `maudebox new` (a persisted manifest). The three sources are independent: a
// `new`-created workspace may have no volume at all, and once its container
// exits `docker ps` shows nothing, so the manifest is the only record left.
//
// Volumes are project state and carry no instance of their own. Reading one out
// of a volume label used to produce a row claiming that a single, arbitrary
// instance — whichever launch created the volume first, since `docker volume
// create` never relabels — owned the project.
pub fn run() -> Result<i32> {
    let vol_fmt = "{{.Name}}\t{{.Label \"maudebox.project\"}}";
    let ps_fmt = "{{.Label \"maudebox.project\"}}\t{{.Label \"maudebox.instance\"}}\t{{.Label \"maudebox.ephemeral\"}}\t{{.Label \"maudebox.state\"}}";

    let ps_out = docker::capture(&[
        "ps",
        "--filter",
        "label=maudebox.instance",
        "--format",
        ps_fmt,
    ])?;
    let vol_out = docker::capture(&[
        "volume",
        "ls",
        "--filter",
        "label=maudebox.project",
        "--format",
        vol_fmt,
    ])?;

    let volumes = parse_volume_rows(&vol_out);
    let running = parse_container_rows(&ps_out);

    // `maudebox new` projects: the manifest's `target` is the worktree path.
    // Skip legacy manifests with no target and any whose workspace has since
    // been removed out-of-band.
    let manifest_projects: Vec<String> = manifest::all()?
        .into_iter()
        .filter_map(|(_state_dir, m)| {
            if m.target.is_empty() || !PathBuf::from(&m.target).is_dir() {
                return None;
            }
            Some(m.target)
        })
        .collect();

    let mut projects: Vec<String> = volumes
        .iter()
        .map(|v| v.project.clone())
        .chain(running.iter().map(|r| r.project.clone()))
        .chain(manifest_projects)
        .filter(|p| !p.is_empty())
        .collect();
    projects.sort();
    projects.dedup();

    if projects.is_empty() {
        println!("No maudebox projects found.");
        return Ok(0);
    }

    let mut rows: Vec<Vec<String>> = vec![vec![
        "STATUS".into(),
        "INSTANCES".into(),
        "EPHEMERAL".into(),
        "OVERLAYS".into(),
        "PATH".into(),
    ]];

    for project in &projects {
        let here: Vec<&ContainerRow> = running.iter().filter(|r| &r.project == project).collect();
        let overlays = volumes
            .iter()
            .filter(|v| &v.project == project && v.name.starts_with(OVERLAY_PREFIX))
            .count();

        // Ephemeral is a property of a launch, not of the project, so it is
        // only meaningful while a container is up. A keep flag means the
        // workspace will outlive the container, so report it as non-ephemeral.
        let ephemeral = if here.is_empty() {
            "-".to_string()
        } else if here.iter().any(|r| r.ephemeral == "true") {
            let state_dir = compute_state_dir(&PathBuf::from(project))?;
            if state_dir.join("keep").exists() {
                "false".into()
            } else {
                "true".into()
            }
        } else {
            "false".into()
        };

        let instances = format_instances(&here);

        rows.push(vec![
            if here.is_empty() {
                "stopped"
            } else {
                "running"
            }
            .into(),
            instances,
            ephemeral,
            overlays.to_string(),
            project.clone(),
        ]);
    }

    print_table(&rows);
    Ok(0)
}

fn format_instances(rows: &[&ContainerRow]) -> String {
    if rows.is_empty() {
        return "-".to_string();
    }
    let mut names: Vec<String> = rows
        .iter()
        .map(|r| {
            let state = if r.state.is_empty() {
                crate::state::DEFAULT_NAMESPACE
            } else {
                &r.state
            };
            format!("{} [{state}]", r.instance)
        })
        .collect();
    names.sort_unstable();
    names.join(", ")
}

fn parse_volume_rows(out: &str) -> Vec<VolumeRow> {
    out.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut it = line.split('\t');
            VolumeRow {
                name: it.next().unwrap_or("").to_string(),
                project: it.next().unwrap_or("").to_string(),
            }
        })
        .collect()
}

fn parse_container_rows(out: &str) -> Vec<ContainerRow> {
    out.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut it = line.split('\t');
            ContainerRow {
                project: it.next().unwrap_or("").to_string(),
                instance: it.next().unwrap_or("").to_string(),
                ephemeral: it.next().unwrap_or("").to_string(),
                state: it.next().unwrap_or("").to_string(),
            }
        })
        .collect()
}

fn print_table(rows: &[Vec<String>]) {
    let cols = rows.first().map(|r| r.len()).unwrap_or(0);
    let mut widths = vec![0usize; cols];
    for row in rows {
        for (i, c) in row.iter().enumerate() {
            if i < cols && c.len() > widths[i] {
                widths[i] = c.len();
            }
        }
    }
    for row in rows {
        let mut line = String::new();
        for (i, c) in row.iter().enumerate() {
            if i + 1 < cols {
                line.push_str(&format!("{c:width$}", width = widths[i]));
                line.push_str("  ");
            } else {
                line.push_str(c);
            }
        }
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Two containers against one project is exactly what `--instance` exists
    // for, and both belong on that project's single row.
    #[test]
    fn concurrent_instances_share_one_project_row() {
        let rows = parse_container_rows(
            "/src/trino\ttrino\tfalse\tdefault\n/src/trino\ttrino-review\tfalse\twork\n/src/other\tother\tfalse\tdefault\n",
        );
        let here: Vec<&str> = rows
            .iter()
            .filter(|r| r.project == "/src/trino")
            .map(|r| r.instance.as_str())
            .collect();
        assert_eq!(here, ["trino", "trino-review"]);
        assert_eq!(rows[1].state, "work");
        let trino: Vec<&ContainerRow> = rows
            .iter()
            .filter(|row| row.project == "/src/trino")
            .collect();
        assert_eq!(
            format_instances(&trino),
            "trino [default], trino-review [work]"
        );

        let legacy = parse_container_rows("/src/old\told\tfalse\t\n");
        assert_eq!(format_instances(&[&legacy[0]]), "old [default]");
    }

    // Only overlay volumes feed the OVERLAYS count; any other project-labelled
    // volume (a leftover from an older maudebox, say) must not inflate it.
    #[test]
    fn overlays_are_counted_by_name_not_by_project_label_alone() {
        let rows = parse_volume_rows(
            "maudebox-overlay-proj-6b166a34-fee00b7a\t/src/proj\n\
             maudebox-codex-proj-proj-review-6b166a34\t/src/proj\n",
        );
        let overlays = rows
            .iter()
            .filter(|v| v.project == "/src/proj" && v.name.starts_with(OVERLAY_PREFIX))
            .count();
        assert_eq!(overlays, 1);
        assert_eq!(rows.len(), 2);
    }
}
