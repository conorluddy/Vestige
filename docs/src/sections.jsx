// sections.jsx — page composition. One function per landing-page section.
// Sections read content from window.VESTIGE and use primitives + diagrams.

const { useState: sUseState } = React;

// ── Top bar ──────────────────────────────────────────────
function Bar() {
  const { meta } = window.VESTIGE;
  return (
    <header className="vt-bar">
      <div className="vt-bar-l">
        <Mark size={20} />
        <span className="vt-brand-name">VESTIGE</span>
        <span className="vt-brand-meta">{meta.version}</span>
        <span className="vt-brand-meta">·</span>
        <span className="vt-brand-meta">{meta.tag}</span>
      </div>
      <nav className="vt-bar-r">
        <a href="#fig01">fig.01</a>
        <a href="#fig02">fig.02</a>
        <a href="#fig03">fig.03</a>
        <a href="#fig04">fig.04</a>
        <a href="#cli">cli</a>
        <a href="#roadmap">roadmap</a>
        <a className="vt-cta-gh" href={meta.repo}>github →</a>
      </nav>
    </header>
  );
}

// ── Hero ─────────────────────────────────────────────────
function Hero() {
  const [copied, setCopied] = sUseState(false);
  const copy = () => {
    try { navigator.clipboard?.writeText('cargo install vestige'); } catch (_) {}
    setCopied(true); setTimeout(() => setCopied(false), 1500);
  };
  return (
    <section className="vt-hero">
      <div className="vt-hero-grid">
        <div>
          <div className="vt-kicker">┌─ ABSTRACT ─────────────────────────</div>
          <h1>
            Agents take notes,<br />
            dream them into shape,<br />
            <em>and recall them when it counts.</em>
          </h1>
          <p>
            Vestige is a local-first, repo-pinned memory layer for coding agents — built around a SQLite canonical store, a six-layer disclosure protocol, and a minimal MCP surface. No daemon, no cloud, no global vector soup.
          </p>
          <div className="vt-install">
            <div className="vt-install-cmd"><span className="vt-prompt">$</span> cargo install vestige</div>
            <button className="vt-install-copy" onClick={copy}>{copied ? 'COPIED' : 'COPY'}</button>
          </div>
          <div className="vt-install-aux">
            also: brew tap conorluddy/vestige && brew install vestige
            <br />
            or: curl -sSfL https://github.com/conorluddy/Vestige/releases/latest/download/vestige-installer.sh | sh
          </div>

          <div className="vt-statgrid">
            <Stat k="scope"   v="project" />
            <Stat k="runtime" v="cli + mcp" />
            <Stat k="store"   v="sqlite" />
            <Stat k="index"   v="fts5 / sqlite-vec" />
            <Stat k="daemon"  v="none" />
            <Stat k="cloud"   v="none" />
          </div>
        </div>

        <div>
          <div className="vt-fig-num">FIG. 01 — SYSTEM SCHEMATIC</div>
          <SystemSchematic />
        </div>
      </div>
    </section>
  );
}

// ── Thesis ───────────────────────────────────────────────
function Thesis() {
  return (
    <Section id="thesis" n="00" title="The problem.">
      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 40 }}>
        <div>
          <div style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, color: 'var(--vt-muted)', letterSpacing: 0.6, marginBottom: 10, textTransform: 'uppercase' }}>Status quo</div>
          <p style={{ margin: 0, fontSize: 14.5, lineHeight: 1.65, color: 'var(--vt-ink-soft)' }}>
            Modern coding agents lose context between sessions. Existing approaches collapse everything into a global vector soup, causing stale recall, context pollution, and poor trust. Memory becomes a debt, not an asset.
          </p>
        </div>
        <div>
          <div style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, color: 'var(--vt-accent)', letterSpacing: 0.6, marginBottom: 10, textTransform: 'uppercase' }}>Vestige inverts</div>
          <ul style={{ margin: 0, padding: 0, listStyle: 'none', fontSize: 14.5, lineHeight: 1.7, color: 'var(--vt-ink)' }}>
            {[
              ['scope',      'project-pinned by default'],
              ['retrieval',  'compact handles, expanded on demand'],
              ['truth',      'SQLite canonical, indexes derived'],
              ['surface',    'CLI + MCP, no daemon'],
              ['inspection', 'human-readable, source-linked'],
            ].map(([k, v], i, arr) => (
              <li key={i} style={{
                display: 'grid', gridTemplateColumns: '110px 1fr', gap: 12,
                padding: '8px 0',
                borderTop: i === 0 ? '1px solid var(--vt-rule)' : 'none',
                borderBottom: '1px solid var(--vt-rule)',
              }}>
                <span style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 11, color: 'var(--vt-muted)', letterSpacing: 0.4 }}>{k}</span>
                <span>{v}</span>
              </li>
            ))}
          </ul>
        </div>
      </div>
    </Section>
  );
}

