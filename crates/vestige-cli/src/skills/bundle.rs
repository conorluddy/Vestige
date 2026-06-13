//! Compile-time snapshot of the bundled `skills/vestige/` directory and
//! install/list operations over it.
//!
//! `BUNDLED` is embedded at compile time via `include_dir!`. All file bytes are
//! baked into the binary; no filesystem access is needed at runtime for reads.

use include_dir::{include_dir, Dir, DirEntry};
use std::{
    io,
    path::{Path, PathBuf},
};

// === TYPES ===

/// Compile-time snapshot of `skills/vestige/`.
///
/// The `skills` entry in the crate directory is a symlink to `../../skills/vestige`
/// so that `cargo package` bundles the skill files into the `.crate` artifact for
/// `cargo install vestige` users.
pub static BUNDLED: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/skills");

/// Outcome of a single [`install`] call.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstallReport {
    /// Directory skills were installed into.
    pub dest: PathBuf,
    /// Bundle-relative paths that were written (new or force-overwritten).
    pub written: Vec<String>,
    /// Bundle-relative paths that already matched on disk — left untouched.
    pub skipped: Vec<String>,
    /// Bundle-relative paths that differ on disk but `force` was not set.
    pub drifted: Vec<String>,
    /// When true, no filesystem mutations were made.
    pub dry_run: bool,
}

/// Compact summary of a single top-level skill.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillSummary {
    /// Top-level directory name, e.g. `"vestige-recall"`.
    pub name: String,
    /// `description:` value from the skill's `SKILL.md` frontmatter.
    pub description: String,
    /// Total number of files in this skill's subtree.
    pub files: usize,
}

// === PUBLIC API ===

/// Install all bundled skills into `dest`.
///
/// Each bundled file is classified as written, skipped, or drifted based on
/// whether the target is absent, byte-identical, or differs. `force` overwrites
/// drifted files. `dry_run` classifies without writing.
pub fn install(dest: &Path, force: bool, dry_run: bool) -> io::Result<InstallReport> {
    let mut report = InstallReport {
        dest: dest.to_path_buf(),
        written: Vec::new(),
        skipped: Vec::new(),
        drifted: Vec::new(),
        dry_run,
    };

    for entry in iter_skill_files(&BUNDLED) {
        let rel = entry.path().to_string_lossy().into_owned();
        let target = dest.join(entry.path());
        let bundled_bytes = entry.contents();

        if target.exists() {
            let on_disk = std::fs::read(&target)?;
            if on_disk == bundled_bytes {
                report.skipped.push(rel);
                continue;
            }
            // Content differs.
            if force {
                if !dry_run {
                    std::fs::write(&target, bundled_bytes)?;
                }
                report.written.push(rel);
            } else {
                report.drifted.push(rel);
            }
        } else {
            if !dry_run {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&target, bundled_bytes)?;
            }
            report.written.push(rel);
        }
    }

    Ok(report)
}

/// Return a summary for every top-level skill directory, sorted by name.
pub fn list() -> Vec<SkillSummary> {
    let mut summaries: Vec<SkillSummary> = BUNDLED
        .entries()
        .iter()
        .filter_map(|entry| {
            let dir = entry.as_dir()?;
            if !is_skill_dir(dir) {
                return None;
            }
            let name = dir.path().file_name()?.to_string_lossy().into_owned();
            let description = read_skill_description(dir);
            let files = count_files(dir);
            Some(SkillSummary {
                name,
                description,
                files,
            })
        })
        .collect();

    summaries.sort_by(|a, b| a.name.cmp(&b.name));
    summaries
}

// === PRIVATE HELPERS ===

