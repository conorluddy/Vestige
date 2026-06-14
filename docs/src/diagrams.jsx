// diagrams.jsx — schematic SVG diagrams.
// All diagrams paint with CSS variable references so theming is centralized.
// Token mapping (var → SVG fill/stroke):
//   --vt-bg        → background fill
//   --vt-ink       → primary stroke / dark fill
//   --vt-muted     → secondary text
//   --vt-faint     → tertiary text
//   --vt-rule      → soft hairlines
//   --vt-accent    → teal accent (active state, "v0.1" features)
//   --vt-mint      → mint highlight (legible on ink)
//   --vt-accent-bg → tinted accent fill
//   --vt-mint-bg   → tinted mint fill

const C = {
  bg:       'var(--vt-bg)',
  ink:      'var(--vt-ink)',
  muted:    'var(--vt-muted)',
  faint:    'var(--vt-faint)',
  rule:     'var(--vt-rule)',
  accent:   'var(--vt-accent)',
  mint:     'var(--vt-mint)',
  accentBg: 'var(--vt-accent-bg)',
  mintBg:   'var(--vt-mint-bg)',
  panel:    'var(--vt-panel)',
};

// ══════════════════════════════════════════════════════════════════
// SystemSchematic — fig.01
// Clients (agent / human / repo) → surfaces (MCP / CLI) → core → stores.
// ══════════════════════════════════════════════════════════════════
function SystemSchematic() {
  return (
    <div className="vt-frame hard">
      <svg viewBox="0 0 620 460" style={{ display: 'block', width: '100%', height: 'auto' }} fontFamily="JetBrains Mono, monospace">
        <rect x="14" y="36" width="592" height="408" fill="none" stroke={C.ink} strokeDasharray="3 3" />
        <text x="20" y="32" fontSize="10" fill={C.muted} letterSpacing="0.6">PROCESS BOUNDARY · LOCAL MACHINE</text>

        {/* Clients */}
        <g>
          <rect x="40" y="58" width="180" height="56" fill={C.bg} stroke={C.ink} />
          <text x="50" y="76" fontSize="10.5" fill={C.muted} letterSpacing="0.6">CLIENT</text>
          <text x="50" y="96" fontSize="13" fill={C.ink} fontWeight="600">Coding agent</text>
          <text x="50" y="108" fontSize="10.5" fill={C.muted}>claude · cursor · codex</text>
        </g>
        <g>
          <rect x="240" y="58" width="160" height="56" fill={C.bg} stroke={C.ink} />
          <text x="250" y="76" fontSize="10.5" fill={C.muted} letterSpacing="0.6">CLIENT</text>
          <text x="250" y="96" fontSize="13" fill={C.ink} fontWeight="600">Human</text>
          <text x="250" y="108" fontSize="10.5" fill={C.muted}>shell · terminal</text>
        </g>
        <g>
          <rect x="420" y="58" width="166" height="56" fill={C.bg} stroke={C.ink} />
          <text x="430" y="76" fontSize="10.5" fill={C.muted} letterSpacing="0.6">CONTEXT</text>
          <text x="430" y="96" fontSize="13" fill={C.ink} fontWeight="600">Repository</text>
          <text x="430" y="108" fontSize="10.5" fill={C.muted}>cwd · git remote</text>
        </g>

        {/* Surfaces */}
        <g>
          <rect x="40" y="156" width="180" height="44" fill={C.accentBg} stroke={C.accent} />
          <text x="130" y="183" fontSize="12" fill={C.ink} textAnchor="middle" fontWeight="600">MCP server</text>
        </g>
        <g>
          <rect x="240" y="156" width="160" height="44" fill={C.accentBg} stroke={C.accent} />
          <text x="320" y="183" fontSize="12" fill={C.ink} textAnchor="middle" fontWeight="600">CLI</text>
        </g>
        <g>
          <rect x="420" y="156" width="166" height="44" fill={C.mintBg} stroke={C.accent} strokeDasharray="3 3" />
          <text x="503" y="178" fontSize="11" fill={C.ink} textAnchor="middle" fontWeight="600">Daemon + UI</text>
          <text x="503" y="192" fontSize="9.5" fill={C.muted} textAnchor="middle">optional local ops</text>
        </g>

        {[[130, 114, 130, 156], [320, 114, 320, 156], [503, 114, 503, 156]].map(([x1,y1,x2,y2], i) => (
          <line key={i} x1={x1} y1={y1} x2={x2} y2={y2} stroke={C.ink} markerEnd="url(#sysArrow)" />
        ))}

        {/* Core */}
        <g>
          <rect x="100" y="234" width="420" height="68" fill={C.ink} />
          <text x="310" y="258" fontSize="11" fill={C.mint} textAnchor="middle" letterSpacing="0.6">SHARED ENGINE · vestige-core / engine</text>
          <text x="310" y="280" fontSize="14" fill={C.bg} textAnchor="middle" fontWeight="600">capture · review · recall · provenance · jobs</text>
          <text x="310" y="295" fontSize="10.5" fill="var(--vt-ink-mute)" textAnchor="middle">CLI/MCP in-process · daemon wraps scheduled work</text>
        </g>
        {[[130, 200, 240, 234], [320, 200, 320, 234], [503, 200, 400, 234]].map(([x1,y1,x2,y2], i) => (
          <line key={i} x1={x1} y1={y1} x2={x2} y2={y2} stroke={C.ink} markerEnd="url(#sysArrow)" />
        ))}

        {/* Stores */}
        <g>
          <rect x="40" y="338" width="240" height="76" fill={C.bg} stroke={C.ink} />
          <text x="52" y="356" fontSize="10.5" fill={C.muted} letterSpacing="0.6">CANONICAL STORE</text>
          <text x="52" y="378" fontSize="14" fill={C.ink} fontWeight="600">SQLite</text>
          <text x="52" y="394" fontSize="10.5" fill={C.muted}>memories · representations · sources</text>
          <text x="52" y="408" fontSize="10.5" fill={C.muted}>~/.local/share/vestige/projects/{'<repo>'}.db</text>
        </g>
        <g>
          <rect x="296" y="338" width="140" height="76" fill={C.bg} stroke={C.ink} />
          <text x="308" y="356" fontSize="10.5" fill={C.muted} letterSpacing="0.6">INDEX · DERIVED</text>
          <text x="308" y="378" fontSize="13" fill={C.ink} fontWeight="600">FTS5</text>
          <text x="308" y="394" fontSize="10.5" fill={C.muted}>lexical · v0</text>
          <text x="308" y="408" fontSize="10.5" fill={C.muted}>rebuildable</text>
        </g>
        <g>
          <rect x="452" y="338" width="134" height="76" fill={C.mintBg} stroke={C.accent} strokeDasharray="3 3" />
          <text x="464" y="356" fontSize="10.5" fill={C.accent} letterSpacing="0.6">INDEX · v0.1</text>
          <text x="464" y="378" fontSize="13" fill={C.ink} fontWeight="600">sqlite-vec</text>
          <text x="464" y="394" fontSize="10.5" fill={C.muted}>vectors · optional</text>
          <text x="464" y="408" fontSize="10.5" fill={C.muted}>rebuildable</text>
        </g>
        {[[200, 302, 160, 338], [310, 302, 366, 338], [430, 302, 519, 338]].map(([x1,y1,x2,y2], i) => (
          <line key={i} x1={x1} y1={y1} x2={x2} y2={y2} stroke={C.ink} markerEnd="url(#sysArrow)" />
        ))}

        <defs>
          <marker id="sysArrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
            <path d="M 0 0 L 10 5 L 0 10 z" fill={C.ink} />
          </marker>
        </defs>
      </svg>
    </div>
  );
}

