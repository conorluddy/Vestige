//! `vestige ui` — launch the macOS menu-bar app, plus the shared init/daemon-install
//! boot-prompt gate (V0.5.2).
//!
//! Two responsibilities, kept in one place so the macOS / TTY / CI gate lives once:
//!
//! 1. [`run`] — the `vestige ui` launcher. macOS-only; resolves the app bundle and `open`s it.
//! 2. [`maybe_offer_boot`] — the opt-in prompt fired from the tail of `vestige init` and
//!    `vestige daemon install`. Strictly guarded: it never fires in agent / CI / non-TTY runs.
//!    [`decide_boot`] is the pure, unit-tested core of that guard — the agent-safety invariant.

use std::io::IsTerminal;

use anyhow::{anyhow, Result};
use clap::Args;

// === CLI ARGS ===

#[derive(Debug, Args)]
pub struct UiArgs {
    /// Also register the app as a Login Item so it starts at every login.
    #[arg(long)]
    pub login: bool,
}

// === PUBLIC API ===

/// Launch the Vestige menu-bar app (`vestige ui`).
pub fn run(args: UiArgs) -> Result<()> {
    if !cfg!(target_os = "macos") {
        return Err(anyhow!(
            "`vestige ui` is macOS-only — the menu-bar app (Vestige.app) requires macOS. \
             The daemon and CLI work on every platform."
        ));
    }
    launch_app(args.login)
}

/// The decision the boot-prompt gate reaches, given fully-resolved inputs.
#[derive(Debug, PartialEq, Eq)]
pub enum BootDecision {
    /// Do nothing — agent / CI / non-TTY / non-macOS / opted-out.
    Skip,
    /// Show the interactive `[y/N]` prompt.
    Prompt,
    /// Launch directly without prompting (`--yes` on macOS).
    Launch,
}

/// Pure core of the boot-prompt gate — **the agent-safety invariant**.
///
/// Returns [`BootDecision::Skip`] unless this is an interactive macOS context that has not
/// opted out. `--yes` upgrades to a direct [`BootDecision::Launch`] (for scripted *human*
/// setup) but still requires macOS and the hard opt-out checks. Without `--yes`, a TTY on
/// both stdin and stdout is required to [`BootDecision::Prompt`]; otherwise we skip silently.
pub fn decide_boot(
    is_macos: bool,
    ci: bool,
    noninteractive: bool,
    no_ui: bool,
    both_tty: bool,
    assume_yes: bool,
) -> BootDecision {
    // Hard gate: never offer the GUI in these contexts, regardless of `--yes`.
    if !is_macos || ci || noninteractive || no_ui {
        return BootDecision::Skip;
    }
    if assume_yes {
        return BootDecision::Launch;
    }
    if both_tty {
        BootDecision::Prompt
    } else {
        // Non-interactive (piped / redirected) without `--yes`: stay invisible.
        BootDecision::Skip
    }
}

/// Offer to launch the app and enable start-at-login after a successful `init` /
/// `daemon install`. Silently does nothing outside an interactive macOS TTY (or when `--no-ui`
/// is passed / `CI` / `VESTIGE_NONINTERACTIVE` is set). `--yes` accepts non-interactively.
///
/// Never returns an error — a failed launch is logged, not surfaced, so project setup is never
/// blocked by a missing app bundle.
pub fn maybe_offer_boot(no_ui: bool, assume_yes: bool) {
    let decision = decide_boot(
        cfg!(target_os = "macos"),
        env_flag_set("CI"),
        env_flag_set("VESTIGE_NONINTERACTIVE"),
        no_ui,
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        assume_yes,
    );

    let launch = match decision {
        BootDecision::Skip => return,
        BootDecision::Launch => true,
        BootDecision::Prompt => {
            prompt_yes_no("Launch the Vestige menu-bar app and start it at login? [y/N] ")
        }
    };

    if launch {
        if let Err(e) = launch_app(true) {
            // Best-effort: setup already succeeded; don't fail it over a GUI launch.
            eprintln!("note: could not launch Vestige.app ({e}); run `vestige ui` later.");
        }
    }
}

