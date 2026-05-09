use crate::docker;
use crate::paths::canonicalize;
use anyhow::Result;

// Remove every overlay volume tied to a project. Volumes carry the
// maudebox.project label as the canonical link. Doesn't validate that the
// project still exists — cleaning orphan volumes for a since-deleted
// worktree by passing the original path is a legitimate use.
pub fn run(project_dir: &str) -> Result<i32> {
    let project_dir = canonicalize(project_dir);

    let label = format!("label=maudebox.project={}", project_dir.display());
    let out = docker::capture(&["volume", "ls", "--filter", &label, "--format", "{{.Name}}"])?;
    let volumes: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
    if volumes.is_empty() {
        println!("No overlay volumes found for: {}", project_dir.display());
        return Ok(0);
    }
    for v in volumes {
        docker::capture(&["volume", "rm", v])?;
        println!("Removed volume: {v}");
    }
    Ok(0)
}
