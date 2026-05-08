//! V0.3 M3 — `vestige why` and `vestige sources` smoke tests.
//!
//! Covers PRD §15 M3 acceptance criteria and issue #58 DoD:
//!
//! - `record → why` shows the recorded event + source.
//! - `candidate add → approve → why <mem_id>` shows both journals + reverse-provenance link.
//! - `forget → why` shows the `memory.forgotten` event.
//! - `sources --kind agent_session` filters correctly; unknown kind errors cleanly.
//! - `--json` shapes validate against PRD §13.1 / §13.2.
//! - Both text and JSON output paths exercised.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_vestige"))
}

struct Repo {
    _tmp: TempDir,
    repo: PathBuf,
    home: PathBuf,
}

fn fresh_repo() -> Repo {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    Repo {
        _tmp: tmp,
        repo,
        home,
    }
}

fn vestige(repo: &Repo, args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .current_dir(&repo.repo)
        .env("HOME", &repo.home)
        .env("VESTIGE_LOG", "warn")
        .args(args)
        .output()
        .expect("vestige binary invoked")
}

fn assert_ok(out: &std::process::Output, ctx: &str) {
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        panic!(
            "{ctx}: exit {:?}\nstdout: {stdout}\nstderr: {stderr}",
            out.status.code()
        );
    }
}

fn assert_fail(out: &std::process::Output, ctx: &str) {
    if out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        panic!("{ctx}: expected failure but exited 0\nstdout: {stdout}");
    }
}

fn parse_json(out: &std::process::Output, ctx: &str) -> Value {
    let stdout = std::str::from_utf8(&out.stdout).expect("utf-8 stdout");
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("{ctx} not JSON: {e}\n{stdout}"))
}