// ── Disclosure ───────────────────────────────────────────
function Disclosure() {
  return (
    <Section id="fig02" n="02" title="Six-layer disclosure." lede="Recall returns the cheapest representation by default. Climb only when needed.">
      <div style={{ display: 'grid', gridTemplateColumns: '1.05fr .95fr', gap: 24 }}>
        <DisclosureLadder memoryId="mem_01" />
        <LayerCostBars />
      </div>
    </Section>
  );
}

// ── Recall demo ──────────────────────────────────────────
function Recall() {
  return (
    <Section id="recall" n="03" title="Hybrid recall." lede="V0 is FTS over the project store. V0.1 layers in semantic kNN behind a replaceable provider.">
      <RecallPipeline />
      <div style={{ marginTop: 18 }}>
        <RecallDemo />
      </div>
    </Section>
  );
}

// ── MCP ──────────────────────────────────────────────────
function MCP() {
  const { mcpTools } = window.VESTIGE;
  return (
    <Section id="fig03" n="04" title="MCP surface." lede="Seven tools. Two write, five read. Destructive ops require a human.">
      <MCPFlow />
      <div className="vt-frame" style={{ marginTop: 20, fontFamily: 'var(--vt-font-mono)', fontSize: 12 }}>
        <div style={{ display: 'grid', gridTemplateColumns: '110px 1fr 90px', padding: '8px 14px', borderBottom: '1px solid var(--vt-rule)', background: 'var(--vt-panel)', color: 'var(--vt-muted)', fontSize: 10.5, letterSpacing: 0.6, textTransform: 'uppercase' }}>
          <span>role</span><span>tool</span><span style={{ textAlign: 'right' }}>v0</span>
        </div>
        {mcpTools.map((t, i) => (
          <div key={t.name} style={{ display: 'grid', gridTemplateColumns: '110px 1fr 90px', padding: '10px 14px', borderTop: i > 0 ? '1px solid var(--vt-rule)' : 'none', alignItems: 'center' }}>
            <span style={{ color: t.role === 'read' ? 'var(--vt-info)' : 'var(--vt-accent)' }}>{t.role}</span>
            <span>
              <span style={{ color: 'var(--vt-ink)', fontWeight: 500 }}>{t.name}</span>
              <span style={{ color: 'var(--vt-muted)', marginLeft: 12, fontFamily: 'var(--vt-font-sans)', fontSize: 12.5 }}>{t.desc}</span>
            </span>
            <span style={{ textAlign: 'right', color: 'var(--vt-faint)' }}>required</span>
          </div>
        ))}
      </div>
    </Section>
  );
}