// ══════════════════════════════════════════════════════════════════
// RecallPipeline — fig.03
// Query → scope filter → FTS + vector → merge → compact cards.
// ══════════════════════════════════════════════════════════════════
function RecallPipeline() {
  return (
    <div className="vt-frame">
      <svg viewBox="0 0 880 280" style={{ display: 'block', width: '100%', height: 'auto' }} fontFamily="JetBrains Mono, monospace">
        <g>
          <rect x="14" y="110" width="120" height="60" fill={C.bg} stroke={C.ink} />
          <text x="74" y="134" fontSize="10.5" fill={C.muted} textAnchor="middle" letterSpacing="0.6">INPUT</text>
          <text x="74" y="155" fontSize="13" fill={C.ink} textAnchor="middle" fontWeight="600">query</text>
        </g>
        <line x1="134" y1="140" x2="170" y2="140" stroke={C.ink} markerEnd="url(#rpArrow)" />

        <g>
          <rect x="170" y="110" width="100" height="60" fill={C.ink} />
          <text x="220" y="134" fontSize="10.5" fill={C.mint} textAnchor="middle" letterSpacing="0.6">SCOPE</text>
          <text x="220" y="155" fontSize="12" fill={C.bg} textAnchor="middle">project filter</text>
        </g>
        <line x1="270" y1="140" x2="310" y2="100" stroke={C.ink} markerEnd="url(#rpArrow)" />
        <line x1="270" y1="140" x2="310" y2="180" stroke={C.ink} markerEnd="url(#rpArrow)" />

        <g>
          <rect x="310" y="60" width="170" height="60" fill={C.bg} stroke={C.ink} />
          <text x="320" y="78" fontSize="10.5" fill={C.muted} letterSpacing="0.6">LEXICAL · v0</text>
          <text x="320" y="98" fontSize="13" fill={C.ink} fontWeight="600">FTS5 search</text>
          <text x="320" y="113" fontSize="10.5" fill={C.muted}>tokens, prefix, BM25</text>
        </g>
        <g>
          <rect x="310" y="160" width="170" height="60" fill={C.mintBg} stroke={C.accent} />
          <text x="320" y="178" fontSize="10.5" fill={C.accent} letterSpacing="0.6">SEMANTIC · v0.1</text>
          <text x="320" y="198" fontSize="13" fill={C.ink} fontWeight="600">vector kNN</text>
          <text x="320" y="213" fontSize="10.5" fill={C.muted}>per-representation</text>
        </g>

        <line x1="480" y1="90"  x2="540" y2="125" stroke={C.ink} markerEnd="url(#rpArrow)" />
        <line x1="480" y1="190" x2="540" y2="155" stroke={C.ink} markerEnd="url(#rpArrow)" />

        <g>
          <rect x="540" y="110" width="160" height="60" fill={C.ink} />
          <text x="620" y="132" fontSize="10.5" fill={C.mint} textAnchor="middle" letterSpacing="0.6">MERGE</text>
          <text x="620" y="152" fontSize="12.5" fill={C.bg} textAnchor="middle" fontWeight="600">dedup · score · rank</text>
          <text x="620" y="164" fontSize="9.5" fill="var(--vt-ink-mute)" textAnchor="middle">by memory_id</text>
        </g>
        <line x1="700" y1="140" x2="740" y2="140" stroke={C.ink} markerEnd="url(#rpArrow)" />

        <g>
          <rect x="740" y="100" width="124" height="80" fill={C.bg} stroke={C.ink} />
          <text x="750" y="118" fontSize="10.5" fill={C.muted} letterSpacing="0.6">OUTPUT</text>
          <text x="750" y="136" fontSize="12" fill={C.ink} fontWeight="600">compact cards</text>
          <text x="750" y="150" fontSize="9.5" fill={C.muted}>id · type · L1</text>
          <text x="750" y="162" fontSize="9.5" fill={C.muted}>score · score_parts</text>
          <text x="750" y="174" fontSize="9.5" fill={C.muted}>available_depths</text>
        </g>

        <defs>
          <marker id="rpArrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
            <path d="M 0 0 L 10 5 L 0 10 z" fill={C.ink} />
          </marker>
        </defs>
      </svg>
    </div>
  );
}

