# Vestige skills

Nine Claude Code skill definitions that mirror Vestige's user-facing memory CLI.
Each skill is a self-contained `SKILL.md` describing **when** an agent should
fire it and **how** to invoke the underlying `vestige <subcommand>`.

## Install (manual, for now)

Copy or symlink this entire `vestige/` directory into the consuming repo's
`.claude/skills/` so the model picks the skills up:

```bash
mkdir -p <repo>/.claude/skills/
cp -R skills/vestige <repo>/.claude/skills/
```

A future `vestige init --install-skills` (or equivalent) will automate this.

## What's here

### Capture (drive automatic memorisation)

| Skill                        | Fires when…                                                  | Wraps                          |
|------------------------------|--------------------------------------------------------------|--------------------------------|
| `vestige-record-decision`    | committing to an architectural / approach choice             | `vestige decision add`         |
| `vestige-record-note`        | learning a non-trivial fact about the codebase               | `vestige note add`             |
| `vestige-record-preference`  | the user states a convention / opinion / "how I like things" | `vestige preference add`       |
| `vestige-record-question`    | an unresolved ambiguity is identified                        | `vestige question add`         |

### Retrieve

| Skill              | Fires when…                                              | Wraps             |
|--------------------|----------------------------------------------------------|-------------------|
| `vestige-context`  | session start, before unfamiliar work, "what's the state?" | `vestige context` |
| `vestige-recall`   | "have we discussed this?", before committing             | `vestige recall`  |
| `vestige-show`     | you have a `mem_<ULID>` handle and need the body         | `vestige show`    |

### Lifecycle

| Skill              | Fires when…                                          | Wraps              |
|--------------------|------------------------------------------------------|--------------------|
| `vestige-forget`   | a memory is wrong / superseded / no longer relevant  | `vestige forget`   |
| `vestige-restore`  | undo a forget                                        | `vestige restore`  |

## Why 9 instead of 1:1 with every CLI subcommand

The CLI has ~20 subcommands, but most aren't agent-driven (`init`, `status`,
`mcp`, `embed`, `embeddings`, `reindex` are user-run; `remember` is an alias
for `note`; `list` and `search` are covered by `recall`). Skills should map to
**agent decision points** — moments where the model recognises a memorable
situation and acts. That's where the automatic-memorisation loop closes.
Every memory CLI capability the agent might want to use on its own is covered.

## Format

Each `SKILL.md` is YAML frontmatter (`name`, `description`) followed by a
markdown body. The `description` field is the trigger: it packs the phrases /
moments / cues that should make the model fire the skill. Bodies tell the
agent how to invoke `vestige <cmd>` (always with `--json` where supported)
and how to interpret the response.

All skills shell out to the `vestige` binary on `PATH`. They do **not**
depend on Vestige's MCP server being configured — though they remain
useful alongside it.