fn stdout_str(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn init(repo: &Repo) {
    let out = vestige(
        repo,
        &["init", "--name", "provenance-smoke", "--no-install-skills"],
    );
    assert_ok(&out, "init");
}

// === TEST 1: record → why (text) ===

#[test]
fn why_shows_recorded_event_for_directly_recorded_memory() {
    let repo = fresh_repo();
    init(&repo);

    // Record a decision memory.
    let out = vestige(
        &repo,
        &[
            "decision",
            "add",
            "Use SQLite as the primary store",
            "--json",
        ],
    );
    assert_ok(&out, "decision add");
    let json = parse_json(&out, "decision add json");
    let mem_id = json["id"].as_str().expect("id").to_string();
    assert!(mem_id.starts_with("mem_"));

    // why (text)
    let out = vestige(&repo, &["why", &mem_id]);
    assert_ok(&out, "why text");
    let text = stdout_str(&out);
    assert!(text.contains(&mem_id), "output must contain the memory id");
    assert!(
        text.contains("memory.recorded"),
        "output must show memory.recorded event; got:\n{text}"
    );
}

// === TEST 2: record → why --json shape ===

#[test]
fn why_json_shape_matches_prd_13_1() {
    let repo = fresh_repo();
    init(&repo);

    let out = vestige(
        &repo,
        &["decision", "add", "Prefer async where I/O-bound", "--json"],
    );
    assert_ok(&out, "decision add");
    let record_json = parse_json(&out, "decision add json");
    let mem_id = record_json["id"].as_str().expect("id").to_string();

    let out = vestige(&repo, &["why", &mem_id, "--json"]);
    assert_ok(&out, "why --json");
    let json = parse_json(&out, "why --json");

    // PRD §13.1 required top-level fields.
    assert_eq!(
        json["memory_id"].as_str().unwrap_or(""),
        mem_id,
        "memory_id must match"
    );
    assert!(json["type"].is_string(), "type must be string");
    assert!(json["status"].is_string(), "status must be string");
    assert!(json["provenance"].is_object(), "provenance must be object");
    assert!(
        json["status_history"].is_array(),
        "status_history must be array"
    );

    // provenance.events must be an array containing at least the recorded event.
    let events = json["provenance"]["events"]
        .as_array()
        .expect("provenance.events must be array");
    assert!(
        !events.is_empty(),
        "provenance.events must contain at least one event"
    );
    let recorded = events
        .iter()
        .any(|e| e["type"].as_str().unwrap_or("") == "memory.recorded");
    assert!(recorded, "provenance.events must contain memory.recorded");

    // Each event must have event_id, type, at.
    for e in events {
        assert!(e["event_id"].is_string(), "event must have event_id");
        assert!(e["type"].is_string(), "event must have type");
        assert!(e["at"].is_string(), "event must have at");
    }

    // provenance.sources must be an array.
    assert!(
        json["provenance"]["sources"].is_array(),
        "provenance.sources must be array"
    );
}

// === TEST 3: candidate add → approve → why <mem_id> shows both journals ===

#[test]
fn why_shows_both_journals_for_candidate_promoted_memory() {
    let repo = fresh_repo();
    init(&repo);

    // Propose a candidate.
    let out = vestige(
        &repo,
        &[
            "candidate",
            "add",
            "--type",
            "decision",
            "--body",
            "Use dual skill targets for cross-agent support",
            "--json",
        ],
    );
    assert_ok(&out, "candidate add");
    let propose_json = parse_json(&out, "propose json");
    let cand_id = propose_json["candidate_id"].as_str().unwrap().to_string();
    assert!(cand_id.starts_with("cand_"));

    // Approve → get the memory ID.
    let out = vestige(&repo, &["approve", &cand_id, "--json"]);
    assert_ok(&out, "approve");
    let approve_json = parse_json(&out, "approve json");
    let mem_id = approve_json["memory_id"].as_str().unwrap().to_string();
    assert!(mem_id.starts_with("mem_"));

    // why --json for the promoted memory.
    let out = vestige(&repo, &["why", &mem_id, "--json"]);
    assert_ok(&out, "why --json");
    let json = parse_json(&out, "why --json");

    // Memory fields.
    assert_eq!(json["memory_id"].as_str().unwrap_or(""), mem_id);
    assert_eq!(json["status"].as_str().unwrap_or(""), "active");

    // Memory journal must contain memory.recorded.
    let mem_events = json["provenance"]["events"]
        .as_array()
        .expect("provenance.events array");
    let has_recorded = mem_events
        .iter()
        .any(|e| e["type"].as_str().unwrap_or("") == "memory.recorded");
    assert!(
        has_recorded,
        "must contain memory.recorded; got: {mem_events:?}"
    );

    // Candidate back-reference must be populated.
    let candidate_block = &json["provenance"]["candidate"];
    assert!(
        !candidate_block.is_null(),
        "provenance.candidate must be non-null for a promoted memory; json={json}"
    );
    assert_eq!(
        candidate_block["candidate_id"].as_str().unwrap_or(""),
        cand_id,
        "candidate_id in provenance must match the originating candidate"
    );

    // Candidate events must include at least candidate.proposed.
    let cand_events = candidate_block["events"]
        .as_array()
        .expect("candidate.events array");
    let has_proposed = cand_events
        .iter()
        .any(|e| e["type"].as_str().unwrap_or("") == "candidate.proposed");
    assert!(
        has_proposed,
        "candidate.events must contain candidate.proposed; got: {cand_events:?}"
    );
}

// === TEST 4: forget → why shows memory.forgotten event ===

#[test]
fn why_shows_forgotten_event_for_soft_deleted_memory() {
    let repo = fresh_repo();
    init(&repo);

    let out = vestige(
        &repo,
        &["note", "add", "A transient thought to forget", "--json"],
    );
    assert_ok(&out, "note add");
    let json = parse_json(&out, "note add json");
    let mem_id = json["id"].as_str().expect("id").to_string();

    // Forget it.
    let out = vestige(&repo, &["forget", &mem_id]);
    assert_ok(&out, "forget");

    // why must still work for a deleted memory.
    let out = vestige(&repo, &["why", &mem_id, "--json"]);
    assert_ok(&out, "why deleted --json");
    let json = parse_json(&out, "why deleted --json");

    assert_eq!(json["status"].as_str().unwrap_or(""), "deleted");

    let events = json["provenance"]["events"]
        .as_array()
        .expect("provenance.events array");
    let has_forgotten = events
        .iter()
        .any(|e| e["type"].as_str().unwrap_or("") == "memory.forgotten");
    assert!(
        has_forgotten,
        "status_history must contain memory.forgotten; got: {events:?}"
    );

    // status_history must also contain the event.
    let history = json["status_history"].as_array().expect("status_history");
    let history_has_forgotten = history
        .iter()
        .any(|e| e["type"].as_str().unwrap_or("") == "memory.forgotten");
    assert!(
        history_has_forgotten,
        "status_history must include forgotten event"
    );
}

// === TEST 5: sources --json shape matches PRD §13.2 ===

#[test]
fn sources_json_shape_matches_prd_13_2() {
    let repo = fresh_repo();
    init(&repo);

    let out = vestige(&repo, &["note", "add", "Sources shape test", "--json"]);
    assert_ok(&out, "note add");
    let note_json = parse_json(&out, "note add json");
    let mem_id = note_json["id"].as_str().expect("id").to_string();

    let out = vestige(&repo, &["sources", &mem_id, "--json"]);
    assert_ok(&out, "sources --json");
    let json = parse_json(&out, "sources --json");

    // PRD §13.2 required fields.
    assert_eq!(
        json["owner_id"].as_str().unwrap_or(""),
        mem_id,
        "owner_id must match"
    );
    assert_eq!(
        json["owner_kind"].as_str().unwrap_or(""),
        "memory",
        "owner_kind must be 'memory'"
    );
    assert!(json["sources"].is_array(), "sources must be an array");
}

// === TEST 6: sources --kind filter works ===

#[test]
fn sources_kind_filter_returns_matching_sources_only() {
    let repo = fresh_repo();
    init(&repo);

    // Record a note without explicit sources (will get no sources or manual).
    let out = vestige(
        &repo,
        &["note", "add", "Plain note for kind filter test", "--json"],
    );
    assert_ok(&out, "note add");
    let json = parse_json(&out, "note add json");
    let mem_id = json["id"].as_str().expect("id").to_string();

    // Filter by agent_session — should return an empty sources array (no agent_session
    // source was attached at record time).
    let out = vestige(
        &repo,
        &["sources", &mem_id, "--kind", "agent_session", "--json"],
    );
    assert_ok(&out, "sources --kind agent_session");
    let json = parse_json(&out, "sources --kind agent_session --json");

    let sources = json["sources"].as_array().expect("sources array");
    // All returned sources must be of the requested kind.
    for src in sources {
        assert_eq!(
            src["kind"].as_str().unwrap_or(""),
            "agent_session",
            "all returned sources must be kind=agent_session"
        );
    }
}

// === TEST 7: sources --kind unknown kind errors cleanly ===

#[test]
fn sources_unknown_kind_exits_non_zero() {
    let repo = fresh_repo();
    init(&repo);

    let out = vestige(&repo, &["note", "add", "Any note", "--json"]);
    assert_ok(&out, "note add");
    let json = parse_json(&out, "note add json");
    let mem_id = json["id"].as_str().expect("id").to_string();

    // An unknown kind must fail with a non-zero exit and a descriptive error.
    let out = vestige(
        &repo,
        &["sources", &mem_id, "--kind", "clipboard", "--json"],
    );
    assert_fail(&out, "sources --kind clipboard must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("clipboard") || stderr.contains("invalid") || stderr.contains("unknown"),
        "stderr must mention the bad kind; got: {stderr}"
    );
}

// === TEST 8: why <cand_id> works for candidates directly ===

#[test]
fn why_works_for_candidate_directly() {
    let repo = fresh_repo();
    init(&repo);

    let out = vestige(
        &repo,
        &[
            "candidate",
            "add",
            "--type",
            "note",
            "--body",
            "Candidate note for direct why test",
            "--json",
        ],
    );
    assert_ok(&out, "candidate add");
    let json = parse_json(&out, "candidate add json");
    let cand_id = json["candidate_id"]
        .as_str()
        .expect("candidate_id")
        .to_string();

    let out = vestige(&repo, &["why", &cand_id, "--json"]);
    assert_ok(&out, "why cand --json");
    let json = parse_json(&out, "why cand --json");

    // Must be keyed by candidate_id (not memory_id).
    assert_eq!(
        json["candidate_id"].as_str().unwrap_or(""),
        cand_id,
        "candidate_id must match"
    );
    assert!(
        json.get("memory_id").map(|v| v.is_null()).unwrap_or(true),
        "memory_id must be absent or null for candidate subject"
    );
    assert_eq!(json["status"].as_str().unwrap_or(""), "pending");

    // Candidate journal must include candidate.proposed.
    let events = json["provenance"]["events"]
        .as_array()
        .expect("provenance.events array");
    let has_proposed = events
        .iter()
        .any(|e| e["type"].as_str().unwrap_or("") == "candidate.proposed");
    assert!(
        has_proposed,
        "must contain candidate.proposed; got: {events:?}"
    );
}

// === TEST 9: sources <cand_id> works for candidates ===

#[test]
fn sources_works_for_candidate() {
    let repo = fresh_repo();
    init(&repo);

    let out = vestige(
        &repo,
        &[
            "candidate",
            "add",
            "--type",
            "preference",
            "--body",
            "Prefer single-pass migrations over mutating shipped ones",
            "--source-type",
            "file",
            "--source-ref",
            "CLAUDE.md:12",
            "--json",
        ],
    );
    assert_ok(&out, "candidate add with source");
    let json = parse_json(&out, "candidate add json");
    let cand_id = json["candidate_id"]
        .as_str()
        .expect("candidate_id")
        .to_string();

    let out = vestige(&repo, &["sources", &cand_id, "--json"]);
    assert_ok(&out, "sources cand --json");
    let json = parse_json(&out, "sources cand --json");

    assert_eq!(json["owner_kind"].as_str().unwrap_or(""), "candidate");
    assert_eq!(json["owner_id"].as_str().unwrap_or(""), cand_id);

    let sources = json["sources"].as_array().expect("sources array");
    assert!(
        !sources.is_empty(),
        "candidate with --source-ref must have sources"
    );
    let src = &sources[0];
    assert_eq!(src["kind"].as_str().unwrap_or(""), "file");
    assert_eq!(src["source_ref"].as_str().unwrap_or(""), "CLAUDE.md:12");
}

// === TEST 10: why invalid id exits non-zero ===

#[test]
fn why_invalid_id_exits_non_zero() {
    let repo = fresh_repo();
    init(&repo);

    let out = vestige(&repo, &["why", "invalid-id-with-no-prefix"]);
    assert_fail(&out, "why invalid id must fail");
}

// === TEST 11: why non-existent memory exits non-zero ===

#[test]
fn why_nonexistent_memory_exits_non_zero() {
    let repo = fresh_repo();
    init(&repo);

    let out = vestige(&repo, &["why", "mem_01HTHISISNOTREAL000000000"]);
    assert_fail(&out, "why nonexistent memory must fail");
}

// === TEST 12: why text output path (non-JSON) ===

#[test]
fn why_text_output_contains_expected_sections() {
    let repo = fresh_repo();
    init(&repo);

    let out = vestige(
        &repo,
        &["preference", "add", "Use structured logs", "--json"],
    );
    assert_ok(&out, "preference add");
    let json = parse_json(&out, "preference add json");
    let mem_id = json["id"].as_str().expect("id").to_string();

    let out = vestige(&repo, &["why", &mem_id]);
    assert_ok(&out, "why text");
    let text = stdout_str(&out);

    // Must include the ID, the type, and section headers.
    assert!(text.contains(&mem_id), "text must contain memory id");
    assert!(
        text.contains("Provenance walk"),
        "text must have 'Provenance walk' section"
    );
    assert!(text.contains("Sources"), "text must have 'Sources' section");
    assert!(
        text.contains("Status history"),
        "text must have 'Status history' section"
    );
}

// === TEST 13: sources text output path ===

#[test]
fn sources_text_output_shows_owner_id() {
    let repo = fresh_repo();
    init(&repo);

    let out = vestige(&repo, &["decision", "add", "Soft-delete only", "--json"]);
    assert_ok(&out, "decision add");
    let json = parse_json(&out, "decision add json");
    let mem_id = json["id"].as_str().expect("id").to_string();

    let out = vestige(&repo, &["sources", &mem_id]);
    assert_ok(&out, "sources text");
    let text = stdout_str(&out);
    assert!(text.contains(&mem_id), "text must contain owner id");
}