// === PRIVATE HELPERS ===

/// Whether an environment variable is set to a non-empty value.
fn env_flag_set(key: &str) -> bool {
    std::env::var_os(key).is_some_and(|v| !v.is_empty())
}

/// Resolve and `open` the Vestige.app bundle. `with_login` passes `--enable-login-item` so the
/// app registers its `SMAppService` Login Item on launch.
///
/// Resolution order: LaunchServices by name (`open -a Vestige`) → `~/Applications/Vestige.app`
/// → `/Applications/Vestige.app`.
fn launch_app(with_login: bool) -> Result<()> {
    use std::process::Command;

    let app_args: &[&str] = if with_login {
        &["--args", "--enable-login-item"]
    } else {
        &[]
    };

    // 1. LaunchServices by name.
    let mut cmd = Command::new("open");
    cmd.args(["-a", "Vestige"]);
    cmd.args(app_args);
    if cmd.status().map(|s| s.success()).unwrap_or(false) {
        return Ok(());
    }

    // 2. / 3. Explicit bundle paths.
    let candidates = [
        directories::BaseDirs::new().map(|b| b.home_dir().join("Applications/Vestige.app")),
        Some(std::path::PathBuf::from("/Applications/Vestige.app")),
    ];
    for path in candidates.into_iter().flatten() {
        if path.exists() {
            let mut cmd = Command::new("open");
            cmd.arg(&path);
            cmd.args(app_args);
            if cmd.status().map(|s| s.success()).unwrap_or(false) {
                return Ok(());
            }
        }
    }

    Err(anyhow!(
        "could not find Vestige.app. Build it with app/Vestige-Mac/scripts/build-app.sh and \
         move it to ~/Applications/, or download a release artifact."
    ))
}

/// Read a single y/N answer from stdin. Default (bare Enter / EOF) is No.
fn prompt_yes_no(prompt: &str) -> bool {
    use std::io::Write;
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_skips_off_macos() {
        // Even a fully interactive TTY must skip when not macOS.
        assert_eq!(
            decide_boot(false, false, false, false, true, false),
            BootDecision::Skip
        );
    }

    #[test]
    fn gate_skips_in_ci_and_noninteractive() {
        assert_eq!(
            decide_boot(true, true, false, false, true, false),
            BootDecision::Skip,
            "CI must suppress"
        );
        assert_eq!(
            decide_boot(true, false, true, false, true, false),
            BootDecision::Skip,
            "VESTIGE_NONINTERACTIVE must suppress"
        );
    }

    #[test]
    fn gate_skips_when_no_ui_flag() {
        assert_eq!(
            decide_boot(true, false, false, true, true, false),
            BootDecision::Skip
        );
        // --no-ui wins even over --yes.
        assert_eq!(
            decide_boot(true, false, false, true, true, true),
            BootDecision::Skip
        );
    }

    #[test]
    fn gate_skips_non_tty_without_yes() {
        assert_eq!(
            decide_boot(true, false, false, false, false, false),
            BootDecision::Skip,
            "piped stdin/stdout without --yes must stay invisible"
        );
    }

    #[test]
    fn gate_prompts_only_in_interactive_macos() {
        assert_eq!(
            decide_boot(true, false, false, false, true, false),
            BootDecision::Prompt
        );
    }

    #[test]
    fn gate_launches_directly_with_yes_on_macos() {
        // --yes upgrades to a direct launch even without a TTY...
        assert_eq!(
            decide_boot(true, false, false, false, false, true),
            BootDecision::Launch
        );
        // ...but still requires macOS and the hard opt-outs.
        assert_eq!(
            decide_boot(false, false, false, false, false, true),
            BootDecision::Skip
        );
    }
}