// ── Skills ───────────────────────────────────────────────
function Skills() {
  const { skills } = window.VESTIGE;
  const kindColor = {
    auto:       'var(--vt-accent)',
    capture:    'var(--vt-info)',
    retrieve:   'var(--vt-mint)',
    lifecycle:  'var(--vt-muted)',
    provenance: 'var(--vt-faint)',
  };
  return (
    <Section id="skills" n="05" title="Skills surface." lede="Fifteen agent skills, bundled in the binary. Installed by vestige init into .claude/skills/ (Claude Code) AND .agents/skills/ (agentskills.io standard, read by Codex). Compliant with the open standard at agentskills.io.">
      <pre className="vt-pre" style={{ marginBottom: 18 }}>{`# Ships with the binary — cargo install / brew users get them too.
vestige skills install                  # writes to BOTH .claude/skills/ and .agents/skills/
vestige skills install --target agents  # only .agents/skills/ (Codex)
vestige skills install --target claude  # only .claude/skills/ (Claude Code)
vestige skills list --json              # 10 skills + version

# init installs to both targets by default; opt out with:
vestige init --no-install-skills`}</pre>
      <div className="vt-frame" style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 12 }}>
        <div style={{ display: 'grid', gridTemplateColumns: '110px 1fr 1fr', padding: '8px 14px', borderBottom: '1px solid var(--vt-rule)', background: 'var(--vt-panel)', color: 'var(--vt-muted)', fontSize: 10.5, letterSpacing: 0.6, textTransform: 'uppercase' }}>
          <span>kind</span><span>skill</span><span>wraps</span>
        </div>
        {skills.map((s, i) => (
          <div key={s.name} style={{ display: 'grid', gridTemplateColumns: '110px 1fr 1fr', padding: '10px 14px', borderTop: i > 0 ? '1px solid var(--vt-rule)' : 'none', alignItems: 'baseline' }}>
            <span style={{ color: kindColor[s.kind] }}>{s.kind}</span>
            <span>
              <span style={{ color: 'var(--vt-ink)', fontWeight: 500 }}>{s.name}</span>
              <span style={{ display: 'block', color: 'var(--vt-muted)', marginTop: 3, fontFamily: 'var(--vt-font-sans)', fontSize: 12.5, lineHeight: 1.5 }}>{s.desc}</span>
            </span>
            <span style={{ color: 'var(--vt-faint)' }}>{s.wraps}</span>
          </div>
        ))}
      </div>
    </Section>
  );
}

// ── Storage ──────────────────────────────────────────────
function Storage() {
  return (
    <Section id="fig04" n="06" title="Storage layout." lede="Repo gets a tiny pin. Private memory lives outside the working tree.">
      <StorageLayout />
      <pre className="vt-pre" style={{ marginTop: 16 }}>{`# .vestige/pin.toml — committed
project_id   = "vestige"
project_name = "Vestige"
scope        = "project"

[storage]
mode = "user_data"
path = "~/.local/share/vestige/vestige.db"

[recall]
default_depth              = "one_liner"
max_results                = 8
include_global_preferences = false`}</pre>
    </Section>
  );
}

// ── Schema + embedding lifecycle ─────────────────────────
function SchemaSection() {
  return (
    <Section id="schema" n="07" title="Schema." lede="V0 owns three tables. V0.1 adds three more, all rebuildable.">
      <SchemaDiagram />
    </Section>
  );
}

function Embeddings() {
  return (
    <Section id="embeddings" n="08" title="Embedding lifecycle." lede="Vectors are derived state. Provider, model, content drift — anything triggers a rebuild.">
      <EmbeddingLifecycle />
    </Section>
  );
}

// ── Features ─────────────────────────────────────────────
function Features() {
  const { features } = window.VESTIGE;
  return (
    <Section id="features" n="11" title="Defaults.">
      <div className="vt-frame hard" style={{ display: 'grid', gridTemplateColumns: 'repeat(3, 1fr)' }}>
        {features.map((f, i) => (
          <div key={i} style={{
            padding: '20px 22px',
            borderRight: i % 3 !== 2 ? '1px solid var(--vt-ink)' : 'none',
            borderBottom: i < 3 ? '1px solid var(--vt-ink)' : 'none',
          }}>
            <div style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, color: 'var(--vt-accent)', letterSpacing: 0.6 }}>· {f.k}</div>
            <h3 style={{ margin: '8px 0 6px', fontSize: 15, fontWeight: 600, color: 'var(--vt-ink)' }}>{f.t}</h3>
            <p style={{ margin: 0, fontSize: 12.5, lineHeight: 1.55, color: 'var(--vt-muted)' }}>{f.b}</p>
          </div>
        ))}
      </div>
    </Section>
  );
}

