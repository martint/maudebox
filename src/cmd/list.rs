use crate::cmd::run::instance_handle;
use crate::docker;
use crate::manifest;
use crate::volume::compute_state_dir;
use anyhow::Result;
use std::path::PathBuf;

#[derive(Debug, Default)]
struct VolumeRow {
    project: String,
    instance: String,
    ephemeral: String,
}

#[derive(Debug, Default)]
struct ContainerRow {
    project: String,
    id: String,
    instance: String,
    ephemeral: String,
}

#[derive(Debug, Default)]
struct ManifestRow {
    project: String,
    instance: String,
}

// An instance is a `(project, instance-handle)` pair: anything currently
// running, with at least one project volume on disk, OR created by `maudebox
// new` (a persisted manifest) shows up. Keying on the pair rather than the
// project alone keeps `--instance` variants — several containers against one
// project, distinguished only by their `maudebox.instance` label — as separate
// rows instead of collapsing them. The three sources are independent: a
// Older `new`-created workspaces may have no volume, and once their container
// exits `docker ps` shows nothing, so their manifest is the only record left.
// The instance handle comes straight from each source's label (the
// manifest reconstructs it from the target basename and recorded discriminator).
//
// An ephemeral instance whose keep flag is set is shown as non-ephemeral,
// since at that point the workspace will outlive the container.
pub fn run() -> Result<i32> {
    let vol_fmt = "{{.Label \"maudebox.project\"}}\t{{.Label \"maudebox.instance\"}}\t{{.Label \"maudebox.ephemeral\"}}";
    let ps_fmt = "{{.Label \"maudebox.project\"}}\t{{.ID}}\t{{.Label \"maudebox.instance\"}}\t{{.Label \"maudebox.ephemeral\"}}";

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
        "name=maudebox-overlay-",
        "--format",
        vol_fmt,
    ])?;
    let volumes: Vec<VolumeRow> = vol_out
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut it = line.split('\t');
            VolumeRow {
                project: it.next().unwrap_or("").to_string(),
                instance: it.next().unwrap_or("").to_string(),
                ephemeral: it.next().unwrap_or("").to_string(),
            }
        })
        .collect();
    let running: Vec<ContainerRow> = ps_out
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let mut it = l.split('\t');
            ContainerRow {
                project: it.next().unwrap_or("").to_string(),
                id: it.next().unwrap_or("").to_string(),
                instance: it.next().unwrap_or("").to_string(),
                ephemeral: it.next().unwrap_or("").to_string(),
            }
        })
        .collect();

    // `maudebox new` instances: the manifest's `target` is the worktree (and
    // thus the project) path. Skip legacy manifests with no target and any
    // whose workspace has since been removed out-of-band. The full instance
    // handle is reconstructed from the target basename and the recorded
    // `--instance` discriminator, exactly as `run` builds the label;
    // volume/container labels still override it in the merge below.
    let manifests: Vec<ManifestRow> = manifest::all()?
        .into_iter()
        .filter_map(|(_state_dir, m)| {
            if m.target.is_empty() {
                return None;
            }
            let target = PathBuf::from(&m.target);
            if !target.is_dir() {
                return None;
            }
            let basename = target
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            Some(ManifestRow {
                instance: instance_handle(&basename, &m.instance),
                project: m.target,
            })
        })
        .collect();

    // An instance is identified by its `(project, instance-handle)` pair, not the
    // project path alone: `--instance` lets several containers run against one
    // project, each with a distinct `maudebox.instance` label but the *same*
    // `maudebox.project`. Keying on project alone collapses them into one row and
    // hides every `--instance` variant behind the first. Overlay volumes and the
    // state dir are per-project (shared across a project's instances), so the
    // OVERLAYS count below is still counted by project.
    let mut keys: Vec<(String, String)> = volumes
        .iter()
        .map(|v| (v.project.clone(), v.instance.clone()))
        .chain(
            running
                .iter()
                .map(|r| (r.project.clone(), r.instance.clone())),
        )
        .chain(
            manifests
                .iter()
                .map(|m| (m.project.clone(), m.instance.clone())),
        )
        .filter(|(p, _)| !p.is_empty())
        .collect();
    keys.sort();
    keys.dedup();

    if keys.is_empty() {
        println!("No maudebox instances found.");
        return Ok(0);
    }

    let mut rows: Vec<Vec<String>> = vec![vec![
        "ID".into(),
        "STATUS".into(),
        "INSTANCE".into(),
        "EPHEMERAL".into(),
        "OVERLAYS".into(),
        "PATH".into(),
    ]];

    for (project, instance) in &keys {
        let vol = volumes
            .iter()
            .find(|v| &v.project == project && &v.instance == instance);
        let run = running
            .iter()
            .find(|r| &r.project == project && &r.instance == instance);
        let instance = instance.clone();
        let mut ephemeral = vol
            .map(|v| v.ephemeral.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| run.map(|r| r.ephemeral.clone()))
            .unwrap_or_default();
        let id = run.map(|r| r.id.clone()).unwrap_or_default();
        let overlays = volumes.iter().filter(|v| &v.project == project).count();

        if ephemeral == "true" {
            let state_dir = compute_state_dir(&PathBuf::from(project))?;
            if state_dir.join("keep").exists() {
                ephemeral = "false".into();
            }
        }

        let id_disp = if id.is_empty() {
            "-".to_string()
        } else {
            id.clone()
        };
        let status = if id.is_empty() { "stopped" } else { "running" };
        rows.push(vec![
            id_disp,
            status.into(),
            if instance.is_empty() {
                "?".into()
            } else {
                instance
            },
            if ephemeral.is_empty() {
                "-".into()
            } else {
                ephemeral
            },
            overlays.to_string(),
            project.clone(),
        ]);
    }

    print_table(&rows);
    Ok(0)
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