// ══════════════════════════════════════════════════════════════════
// StorageLayout — fig.05a
// Two file-tree panels: in-repo committed pin, user-data canonical store.
// ══════════════════════════════════════════════════════════════════
function StorageLayout() {
  const Tree = ({ title, lines }) => (
    <div className="vt-frame panel" style={{ flex: 1 }}>
      <div style={{ padding: '10px 14px', borderBottom: '1px solid var(--vt-rule)', fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, color: 'var(--vt-muted)', letterSpacing: 0.6, textTransform: 'uppercase' }}>{title}</div>
      <pre style={{ margin: 0, padding: '14px 16px', fontFamily: 'var(--vt-font-mono)', fontSize: 12.5, lineHeight: 1.6, color: 'var(--vt-ink)', whiteSpace: 'pre' }}>{lines}</pre>
    </div>
  );
  return (
    <div style={{ display: 'flex', gap: 16 }}>
      <Tree title="repo · committed" lines={
`my-app/
├── .vestige/
│   └── pin.toml          ← commit me
├── .gitignore
├── src/
└── README.md`} />
      <Tree title="user · outside working tree" lines={
`~/.local/share/vestige/
├── projects/
│   └── my-app.db         ← canonical
├── indexes/
│   ├── my-app.fts5       ← derived
│   └── my-app.vec        ← derived (v0.1)
└── jobs/
    └── embed.log`} />
    </div>
  );
}