/// Yield every file under a top-level skill directory (one containing a
/// `SKILL.md`). Top-level dirs without a `SKILL.md` — e.g. eval-grading
/// workspaces — are excluded from install so they never leak into a
/// consumer's `.claude/skills/`.
fn iter_skill_files(
    dir: &'static Dir<'static>,
) -> impl Iterator<Item = &'static include_dir::File<'static>> {
    dir.entries()
        .iter()
        .filter_map(|entry| entry.as_dir())
        .filter(|sub| is_skill_dir(sub))
        .flat_map(|sub| {
            sub.find("**/*")
                .expect("glob pattern is valid")
                .filter_map(|e| match e {
                    DirEntry::File(f) => Some(f),
                    _ => None,
                })
        })
}

/// A skill directory is any top-level subtree that contains a `SKILL.md` at
/// its root. Auxiliary directories (eval workspaces, tooling) are filtered out.
fn is_skill_dir(dir: &Dir<'_>) -> bool {
    BUNDLED.get_file(dir.path().join("SKILL.md")).is_some()
}

/// Count files recursively under a directory.
fn count_files(dir: &Dir<'_>) -> usize {
    dir.entries().iter().fold(0, |acc, entry| match entry {
        DirEntry::File(_) => acc + 1,
        DirEntry::Dir(sub) => acc + count_files(sub),
    })
}

/// Extract the `description:` value from YAML frontmatter in `SKILL.md`.
///
/// Frontmatter is delimited by leading `---` lines. The `description:` key
/// carries a single-line quoted or unquoted value — no multi-line block scalars
/// appear in this corpus. Falls back to empty string on any parse failure.
fn read_skill_description(dir: &Dir<'_>) -> String {
    let skill_md_path = dir.path().join("SKILL.md");
    let file = match BUNDLED.get_file(&skill_md_path) {
        Some(f) => f,
        None => return String::new(),
    };
    let text = match std::str::from_utf8(file.contents()) {
        Ok(s) => s,
        Err(_) => return String::new(),
    };
    parse_description_from_frontmatter(text)
}

/// Pull the `description:` value out of a `---`-delimited YAML frontmatter block.
fn parse_description_from_frontmatter(text: &str) -> String {
    let mut lines = text.lines();

    // Frontmatter must start with `---`.
    if lines.next().map(str::trim) != Some("---") {
        return String::new();
    }

    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("description:") {
            return strip_yaml_quotes(rest.trim()).into_owned();
        }
    }

    String::new()
}

