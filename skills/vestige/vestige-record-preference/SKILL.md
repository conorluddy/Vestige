---
name: vestige-record-preference
description: 'Capture a user preference to Vestige memory when the human expresses a convention, opinion, or "how I like things done". Fire on "I prefer X", "always use Y", "don''t do Z", "we always…", "never…", "I like…", "I don''t like…", "make sure to…", "do not…", "convention:…", "rule:…", "house style:", "in this project we…", or when correcting your own output to match what the user just said. Preferences constrain every future agent run via the `vestige context` pack. Default importance 0.6. Returns the new handle (`mem_<ULID>`).'
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

Or you're inferring a preference from a correction the user just gave ("no, I want it written like this") — capture the corrected form in their voice.

Tie-breakers vs siblings:

- *Preference vs decision* — preferences are stylistic / methodological / personal; decisions are architectural commitments with a rationale. If the user gave a *why*, it might still be a decision.
- *Preference vs note* — preferences are stated opinions ("I prefer …"); notes are facts about the project. Don't conflate them: notes don't earn the same surfacing treatment in the context pack.

## How to invoke

```bash
vestige preference add "<the preference, written as the user would phrase it>" \
  --importance 0.6 \
  --json
```

- **body** (positional, required): the preference verbatim if you can. Write in the user's voice ("I prefer …", "always use …") — don't reword to third-person.
- **`--importance`** (optional, default 0.6): bump to 0.8+ for hard rules ("never commit to main"); drop to 0.5 for taste.
- **`--source`** (optional): the file or message where the preference was expressed, if inspectable.

## After invocation

The JSON envelope returns `{ "id": "mem_<ULID>", ... }`. Surface as: *"Captured your preference (`mem_…`). I'll honour it going forward."* — explicit acknowledgement of the constraint.

Preferences feed `vestige context` and are surfaced at the top of every project context pack — they're how the agent inherits the project's accumulated taste without having to re-ask the same questions every session.

## Idempotence & dedup

Every call is a fresh write. Before capturing, dedup:

```bash
vestige recall "<key phrase>" --type preference --json --limit 3
```

If a near-identical preference already exists, skip. If the user has *changed their mind* (the new preference contradicts the old), capture the new one and `vestige-forget` the prior — don't leave contradictory rules active.