// ══════════════════════════════════════════════════════════════════
// SchemaDiagram — fig.05b
// Six tables: V0 core (memories, representations, sources)
//             V0.1 (memory_embeddings, embedding_jobs, vec_index).
// ══════════════════════════════════════════════════════════════════
function SchemaDiagram() {
  const Table = ({ x, y, w, name, cols, accent = false }) => (
    <g>
      <rect x={x} y={y} width={w} height={28} fill={accent ? C.accent : C.ink} />
      <text x={x + 10} y={y + 19} fontSize="11.5" fill={accent ? C.ink : C.bg} fontWeight="600" fontFamily="JetBrains Mono, monospace">{name}</text>
      <rect x={x} y={y + 28} width={w} height={cols.length * 18 + 8} fill={C.bg} stroke={C.rule} />
      {cols.map((c, i) => (
        <g key={i}>
          <text x={x + 10} y={y + 46 + i * 18} fontSize="10.5" fill={C.ink} fontFamily="JetBrains Mono, monospace">{c[0]}</text>
          <text x={x + w - 10} y={y + 46 + i * 18} fontSize="10.5" fill={C.muted} textAnchor="end" fontFamily="JetBrains Mono, monospace">{c[1]}</text>
        </g>
      ))}
    </g>
  );
  return (
    <div className="vt-frame" style={{ padding: 16 }}>
      <svg viewBox="0 0 880 410" style={{ display: 'block', width: '100%', height: 'auto' }}>
        <Table x={20}  y={20}  w={210} name="memories"               cols={[['id','TEXT PK'],['type','TEXT'],['title','TEXT'],['importance','REAL'],['created_at','TEXT'],['deleted_at','TEXT?']]} />
        <Table x={310} y={20}  w={240} name="memory_representations" cols={[['id','TEXT PK'],['memory_id','FK → memories'],['kind','one_liner|summary|...'],['content','TEXT'],['content_hash','TEXT'],['updated_at','TEXT']]} />
        <Table x={620} y={20}  w={240} name="sources"                cols={[['id','TEXT PK'],['memory_id','FK → memories'],['kind','file|commit|url'],['locator','TEXT'],['captured_at','TEXT']]} />
        <Table x={20}  y={210} w={240} name="memory_embeddings · v0.1" accent cols={[['id','TEXT PK'],['memory_id','FK → memories'],['representation_id','FK → reprs'],['provider','TEXT'],['model','TEXT'],['dimensions','INTEGER'],['vector_hash','TEXT'],['stale_at','TEXT?']]} />
        <Table x={310} y={210} w={240} name="embedding_jobs · v0.1"    accent cols={[['id','TEXT PK'],['memory_id','FK → memories'],['representation_id','FK → reprs'],['status','pending|done|fail'],['error','TEXT?'],['updated_at','TEXT']]} />
        <Table x={620} y={210} w={240} name="vec_index · sqlite-vec"   accent cols={[['rowid','INTEGER'],['embedding','FLOAT[N]'],['representation_id','TEXT'],['model','TEXT']]} />

        <line x1={230} y1={62}  x2={310} y2={62}  stroke={C.muted} />
        <line x1={550} y1={62}  x2={620} y2={62}  stroke={C.muted} />
        <line x1={140} y1={150} x2={140} y2={210} stroke={C.muted} strokeDasharray="3 3" />
        <line x1={430} y1={150} x2={430} y2={210} stroke={C.muted} strokeDasharray="3 3" />
        <line x1={555} y1={290} x2={620} y2={290} stroke={C.muted} strokeDasharray="3 3" />
      </svg>
    </div>
  );
}