/// Remove wrapping `"..."` or `'...'` from a YAML scalar, leaving the value.
///
/// For single-quoted scalars the YAML spec encodes a literal apostrophe as `''`
/// (two consecutive single quotes). This function collapses those pairs after
/// stripping the outer quotes so callers receive the real text.
fn strip_yaml_quotes(value: &str) -> std::borrow::Cow<'_, str> {
    if value.starts_with('"') && value.ends_with('"') {
        std::borrow::Cow::Borrowed(&value[1..value.len() - 1])
    } else if value.starts_with('\'') && value.ends_with('\'') {
        let inner = &value[1..value.len() - 1];
        if inner.contains("''") {
            std::borrow::Cow::Owned(inner.replace("''", "'"))
        } else {
            std::borrow::Cow::Borrowed(inner)
        }
    } else {
        std::borrow::Cow::Borrowed(value)
    }
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_skills_include_auto_memorise() {
        assert!(BUNDLED.get_file("vestige-auto-memorise/SKILL.md").is_some());
    }

    #[test]
    fn list_returns_sixteen_skills() {
        let skills = list();
        assert!(skills.iter().any(|s| s.name == "vestige-auto-memorise"));
        assert!(skills.iter().any(|s| s.name == "vestige-scan-sessions"));
        assert_eq!(skills.len(), 16);
    }

    #[test]
    fn install_to_tmpdir_and_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let r1 = install(tmp.path(), false, false).unwrap();
        assert!(!r1.written.is_empty());
        assert!(r1.drifted.is_empty());
        let r2 = install(tmp.path(), false, false).unwrap();
        assert!(r2.written.is_empty());
        assert!(r2.drifted.is_empty());
        assert_eq!(r2.skipped.len(), r1.written.len());
    }

    #[test]
    fn install_dry_run_writes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let r = install(tmp.path(), false, true).unwrap();
        assert!(
            !r.written.is_empty(),
            "dry_run should still classify as written"
        );
        // No files should actually exist.
        let any_file = tmp.path().read_dir().unwrap().next();
        assert!(any_file.is_none(), "dry_run must not create files");
    }

    #[test]
    fn install_drifted_without_force_leaves_file_unchanged() {
        let tmp = tempfile::TempDir::new().unwrap();
        // First install populates the dest.
        install(tmp.path(), false, false).unwrap();

        // Corrupt one file on disk.
        let target = tmp.path().join("vestige-recall/SKILL.md");
        std::fs::write(&target, b"corrupted").unwrap();

        let r = install(tmp.path(), false, false).unwrap();
        assert!(r
            .drifted
            .iter()
            .any(|p| p.contains("vestige-recall/SKILL.md")));
        // File must still be the corrupted version.
        assert_eq!(std::fs::read(&target).unwrap(), b"corrupted");
    }

    #[test]
    fn install_drifted_with_force_overwrites() {
        let tmp = tempfile::TempDir::new().unwrap();
        install(tmp.path(), false, false).unwrap();

        let target = tmp.path().join("vestige-recall/SKILL.md");
        std::fs::write(&target, b"corrupted").unwrap();

        let r = install(tmp.path(), true, false).unwrap();
        assert!(r
            .written
            .iter()
            .any(|p| p.contains("vestige-recall/SKILL.md")));
        assert!(r.drifted.is_empty());
        assert_ne!(std::fs::read(&target).unwrap(), b"corrupted");
    }

    #[test]
    fn list_descriptions_non_empty_for_skills_with_skill_md() {
        // Skills that have a SKILL.md must have a non-empty description.
        // Directories without SKILL.md (e.g. workspace tooling dirs) return "".
        for skill in list() {
            let skill_md_path = format!("{}/SKILL.md", skill.name);
            if BUNDLED.get_file(&skill_md_path).is_some() {
                assert!(
                    !skill.description.is_empty(),
                    "skill {} has a SKILL.md but empty description",
                    skill.name
                );
            }
        }
    }

    #[test]
    fn list_file_counts_positive() {
        for skill in list() {
            assert!(skill.files > 0, "skill {} reports 0 files", skill.name);
        }
    }

    #[test]
    fn parse_description_handles_unquoted() {
        let md = "---\nname: foo\ndescription: some plain value\n---\n";
        assert_eq!(parse_description_from_frontmatter(md), "some plain value");
    }

    #[test]
    fn parse_description_handles_quoted() {
        let md = "---\nname: foo\ndescription: \"quoted value\"\n---\n";
        assert_eq!(parse_description_from_frontmatter(md), "quoted value");
    }

    #[test]
    fn parse_description_missing_returns_empty() {
        let md = "---\nname: foo\n---\n";
        assert_eq!(parse_description_from_frontmatter(md), "");
    }

    #[test]
    fn parse_description_single_quoted_with_escaped_apostrophes() {
        // YAML single-quoted scalar: '' encodes a literal apostrophe.
        let md = "---\nname: foo\ndescription: 'we''ll go with X; it''s settled'\n---\n";
        assert_eq!(
            parse_description_from_frontmatter(md),
            "we'll go with X; it's settled"
        );
    }

    #[test]
    fn parse_description_single_quoted_no_apostrophes() {
        let md = "---\nname: foo\ndescription: 'plain single-quoted value'\n---\n";
        assert_eq!(
            parse_description_from_frontmatter(md),
            "plain single-quoted value"
        );
    }

    #[test]
    fn strip_yaml_quotes_collapses_double_single_quotes() {
        assert_eq!(
            strip_yaml_quotes("'don''t'"),
            std::borrow::Cow::Owned::<str>("don't".to_owned())
        );
    }
}