// ── Provenance ───────────────────────────────────────────
function Provenance() {
  return (
    <Section id="provenance" n="09" title="Provenance." lede="V0.3 makes every memory answerable: where it came from, what evidence backs it, and what the agent asked.">
      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 1fr', gap: 24, marginBottom: 20 }}>
        <div>
          <div style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, color: 'var(--vt-accent)', letterSpacing: 0.6, textTransform: 'uppercase', marginBottom: 8 }}>Why</div>
          <p style={{ margin: '0 0 10px', fontSize: 13.5, lineHeight: 1.6, color: 'var(--vt-ink-soft)' }}>
            Full provenance walk — raw events, candidate back-reference, source receipts. Works for any memory or pre-approval candidate.
          </p>
          <pre className="vt-pre">{`vestige why mem_01JWXXXXXXXXXXXXXXXXXX

mem_01JWXX…  decision  status=active

Provenance walk:
  ◆ Promoted from candidate cand_01JVXX…
  ◇ candidate.proposed  2026-05-08 11:23:47
  ◇ candidate.approved  2026-05-08 11:24:03

Sources (2):
  ─ src_01JW…  candidate      cand_01JVXX…
  ─ src_01JV…  agent_session  current`}</pre>
        </div>
        <div>
          <div style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, color: 'var(--vt-accent)', letterSpacing: 0.6, textTransform: 'uppercase', marginBottom: 8 }}>Trace receipts</div>
          <p style={{ margin: '0 0 10px', fontSize: 13.5, lineHeight: 1.6, color: 'var(--vt-ink-soft)' }}>
            Every recall call writes a <code>query_events</code> row automatically. Inspect or replay any trace to answer "would I get the same memories today?".
          </p>
          <pre className="vt-pre">{`vestige trace trace_01JWXXXXXXXXXXXXXXXXXX

trace_01JWXX…  search · hybrid  caller=mcp
Time: 2026-05-08 14:02:11 (43ms)
Query: "ULID migration ordering"
Results (2):
  1. mem_01JWXX…  0.83
  2. mem_01HVXX…  0.61

vestige trace replay trace_01JWXXXXXXXXXXXXXXXXXX
→ corpus_drift: 1 (mem_01KAXX… added)`}</pre>
        </div>
        <div>
          <div style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, color: 'var(--vt-accent)', letterSpacing: 0.6, textTransform: 'uppercase', marginBottom: 8 }}>Config</div>
          <p style={{ margin: '0 0 10px', fontSize: 13.5, lineHeight: 1.6, color: 'var(--vt-ink-soft)' }}>
            Tracing is on by default. Tune or disable per-surface in <code>.vestige/config.toml</code>.
          </p>
          <pre className="vt-pre">{`[traces]
enabled                   = true
max_per_project           = 10000
truncate_query_text_bytes = 1024
trace_caller_cli          = true
trace_caller_mcp          = true`}</pre>
        </div>
      </div>
    </Section>
  );
}

