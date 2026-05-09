use crate::docker;
use anyhow::Result;
use std::fs;
use std::path::PathBuf;

// Find a running ephemeral container by container ID, instance basename, or
// the original `maudebox new <name>` name, then drop the `keep` flag in its
// state dir. A no-op for non-ephemeral instances.
pub fn run(target: &str) -> Result<i32> {
    let fmt = "{{.ID}}\t{{.Label \"maudebox.instance\"}}\t{{.Label \"maudebox.ephemeral-name\"}}\t{{.Label \"maudebox.ephemeral\"}}\t{{.Label \"maudebox.state-dir\"}}\t{{.Label \"maudebox.project\"}}";

    let out = docker::capture(&[
        "ps",
        "--filter",
        "label=maudebox.instance",
        "--format",
        fmt,
    ])?;

    struct Match {
        id: String,
        instance: String,
        ename: String,
        ephemeral: String,
        state_dir: String,
        project: String,
    }

    let matches: Vec<Match> = out
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let mut it = l.split('\t');
            Match {
                id: it.next().unwrap_or("").to_string(),
                instance: it.next().unwrap_or("").to_string(),
                ename: it.next().unwrap_or("").to_string(),
                ephemeral: it.next().unwrap_or("").to_string(),
                state_dir: it.next().unwrap_or("").to_string(),
                project: it.next().unwrap_or("").to_string(),
            }
        })
        .filter(|m| m.id == target || m.instance == target || m.ename == target)
        .collect();

    if matches.is_empty() {
        eprintln!("No running maudebox container matches '{target}'.");
        return Ok(1);
    }
    if matches.len() > 1 {
        eprintln!("Multiple running maudebox containers match '{target}':");
        for m in &matches {
            eprintln!("  {}  {}  ({})", m.id, m.instance, m.project);
        }
        return Ok(1);
    }

    let m = &matches[0];
    if m.ephemeral != "true" {
        println!("Instance '{}' is not ephemeral; nothing to keep.", m.instance);
        return Ok(0);
    }
    if m.state_dir.is_empty() {
        eprintln!(
            "Instance '{}' was launched by an older version of maudebox that doesn't support 'keep'; restart the container to enable it.",
            m.instance
        );
        return Ok(1);
    }

    let dir = PathBuf::from(&m.state_dir);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("keep"), "")?;
    println!(
        "Marked '{}' to keep — workspace and overlay volume will survive when the container exits.",
        m.instance
    );
    Ok(0)
}
