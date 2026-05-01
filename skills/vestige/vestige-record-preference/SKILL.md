---
name: vestige-record-preference
description: Capture a user preference to Vestige memory when the human you're working with expresses a convention, opinion, taste, or "how I like things done" about this project. Examples to fire on - "I prefer X", "always use Y", "don't do Z", "we always…", "never…", "I like…", "I don't like…", "make sure to…", "do not…", "convention:…", "rule:…", "in this project we…", or "the team prefers…". The value of preferences is durability across sessions — a recorded preference becomes a constraint on every future agent run in this repo via the `vestige context` pack. Default importance is 0.6; returns the new memory's handle (`mem_<ULID>`).
---

# Record a project preference

Capture a stated preference / convention / "how we do things here" so it persists beyond the current session. Preferences are user-stated rules; they're how the project's taste compounds.

## When to fire

The user just said one of:

- "I prefer X" / "I like to…" / "I don't like…"
- "always X" / "never Y" / "we always…" / "we never…"
- "make sure you…" / "make sure not to…"
- "convention:" / "rule:" / "house style:"
- "in this project, we…"

Or you're inferring a preference from a correction the user gave ("no, I want it written like this") — capture the corrected form.

If the human is committing to an *architectural* choice (with a why), it's a decision, not a preference — use `vestige-record-decision`. Preferences are stylistic / methodological / personal.

## How to capture

```bash
vestige preference add "<the preference, written as the user would phrase it>" \
  --importance 0.6 \
  --json
```

- **body** (positional, required): the preference verbatim if you can. Write in the user's voice ("I prefer …", "always use …") — don't reword to third-person.
- **`--importance`** (optional): default 0.6. Bump to 0.8+ for hard rules ("never commit to main"); drop to 0.5 for taste.

## After capture

Read the returned `id`. Surface as: *"Captured your preference (`mem_…`). I'll honour it going forward."* — explicit acknowledgement of the constraint.

## Why preferences matter

Preferences feed `vestige context` and are surfaced at the top of every project context pack. They're how the agent inherits the project's accumulated taste without having to re-ask the same questions every session.