// ── Memory browser (TUI) ─────────────────────────────────
function Browser() {
  return (
    <Section id="browser" n="10" title="Memory browser." lede="V0.4 wraps every V0–V0.3 surface in an interactive terminal browser. Three tabs, two-pane layout, keyboard-driven.">
      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 1fr', gap: 24, marginBottom: 20 }}>
        <div>
          <div style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, color: 'var(--vt-accent)', letterSpacing: 0.6, textTransform: 'uppercase', marginBottom: 8 }}>Browse</div>
          <p style={{ margin: '0 0 10px', fontSize: 13.5, lineHeight: 1.6, color: 'var(--vt-ink-soft)' }}>
            Three tabs (Memories · Candidates · Traces), two-pane list+detail, vim navigation. Strikethrough on soft-deleted memories. Live counts in the status row.
          </p>
          <pre className="vt-pre">{`vestige browse

[Memories(47)] [Candidates(3)] [Traces(184)]
┌─ list ────────────┬─ mem_01HX7 ─────────────────┐
│> dec   FTS5+vec hybrid  │ decision  imp 0.82      │
│  note  Trace replay…    │ created  2026-05-08 …  │
│  q     Directives or…   │ summary                │
│  pref  Markdown PRDs    │ Hybrid recall blends … │
│  ̶d̶e̶c̶ ̶ ̶U̶L̶I̶D̶ ̶m̶i̶g̶r̶a̶t̶i̶o̶n̶ ̶  │ sources (1)            │
└───────────────────┴────────────────────────────┘
Vestige · proj_my-project · [Mem 47 · Cand 3 · Trc 184]`}</pre>
        </div>
        <div>
          <div style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, color: 'var(--vt-accent)', letterSpacing: 0.6, textTransform: 'uppercase', marginBottom: 8 }}>Curate</div>
          <p style={{ margin: '0 0 10px', fontSize: 13.5, lineHeight: 1.6, color: 'var(--vt-ink-soft)' }}>
            <code>w</code> / <code>s</code> / <code>t</code> swap the detail pane for the V0.3 provenance walks. <code>f</code> / <code>r</code> forget / restore memories with a confirm modal. <code>a</code> / <code>R</code> approve / reject candidates. <code>p</code> replays the selected trace and renders the diff inline.
          </p>
          <pre className="vt-pre">{`Detail · replay diff
replay of trace_01HX…
new trace: trace_01HY…

⚠  provider mismatch

added (1)
  + mem_01KA…  (new)

removed (1)
  - mem_01HV…  (forgotten)

score changes (1)
  Δ +0.234  mem_01HX…

corpus size: 47`}</pre>
        </div>
        <div>
          <div style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, color: 'var(--vt-accent)', letterSpacing: 0.6, textTransform: 'uppercase', marginBottom: 8 }}>Navigate</div>
          <p style={{ margin: '0 0 10px', fontSize: 13.5, lineHeight: 1.6, color: 'var(--vt-ink-soft)' }}>
            The <code>:</code> command palette jumps across tabs by ID, filters by kind / status / caller, mirrors <code>/</code> search, and aliases <code>?</code> / <code>q</code>. No daemon, no schema change, no new MCP tool.
          </p>
          <pre className="vt-pre">{`:goto trace_01HX0000000000…
:kind decision
:status deleted
:caller mcp
:search hybrid
:help        ·   :quit`}</pre>
        </div>
      </div>
    </Section>
  );
}

// ── CLI reference (tabbed) ───────────────────────────────
function CLI() {
  const { commands } = window.VESTIGE;
  const [tab, setTab] = sUseState(0);
  // Groups are derived from the `group` field on each command entry.
  // Named lookups replace the previous magic-number slice/index approach
  // so that appending new commands to data.js never shifts an existing tab.
  const byGroup = (g) => commands.filter(c => c.group === g);
  const groups = [
    { name: 'capture',    cmds: byGroup('capture') },
    { name: 'recall',     cmds: byGroup('recall') },
    { name: 'lifecycle',  cmds: byGroup('lifecycle') },
    { name: 'provenance', cmds: byGroup('provenance') },
  ];
  return (
    <Section id="cli" n="12" title="CLI reference." lede="Twenty-three commands. Pipe-friendly. Deterministic. (Plus `vestige browse` — interactive, not in the tabs.)">
      <div className="vt-frame hard">
        <div style={{ display: 'flex', borderBottom: '1px solid var(--vt-ink)' }}>
          {groups.map((g, i) => (
            <button key={g.name} onClick={() => setTab(i)} style={{
              flex: 1, appearance: 'none', border: 'none',
              borderRight: i < groups.length - 1 ? '1px solid var(--vt-ink)' : 'none',
              background: tab === i ? 'var(--vt-ink)' : 'var(--vt-bg)',
              color:      tab === i ? 'var(--vt-bg)'  : 'var(--vt-ink)',
              padding: '12px 14px', fontFamily: 'var(--vt-font-mono)', fontSize: 12, letterSpacing: 0.5,
              cursor: 'pointer', textAlign: 'left', textTransform: 'uppercase',
              transition: 'background var(--vt-dur-fast) var(--vt-ease)',
            }}>{g.name}</button>
          ))}
        </div>
        <div>
          {groups[tab].cmds.map((c, i) => (
            <div key={i} style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 18, padding: '12px 18px', borderTop: i > 0 ? '1px solid var(--vt-rule)' : 'none', alignItems: 'baseline' }}>
              <div style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 13, color: 'var(--vt-ink)' }}>
                <span style={{ color: 'var(--vt-accent)' }}>$ </span>{c.cmd}
              </div>
              <div style={{ fontSize: 12.5, color: 'var(--vt-muted)', lineHeight: 1.5 }}>{c.desc}</div>
            </div>
          ))}
        </div>
      </div>
    </Section>
  );
}

