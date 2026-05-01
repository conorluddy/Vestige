# Vestige skills

Ten Claude Code skill definitions that mirror Vestige's user-facing memory CLI.
Each skill is a self-contained `SKILL.md` describing **when** an agent should
fire it and **how** to invoke the underlying `vestige <subcommand>`.

## Install

Skills are bundled into the `vestige` binary as of v0.3 — `cargo install vestige`
and Homebrew users get them automatically.

**New repo** — `vestige init` installs skills into `<repo>/.claude/skills/` by
default. Pass `--no-install-skills` to opt out.

**Existing repo** — run `vestige skills install` to write into `<repo>/.claude/skills/`.
Idempotent. Hard-fails on drift unless `--force`.

```bash
# Install (or re-install) skills into the current repo
vestige skills install

# Override destination
vestige skills install --dest /path/to/.claude/skills/

# Preview without writing
vestige skills install --dry-run

# Inspect what shipped with this binary (name + description + file count + version)
vestige skills list --json
```

## What's here

### Auto-capture (proactive, no prompt required)

| Skill                   | Fires when…                                                          | Wraps                                  |
|-------------------------|----------------------------------------------------------------------|----------------------------------------|
| `vestige-auto-memorise` | the conversation produces *anything* memorable — decisions, preferences, open questions, aha / TIL / gotcha / smell / TODO / root cause / workaround / surprise | dispatches inline to `vestige <cmd> add` |

Auto-memorise is the meta-capture skill. It categorises the moment, dedups
against existing memory via `vestige recall`, and dispatches inline to the
right `vestige <cmd> add`. One ambient line per capture; no narration.

### Capture (explicit-cue triggers)

| Skill                       | Fires when…                                                  | Wraps                          |
|-----------------------------|--------------------------------------------------------------|--------------------------------|
| `vestige-record-decision`   | committing to an architectural / approach choice             | `vestige decision add`         |
| `vestige-record-note`       | learning a non-trivial fact about the codebase               | `vestige note add`             |
| `vestige-record-preference` | the user states a convention / opinion / "how I like things" | `vestige preference add`       |
| `vestige-record-question`   | an unresolved ambiguity is identified                        | `vestige question add`         |

### Retrieve

| Skill              | Fires when…                                                | Wraps             |
|--------------------|------------------------------------------------------------|-------------------|
| `vestige-context`  | session start, before unfamiliar work, "what's the state?" | `vestige context` |
| `vestige-recall`   | "have we discussed this?", before committing               | `vestige recall`  |
| `vestige-show`     | you have a `mem_<ULID>` handle and need the body           | `vestige show`    |

### Lifecycle

| Skill              | Fires when…                                          | Wraps              |
|--------------------|------------------------------------------------------|--------------------|
| `vestige-forget`   | a memory is wrong / superseded / no longer relevant  | `vestige forget`   |
| `vestige-restore`  | undo a forget                                        | `vestige restore`  |

## Format

Each `SKILL.md` is YAML frontmatter (`name`, `description`) followed by a
markdown body conforming to a shared template:

1. **`# Title`** — one-sentence purpose.
2. **`## When to fire`** — explicit cues, plus tie-breakers vs adjacent skills.
3. **`## How to invoke`** — the canonical `vestige <subcommand> ... --json`
   bash block, with one bullet per flag.
4. **`## After invocation`** — how to read the JSON envelope and what to
   surface to the user.
5. **`## Idempotence & dedup`** — when re-running is safe; when to dedup via
   `vestige recall` first.

The `description` field is the trigger — it packs the phrases / moments / cues
that should make the model fire the skill. Bodies tell the agent how to
invoke `vestige <cmd>` (always with `--json` where supported) and how to
interpret the response.

All skills shell out to the `vestige` binary on `PATH`. They do **not**
depend on Vestige's MCP server being configured — though they remain useful
alongside it.

## Tests

Each skill has an `evals/` directory:

- **`evals/evals.json`** — 3-5 realistic task prompts that should trigger
  the skill, with `expected_output` describing the shape of the right
  response. Drives the skill-creator output-eval loop (with-skill vs
  baseline subagent runs, grading, `benchmark.json`, viewer).
- **`evals/trigger_evals.json`** — 6-10 should-trigger queries plus 6-10
  near-miss should-not-trigger queries. Drives the skill-creator
  description-optimisation loop (`scripts/run_loop.py`) when we're ready
  to tune triggering precision.

To run the full skill-creator loop locally, follow the instructions in
`~/.claude/plugins/cache/claude-plugins-official/skill-creator/.../SKILL.md`.
The eval workspace lives at `skills/vestige/vestige-skills-workspace/` and
is git-ignored.

## Why ten skills, not 1:1 with every CLI subcommand

The CLI has ~20 subcommands, but most aren't agent-driven (`init`, `status`,
`mcp`, `embed`, `embeddings`, `reindex` are user-run; `remember` is an alias
for `note`; `list` and `search` are covered by `recall`). Skills should map
to **agent decision points** — moments where the model recognises a
memorable situation and acts. That's where the automatic-memorisation loop
closes. Every memory CLI capability the agent might want to use on its own
is covered by one of the ten skills; auto-memorise is the meta-trigger that
closes the loop without requiring an explicit "remember this".

## Future configurability

`vestige-auto-memorise` currently fires on all four memory categories
(decision / preference / question / note). A future
`.vestige/config.toml` knob will let projects opt out per category — e.g.
`auto_memorise = ["decision", "preference", "question"]` to skip notes in
projects where the note channel would be too noisy. Ship-when-stable.
