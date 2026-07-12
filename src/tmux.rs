// Name the host tmux window after the maudebox instance so the status bar and
// iTerm2's tmux tab track the feature. The container can't do this itself: the
// `\033k…` rename escape it could emit does not disable tmux's
// `automatic-rename`, which keeps re-deriving the window name from the host
// foreground process (`maudebox`/`docker`) and clobbering it. `tmux
// rename-window`, run host-side where tmux is reachable, *does* turn
// automatic-rename off for the window, so the name sticks.

use std::process::{Command, Stdio};

// Run a `tmux <args>` command, discarding output. Best-effort: any failure
// (tmux missing, not in a session) is swallowed — window naming is cosmetic.
fn tmux(args: &[&str]) -> bool {
    Command::new("tmux")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// Capture a single trimmed line from `tmux <args>`.
fn tmux_capture(args: &[&str]) -> Option<String> {
    let out = Command::new("tmux")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Sets the current tmux window's name on construction and restores the prior
/// name / `automatic-rename` state on drop. A no-op (and `Drop` does nothing)
/// when not running inside tmux, so it's safe to construct unconditionally.
pub struct WindowName {
    active: bool,
    prev_name: Option<String>,
    // The window's effective `automatic-rename` value before we renamed.
    prev_auto_on: bool,
}

impl WindowName {
    /// Rename the current tmux window to `name`. Returns an inert guard when
    /// `$TMUX` is unset (not inside tmux).
    pub fn apply(name: &str) -> Self {
        if std::env::var_os("TMUX").is_none() {
            return Self {
                active: false,
                prev_name: None,
                prev_auto_on: false,
            };
        }
        let prev_name = tmux_capture(&["display-message", "-p", "#{window_name}"]);
        let prev_auto_on = tmux_capture(&["display-message", "-p", "#{automatic-rename}"])
            .map(|v| v != "off")
            .unwrap_or(true);
        // rename-window sets the name *and* sets automatic-rename off for the
        // window, which is exactly what keeps the name from snapping back.
        let active = tmux(&["rename-window", name]);
        Self {
            active,
            prev_name,
            prev_auto_on,
        }
    }
}

impl Drop for WindowName {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        // Drop our window-level automatic-rename override so the option reverts
        // to the global default.
        tmux(&["set-window-option", "-u", "automatic-rename"]);
        if self.prev_auto_on {
            // It was on — tmux will rename the window after the foreground
            // command again, so nothing more to restore.
            return;
        }
        // It was explicitly off: put the old name back and re-disable auto.
        if let Some(name) = &self.prev_name {
            tmux(&["rename-window", name]);
        }
        tmux(&["set-window-option", "automatic-rename", "off"]);
    }
}