// ── Roadmap ──────────────────────────────────────────────
function Roadmap() {
  const { roadmap } = window.VESTIGE;
  return (
    <Section id="roadmap" n="13" title="Roadmap." lede="V0 proves the loop. Everything after earns its weight.">
      <div className="vt-frame hard">
        <div style={{ display: 'grid', gridTemplateColumns: '60px 180px 1fr 80px', padding: '10px 14px', background: 'var(--vt-ink)', color: 'var(--vt-bg)', fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, letterSpacing: 0.6, textTransform: 'uppercase' }}>
          <span>ver</span><span>title</span><span>scope</span><span style={{ textAlign: 'right' }}>state</span>
        </div>
        {roadmap.map((r) => {
          const sc = r.status === 'now' ? 'var(--vt-accent)' : r.status === 'next' ? 'var(--vt-info)' : r.status === 'done' ? 'var(--vt-mint)' : 'var(--vt-muted)';
          const rowBg = r.status === 'now' ? 'var(--vt-accent-bg)' : 'transparent';
          return (
            <div key={r.v} style={{ display: 'grid', gridTemplateColumns: '60px 180px 1fr 80px', padding: '11px 14px', borderTop: '1px solid var(--vt-rule)', alignItems: 'baseline', background: rowBg }}>
              <span style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 12, color: 'var(--vt-ink)', fontWeight: 600 }}>{r.v}</span>
              <span style={{ fontSize: 13.5, color: 'var(--vt-ink)', fontWeight: 500 }}>{r.title}</span>
              <span style={{ fontSize: 12.5, color: 'var(--vt-muted)', lineHeight: 1.5 }}>{r.items}</span>
              <span style={{ textAlign: 'right', fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, letterSpacing: 0.5, color: sc, textTransform: 'uppercase' }}>{r.status}</span>
            </div>
          );
        })}
      </div>
    </Section>
  );
}

// ── Footer ───────────────────────────────────────────────
function Footer() {
  const { meta } = window.VESTIGE;
  return (
    <footer className="vt-footer">
      <div className="vt-footer-row">
        <div>
          <div className="vt-footer-brand"><Mark size={18} /> VESTIGE</div>
          <div className="vt-footer-meta">repo-pinned memory · {meta.version} · {meta.license}</div>
        </div>
        <div className="vt-footer-links">
          <a className="is-mint" href={meta.repo}>github →</a>
          <a href="#fig02">disclosure</a>
          <a href="#cli">cli</a>
          <a href="#roadmap">roadmap</a>
        </div>
      </div>
    </footer>
  );
}

Object.assign(window, { Bar, Hero, Thesis, Disclosure, Recall, MCP, Skills, Storage, SchemaSection, Embeddings, Provenance, Browser, Features, CLI, Roadmap, Footer });
