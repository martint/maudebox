use anyhow::{bail, Context, Result};
use std::process::{Command, Stdio};

// Capture stdout from a `docker <args>` invocation. Errors include captured
// stderr so the user sees the actual docker failure.
pub fn capture(args: &[&str]) -> Result<String> {
    let output = Command::new("docker")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("spawning docker")?;
    if !output.status.success() {
        bail!(
            "docker {} failed ({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// Inherit stdio for the foreground container — this is the long-running
// `docker run -it` invocation. Returns the container's exit code so the
// wrapper can propagate it.
pub fn run_inherit(args: &[String]) -> Result<i32> {
    let status = Command::new("docker")
        .args(args)
        .status()
        .context("spawning docker")?;
    Ok(status.code().unwrap_or(1))
}

// `docker volume create` — idempotent; takes label strings as `key=value`.
pub fn ensure_volume(name: &str, labels: &[String]) -> Result<()> {
    let mut args: Vec<&str> = vec!["volume", "create"];
    for l in labels {
        args.push("--label");
        args.push(l);
    }
    args.push(name);
    capture(&args)?;
    Ok(())
}

// One-shot mkdir inside a throwaway container so volume-subpath mounts have
// the expected sub-directory structure on first use.
pub fn ensure_subpaths(image: &str, volume: &str, subpaths: &[&str]) -> Result<()> {
    let mut args: Vec<String> = ["run", "--rm", "--entrypoint", "mkdir", "-v"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    args.push(format!("{volume}:/v"));
    args.push(image.to_string());
    args.push("-p".to_string());
    for s in subpaths {
        args.push(format!("/v/{s}"));
    }
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    capture(&refs)?;
    Ok(())
}
