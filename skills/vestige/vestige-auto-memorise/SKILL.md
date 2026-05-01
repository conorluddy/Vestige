---
name: vestige-auto-memorise
description: Fires automatically — without the user asking — whenever the conversation produces something a future session would want to recover. Watch for and trigger on decisions ("we'll go with…", "let's use…", "settled — X", "I'm choosing X over Y"), preferences ("I prefer…", "always…", "never…", "in this project we…", "make sure to…"), open questions ("TBD", "unclear whether…", "we should figure out…", "follow-up:"), aha moments / TILs ("turns out…", "the reason X happens is…", "good to know:", "interesting — X"), durable todos ("TODO:", "remember to…", "come back to this"), code smells ("this is hairy", "refactor candidate", "duplication here", "leaky abstraction", "smell:"), gotchas ("careful — X breaks Y", "non-obvious", "watch out for…"), workarounds ("hack until upstream fixes…", "temporary fix"), root causes after debugging ("the bug was…", "actually caused by…"), surprising behaviour, and broken assumptions ("I had assumed X, but actually Y"). Always probe `vestige recall` first to dedup, then dispatch inline to the matching `vestige <subcommand> add` and surface a single ambient line — `Recorded <kind> mem_<ULID>.` Use this skill proactively; do not wait for "remember this".
---

# Auto-memorise: fire it yourself

This is the meta-capture skill. The four `vestige-record-*` skills wait for an explicit cue. Auto-memorise fires *proactively* whenever you spot a memorable moment — and dispatches inline to the right `vestige` subcommand without bouncing through another skill.

The point: agentic memory only compounds if capture is automatic. If the agent waits to be told "remember this" every time, the project's accumulated context stays in the user's head — exactly the failure mode Vestige exists to fix.

## When to fire (the watch list)

Fire whenever the conversation produces any of the categories below. You decide the category based on the cue, then dispatch via the matching subcommand.

| Category                    | Cues you watch for                                                                                                                                                       | Subcommand                | Default importance |
|-----------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------|---------------------------|--------------------|
| Decision                    | "we'll go with…", "let's use…", "settled —", "going with X over Y because…", "decision:", explicit `## Decision` headers, you chose A over B with a stated reason         | `vestige decision add`    | 0.7 (0.85+ if architectural) |
| Preference                  | "I prefer…", "always…", "never…", "make sure to…", "in this project we…", "convention:", "rule:", correction the user gave that you should bake in                       | `vestige preference add`  | 0.6 (0.8+ for hard rules) |
| Open question               | "TBD:", "open question:", "unclear whether…", "we should figure out…", "follow-up:", "park that", `## Open Questions` headers                                            | `vestige question add`    | 0.5 (0.8+ if blocking) |
| Aha / TIL / gotcha / smell / TODO / root cause / workaround / surprise | "TIL", "turns out…", "the reason X happens is…", "good to know:", "careful — X breaks Y", "non-obvious", "smell:", "refactor candidate", "TODO:", "the bug was…", "I had assumed X but actually Y", "hack until…" | `vestige note add`        | 0.5 (0.6 for smells / root causes) |

Tie-breaker rule of thumb: did the conversation produce a *commitment with a why*? Decision. A user *opinion*? Preference. An *unknown*? Question. Anything else worth keeping? Note.

If the moment is purely conversational acknowledgement, code that's better expressed as a TODO comment in source, or a fact already encoded in the codebase — **don't capture**. The auto-memorise loop's value comes from precision, not volume. A noisy memory store decays faster than a sparse one.

## How to invoke (categorise → dedup → dispatch)

### Step 1 — Categorise

Pick one row from the table above based on the cue. If two categories overlap (a user-stated commitment with a why), prefer **decision** over preference; the rationale field is the tiebreaker.

### Step 2 — Dedup probe

Before writing, probe for an existing memory that already says this:

```bash
vestige recall "<3-6 keywords from the moment>" --type <category> --json --limit 3
```

Skip the capture if the top hit's `score` clears the threshold and its `one_liner` says essentially the same thing:

- Lexical-only mode (default): skip when `score >= 0.6`.
- `--hybrid` mode: skip when `score >= 0.75`.

If you skip, cite the existing handle: *"Already recorded as `mem_…`."* That's still ambient surface — short and informative.

### Step 3 — Dispatch inline

Call the underlying CLI directly. Don't bounce through another skill — that just wastes turns.

**Decision:**
```bash
vestige decision add "<one-line decision>" \
  --rationale "<the why; cite tradeoffs / alternatives / constraints>" \
  --importance 0.7 \
  --json
```

**Preference:**
```bash
vestige preference add "<the preference, in the user's voice>" \
  --importance 0.6 \
  --json
```

**Question:**
```bash
vestige question add "<framed as an actual question>" \
  --importance 0.5 \
  --json
```

**Note (aha / TIL / gotcha / smell / TODO / root cause / workaround / surprise):**
```bash
vestige note add "<the fact, written so it stands alone>" \
  --importance 0.5 \
  --json
```

When the moment came from a specific file, attach `--source <path:line>` so the captured memory traces back to inspectable evidence.

## After invocation

Surface a single ambient line per capture, no more:

```
Recorded decision mem_01HXXXXXXXXXXXXXXXX.
```

(Or `note` / `preference` / `question` for the other categories.) Don't read the body back to the user — they just said it. Don't list flags or rationale; the JSON already has them. Auto-memorise must feel like a quiet background commit, not a conversation interruption.

If you captured several memories in a single turn (e.g. a long planning discussion produced two decisions and one question), batch the surface line:

```
Recorded decision mem_01… , decision mem_01… , question mem_01… .
```

## What this skill must NOT do

- **Never call `forget` / `restore` / `show` / `context`** as the primary action. Those have their own skills.
- **Never use `vestige recall`** for anything other than the dedup probe in Step 2. Recall has its own skill for retrieval-driven moments.
- **Never re-capture** when the dedup probe finds a near-duplicate. Cite the existing handle instead.
- **Never narrate the capture.** No "I'll record that decision because…" — just record it and surface the one-liner.

## Idempotence & dedup

The skill is idempotent *by discipline* — the dedup probe in Step 2 is what makes proactive auto-firing safe. Without it, every "we'll go with X" from the user produces another row. With it, the second mention is a cite, not a write.

If you ever notice the project memory has accumulated near-duplicates from earlier sessions, that's a signal the dedup threshold was too generous; tighten the score thresholds in Step 2 for the rest of the session.

## Why this exists (the design rationale)

The four `vestige-record-*` skills are precise but require an explicit "remember this" moment in the agent's reasoning. In practice that explicit moment doesn't always happen — the conversation produces a decision and moves on, and three sessions later we re-litigate the same choice.

Auto-memorise closes that loop. It says: *every memorable moment is a capture moment, and you, the agent, are the trigger.* The dedup probe keeps the precision that the per-type skills give you. The categorisation table keeps you using the right memory type. The ambient one-line surface keeps the user undisturbed.

Future configurability: `.vestige/config.toml` will gain an `auto_memorise = ["decision", "preference", "question"]` knob so projects can opt out of categories that are too noisy for them. Until that ships, all four categories are on by default.
