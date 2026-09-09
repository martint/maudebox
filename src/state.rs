use crate::config::read_state;
use crate::docker;
use anyhow::{bail, Result};

pub const DEFAULT_NAMESPACE: &str = "default";
pub const DEFAULT_VOLUME: &str = "maudebox-state";
const VOLUME_PREFIX: &str = "maudebox-state-";
const MAX_NAME_LEN: usize = 32;

/// Names map one-to-one to Docker volume names. Restricting the public name
/// keeps that mapping reversible and prevents two user-visible namespaces
/// from ever resolving to the same volume.
pub fn validate_namespace(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        bail!("invalid state namespace `{name}`: use 1-{MAX_NAME_LEN} characters");
    }
    let bytes = name.as_bytes();
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        bail!(
            "invalid state namespace `{name}`: use lowercase letters, digits, and single hyphens; start and end with a letter or digit"
        );
    }
    if !bytes[bytes.len() - 1].is_ascii_lowercase() && !bytes[bytes.len() - 1].is_ascii_digit() {
        bail!(
            "invalid state namespace `{name}`: use lowercase letters, digits, and single hyphens; start and end with a letter or digit"
        );
    }
    if bytes
        .iter()
        .any(|b| !b.is_ascii_lowercase() && !b.is_ascii_digit() && *b != b'-')
        || name.contains("--")
    {
        bail!(
            "invalid state namespace `{name}`: use lowercase letters, digits, and single hyphens; start and end with a letter or digit"
        );
    }
    Ok(())
}

pub fn volume_name(namespace: &str) -> String {
    if namespace == DEFAULT_NAMESPACE {
        DEFAULT_VOLUME.to_string()
    } else {
        format!("{VOLUME_PREFIX}{namespace}")
    }
}

/// Resolve CLI-over-config precedence and validate before the caller performs
/// any launch side effects. In particular, `maudebox new` calls this before it
/// creates a workspace.
pub fn select(requested: Option<&str>) -> Result<String> {
    let namespace = match requested {
        Some(name) => name.to_string(),
        None => read_state()?.unwrap_or_else(|| DEFAULT_NAMESPACE.to_string()),
    };
    validate_namespace(&namespace)?;
    Ok(namespace)
}

/// Create a state volume if needed and verify ownership for named namespaces.
/// The legacy default is allowed to be unlabeled because existing installs
/// predate namespace labels. A named volume without the expected label could
/// belong to another application, so refuse to consume it silently.
pub fn ensure_volume(namespace: &str) -> Result<String> {
    validate_namespace(namespace)?;
    let volume = volume_name(namespace);
    let labels = vec![
        "maudebox.kind=state".to_string(),
        format!("maudebox.state={namespace}"),
    ];
    docker::ensure_volume(&volume, &labels)?;

    if namespace != DEFAULT_NAMESPACE {
        let actual = docker::capture(&[
            "volume",
            "inspect",
            "--format",
            "{{index .Labels \"maudebox.state\"}}",
            &volume,
        ])?;
        if actual.trim() != namespace {
            bail!(
                "Docker volume `{volume}` already exists without the expected Maudebox namespace label; rename or remove it before using state namespace `{namespace}`"
            );
        }
    }
    Ok(volume)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_names_are_unambiguous_slugs() {
        for valid in ["default", "work", "personal-2", "0"] {
            validate_namespace(valid).unwrap();
        }
        for invalid in ["", "Work", "work_personal", "-work", "work-", "work--2"] {
            assert!(
                validate_namespace(invalid).is_err(),
                "{invalid} should fail"
            );
        }
    }

    #[test]
    fn default_preserves_legacy_volume_and_named_states_are_distinct() {
        assert_eq!(volume_name("default"), "maudebox-state");
        assert_eq!(volume_name("work"), "maudebox-state-work");
        assert_ne!(volume_name("work"), volume_name("personal"));
    }
}