// ══════════════════════════════════════════════════════════════════
// EmbeddingLifecycle — fig.06
// State diagram: missing → pending → active → stale → rebuilt.
// ══════════════════════════════════════════════════════════════════
function EmbeddingLifecycle() {
  const State = ({ x, y, w, h, label, sub, fill = C.bg, textColor = C.ink }) => (
    <g>
      <rect x={x} y={y} width={w} height={h} fill={fill} stroke={C.ink} />
      <text x={x + w/2} y={y + 22} fontSize="11.5" fill={textColor} textAnchor="middle" fontWeight="600" fontFamily="JetBrains Mono, monospace">{label}</text>
      {sub && <text x={x + w/2} y={y + 38} fontSize="10" fill={C.muted} textAnchor="middle" fontFamily="JetBrains Mono, monospace">{sub}</text>}
    </g>
  );
  const Arrow = ({ x1, y1, x2, y2, label, dy = -6 }) => (
    <g>
      <line x1={x1} y1={y1} x2={x2} y2={y2} stroke={C.ink} markerEnd="url(#elArrow)" />
      {label && <text x={(x1 + x2) / 2} y={(y1 + y2) / 2 + dy} fontSize="10" fill={C.muted} textAnchor="middle" fontFamily="JetBrains Mono, monospace">{label}</text>}
    </g>
  );
  return (
    <div className="vt-frame">
      <svg viewBox="0 0 880 280" style={{ display: 'block', width: '100%', height: 'auto' }}>
        <State x={20}  y={110} w={130} h={50} label="missing" sub="no row" />
        <State x={210} y={50}  w={130} h={50} label="pending" sub="job queued" />
        <State x={210} y={170} w={130} h={50} label="failed" sub="error logged" />
        <State x={400} y={110} w={130} h={50} label="active"  sub="indexed" fill={C.accent} textColor={C.ink} />
        <State x={590} y={50}  w={130} h={50} label="stale"   sub="content/model changed" fill={C.mintBg} />
        <State x={590} y={170} w={130} h={50} label="deleted" sub="soft-deleted memory" />
        <State x={760} y={110} w={100} h={50} label="rebuilt" sub="reindex" />

        <Arrow x1={150} y1={130} x2={210} y2={85}  label="vestige embed" />
        <Arrow x1={340} y1={75}  x2={400} y2={125} label="success" />
        <Arrow x1={340} y1={195} x2={400} y2={155} label="retry" />
        <Arrow x1={275} y1={100} x2={275} y2={170} label="error" dy={4} />
        <Arrow x1={530} y1={125} x2={590} y2={75}  label="content/model Δ" />
        <Arrow x1={530} y1={145} x2={590} y2={195} label="memory Δ" />
        <Arrow x1={655} y1={100} x2={760} y2={125} label="reindex" />
        <Arrow x1={760} y1={140} x2={530} y2={140} />
        <text x={645} y={154} fontSize="10" fill={C.muted} textAnchor="middle" fontFamily="JetBrains Mono, monospace">replace</text>

        <defs>
          <marker id="elArrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
            <path d="M 0 0 L 10 5 L 0 10 z" fill={C.ink} />
          </marker>
        </defs>
      </svg>
    </div>
  );
}

// ══════════════════════════════════════════════════════════════════
// LayerCostBars — small bar chart for the disclosure section.
// ══════════════════════════════════════════════════════════════════
function LayerCostBars() {
  const layers = window.VESTIGE.layers;
  return (
    <div className="vt-frame panel" style={{ padding: '14px 18px' }}>
      <div style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, color: 'var(--vt-muted)', letterSpacing: 0.6, marginBottom: 10, textTransform: 'uppercase' }}>token cost per representation (log)</div>
      <svg viewBox="0 0 320 180" style={{ width: '100%', height: 'auto' }}>
        {layers.map((L, i) => {
          const x = 30 + i * 50;
          const h = Math.max(4, Math.log2(L.tokens + 1) * 18);
          return (
            <g key={L.id}>
              <rect x={x} y={160 - h} width="36" height={h} fill={i === 1 ? C.accent : C.ink} opacity={i === 1 ? 1 : 0.85} />
              <text x={x + 18} y="174" fontFamily="JetBrains Mono, monospace" fontSize="10" fill={C.muted} textAnchor="middle">{L.id}</text>
              <text x={x + 18} y={156 - h} fontFamily="JetBrains Mono, monospace" fontSize="9" fill={C.muted} textAnchor="middle">~{L.tokens}</text>
            </g>
          );
        })}
      </svg>
      <div style={{ fontSize: 12, color: 'var(--vt-muted)', marginTop: 8, fontStyle: 'italic' }}>Default recall stops at L1. Most queries never need more.</div>
    </div>
  );
}

Object.assign(window, { SystemSchematic, RecallPipeline, StorageLayout, SchemaDiagram, EmbeddingLifecycle, LayerCostBars });
