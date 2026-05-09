use crate::docker;
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

// An "instance" is a project: anything currently running OR with at least
// one overlay volume on disk shows up. Volume labels are authoritative when
// available — they survive container exit, so a stopped instance still has
// full metadata. Container labels fill in when there's no volume yet.
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
        .filter(|l| !l.is_empty())
        .map(|l| {
            let mut it = l.split('\t');
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

    let mut projects: Vec<String> = volumes
        .iter()
        .map(|v| v.project.clone())
        .chain(running.iter().map(|r| r.project.clone()))
        .filter(|p| !p.is_empty())
        .collect();
    projects.sort();
    projects.dedup();

    if projects.is_empty() {
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

    for project in &projects {
        let vol = volumes.iter().find(|v| &v.project == project);
        let run = running.iter().find(|r| &r.project == project);
        let instance = vol
            .map(|v| v.instance.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| run.map(|r| r.instance.clone()))
            .unwrap_or_default();
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

        let id_disp = if id.is_empty() { "-".to_string() } else { id.clone() };
        let status = if id.is_empty() { "stopped" } else { "running" };
        rows.push(vec![
            id_disp,
            status.into(),
            if instance.is_empty() { "?".into() } else { instance },
            if ephemeral.is_empty() { "-".into() } else { ephemeral },
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
