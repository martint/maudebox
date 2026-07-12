use crate::paths::{expand_container_tilde, expand_host_tilde};
use anyhow::{anyhow, bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Ro,
    Rw,
    Overlay,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Ro => "ro",
            Mode::Rw => "rw",
            Mode::Overlay => "overlay",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MountSpec {
    pub src: String,
    pub dst: String,
    pub mode: Mode,
}

// Parse a HOST:CONTAINER[:MODE] spec and validate each field. Empty src/dst,
// an unknown mode, or extra ':'-separated fields would otherwise fall
// through to docker (or be silently persisted by `mount add`) and the
// failure would surface far from the typo.
pub fn parse_mount_spec(spec: &str) -> Result<MountSpec> {
    let parts: Vec<&str> = spec.split(':').collect();
    if parts.len() < 2 {
        bail!("Invalid mount spec: '{spec}' (expected HOST:CONTAINER[:ro|rw|overlay])");
    }
    if parts.len() > 3 {
        bail!("Invalid mount spec: '{spec}' (too many ':'-separated fields)");
    }
    let src = parts[0];
    let dst = parts[1];
    if src.is_empty() || dst.is_empty() {
        bail!("Invalid mount spec: '{spec}' (expected HOST:CONTAINER[:ro|rw|overlay])");
    }
    let mode_str = parts.get(2).copied().unwrap_or("rw");
    let mode = match mode_str {
        "ro" => Mode::Ro,
        "rw" => Mode::Rw,
        "overlay" => Mode::Overlay,
        other => {
            return Err(anyhow!(
                "Invalid mount mode: '{other}' in '{spec}' (expected ro, rw, or overlay)"
            ))
        }
    };
    Ok(MountSpec {
        src: src.to_string(),
        dst: dst.to_string(),
        mode,
    })
}

// Canonical key for spec equality. Two specs that mean the same thing should
// produce the same key — so `~/x:~/x` (mode defaults to rw) and `~/x:~/x:rw`
// collide on add/rm. Malformed specs round-trip as themselves.
pub fn mount_spec_key(spec: &str) -> String {
    match parse_mount_spec(spec) {
        Ok(m) => format!("{}:{}:{}", m.src, m.dst, m.mode.as_str()),
        Err(_) => spec.to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct Overlay {
    pub host_src: String,
    pub container_dst: String,
}

#[derive(Debug, Default)]
pub struct MountPlan {
    pub mount_args: Vec<String>,
    pub overlays: Vec<Overlay>,
}

// Resolve every spec in a launch: build the docker `-v` args for ro/rw mounts
// and an ordered list of overlays the launch step materializes into per-
// project Docker volumes.
pub fn build_mount_plan(specs: &[String]) -> Result<MountPlan> {
    let mut plan = MountPlan::default();
    for spec in specs {
        let m = parse_mount_spec(spec)?;
        let src = expand_host_tilde(&m.src)?;
        let dst = expand_container_tilde(&m.dst);

        match m.mode {
            Mode::Overlay => {
                if let Some(dup) = plan.overlays.iter().find(|o| o.container_dst == dst) {
                    bail!(
                        "Duplicate overlay target: {dst} (sources: {} and {src})",
                        dup.host_src
                    );
                }
                plan.overlays.push(Overlay {
                    host_src: src,
                    container_dst: dst,
                });
            }
            Mode::Ro | Mode::Rw => {
                plan.mount_args.push("-v".into());
                plan.mount_args
                    .push(format!("{src}:{dst}:{}", m.mode.as_str()));
            }
        }
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modes_and_defaults_to_rw() {
        assert_eq!(parse_mount_spec("/a:/b").unwrap().mode, Mode::Rw);
        assert_eq!(parse_mount_spec("/a:/b:ro").unwrap().mode, Mode::Ro);
        assert_eq!(
            parse_mount_spec("/a:/b:overlay").unwrap().mode,
            Mode::Overlay
        );
    }

    #[test]
    fn rejects_malformed_specs() {
        for spec in ["/a", ":/b", "/a:", "/a:/b:nope", "/a:/b:rw:extra"] {
            assert!(parse_mount_spec(spec).is_err(), "accepted {spec}");
        }
    }

    #[test]
    fn canonical_key_normalizes_default_mode() {
        assert_eq!(mount_spec_key("~/a:~/b"), mount_spec_key("~/a:~/b:rw"));
    }

    #[test]
    fn duplicate_overlay_targets_are_rejected() {
        let specs = vec!["/a:/cache:overlay".into(), "/b:/cache:overlay".into()];
        assert!(build_mount_plan(&specs)
            .unwrap_err()
            .to_string()
            .contains("Duplicate"));
    }
}
