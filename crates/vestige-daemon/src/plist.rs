//! LaunchAgent plist generation for `vestige daemon install`.
//!
//! The template lives in `crates/vestige-cli/templates/`. We embed it at compile
//! time via `include_str!` and substitute two placeholders: `{{VESTIGE_BIN}}` for
//! the absolute path to the running binary, and `{{HOME}}` for the user's home
//! directory.

use std::path::{Path, PathBuf};

// === CONSTANTS ===

/// Hard-coded label — matches the plist's `<key>Label</key>` and the launchctl
/// target the install/uninstall commands use.
pub const LAUNCH_AGENT_LABEL: &str = "com.vestige.daemon";

// === PUBLIC API ===

/// Default plist install location (per-user LaunchAgents directory).
pub fn default_plist_path() -> PathBuf {
    home_dir_or_empty()
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LAUNCH_AGENT_LABEL}.plist"))
}

/// Render the template with the given substitutions.
///
/// Replaces `{{VESTIGE_BIN}}` with the absolute path to the `vestige` binary,
/// and `{{HOME}}` with the user's home directory. Returns the fully rendered
/// plist string; the caller is responsible for writing it to disk.
pub fn render(template: &str, vestige_bin: &Path, home: &Path) -> String {
    template
        .replace("{{VESTIGE_BIN}}", &vestige_bin.display().to_string())
        .replace("{{HOME}}", &home.display().to_string())
}

// === PRIVATE HELPERS ===

fn home_dir_or_empty() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const SAMPLE: &str = r#"<?xml version="1.0"?><plist><dict>
<key>ProgramArguments</key><array>
<string>{{VESTIGE_BIN}}</string><string>daemon</string><string>start</string><string>--foreground</string>
</array>
<key>WorkingDirectory</key><string>{{HOME}}</string>
</dict></plist>"#;

    #[test]
    fn render_substitutes_both_placeholders() {
        let out = render(
            SAMPLE,
            &PathBuf::from("/usr/local/bin/vestige"),
            &PathBuf::from("/Users/test"),
        );
        assert!(out.contains("<string>/usr/local/bin/vestige</string>"));
        assert!(out.contains("<string>/Users/test</string>"));
        assert!(!out.contains("{{VESTIGE_BIN}}"));
        assert!(!out.contains("{{HOME}}"));
    }

    #[test]
    fn render_handles_spaces_in_paths() {
        let out = render(
            SAMPLE,
            &PathBuf::from("/Users/test name/bin/vestige"),
            &PathBuf::from("/Users/test name"),
        );
        // Spaces are valid in plist <string> values; confirm substitution happened.
        assert!(out.contains("test name"));
        assert!(!out.contains("{{VESTIGE_BIN}}"));
        assert!(!out.contains("{{HOME}}"));
    }

    #[test]
    fn render_multiple_home_occurrences() {
        // The real template uses {{HOME}} three times (WorkingDirectory,
        // StandardOutPath, StandardErrorPath). All must be replaced.
        let multi = "{{HOME}}/a\n{{HOME}}/b\n{{HOME}}/c";
        let out = render(multi, &PathBuf::from("/bin/v"), &PathBuf::from("/home/x"));
        assert_eq!(out.lines().count(), 3);
        assert!(!out.contains("{{HOME}}"));
        assert_eq!(out.matches("/home/x").count(), 3);
    }

    #[test]
    fn default_plist_path_ends_with_expected_filename() {
        // Can't assert the full path (HOME varies per machine), but we can check
        // the structure: ends with com.vestige.daemon.plist inside LaunchAgents.
        let p = default_plist_path();
        assert_eq!(
            p.file_name().unwrap().to_str().unwrap(),
            "com.vestige.daemon.plist"
        );
        assert!(p.to_string_lossy().contains("LaunchAgents"));
    }
}
