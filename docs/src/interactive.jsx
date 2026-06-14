// interactive.jsx — stateful UI: DisclosureLadder, RecallDemo, MCPFlow.
// Pulls memory data from window.VESTIGE.

const { useState: vUseState, useEffect: vUseEffect, useRef: vUseRef } = React;

// ── DisclosureLadder ─────────────────────────────────────
// Tabs L0–L5 over a single memory. Click to climb the ladder.
function DisclosureLadder({ memoryId }) {
  const { memories, layers, typeStyle } = window.VESTIGE;
  const memory = memories.find(m => m.id === memoryId) || memories[0];
  const [layer, setLayer] = vUseState('L1');
  const cur = layers.find(L => L.id === layer);
  const value = layer === 'L0' ? memory.id : memory[cur.key];
  const isMono = layer === 'L0' || layer === 'L3' || layer === 'L5';
  const ts = typeStyle[memory.type];

  return (
    <div className="vt-frame" style={{ overflow: 'hidden', borderRadius: 3 }}>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(6, 1fr)', borderBottom: '1px solid var(--vt-rule)' }}>
        {layers.map((L, i) => {
          const active = L.id === layer;
          return (
            <button key={L.id} onClick={() => setLayer(L.id)} style={{
              appearance: 'none', border: 'none',
              background: active ? 'var(--vt-panel)' : 'transparent',
              borderRight: i < 5 ? '1px solid var(--vt-rule)' : 'none',
              borderBottom: active ? '2px solid var(--vt-accent)' : '2px solid transparent',
              padding: '12px 8px', cursor: 'pointer', textAlign: 'left',
              fontFamily: 'var(--vt-font-mono)',
              color: active ? 'var(--vt-ink)' : 'var(--vt-muted)',
              transition: 'background var(--vt-dur-fast) var(--vt-ease)',
            }}>
              <div style={{ fontSize: 11, fontWeight: 600, letterSpacing: 0.4 }}>{L.id}</div>
              <div style={{ fontSize: 10, opacity: 0.85, marginTop: 1 }}>{L.name}</div>
            </button>
          );
        })}
      </div>
      <div style={{ padding: '20px 22px', minHeight: 170 }}>
        <div style={{ display: 'flex', gap: 10, alignItems: 'center', fontFamily: 'var(--vt-font-mono)', fontSize: 11, color: 'var(--vt-muted)', marginBottom: 12, letterSpacing: 0.3 }}>
          <span>{memory.id}</span><span>·</span>
          <span style={{ color: ts.fg }}>{memory.type}</span><span>·</span>
          <span>~{cur.tokens} tokens</span>
          <span style={{ marginLeft: 'auto', color: 'var(--vt-accent)' }}>{cur.id} {cur.name}</span>
        </div>
        <div key={layer} style={{
          fontFamily: isMono ? 'var(--vt-font-mono)' : 'var(--vt-font-sans)',
          fontSize: layer === 'L0' ? 17 : 14.5,
          lineHeight: 1.6, color: 'var(--vt-ink)', whiteSpace: 'pre-wrap',
          animation: 'vstg-fade .25s ease',
        }}>{value}</div>
      </div>
    </div>
  );
}

// ── RecallDemo ───────────────────────────────────────────
// Editable query → ranked memory cards → click to expand to L4 + sources.
function RecallDemo() {
  const { memories, typeStyle } = window.VESTIGE;
  const [q, setQ] = vUseState('storage');
  const [expanded, setExpanded] = vUseState(null);

  const results = memories
    .map(m => {
      const text = (m.title + ' ' + m.L1 + ' ' + m.L2 + ' ' + m.L3).toLowerCase();
      const tokens = q.toLowerCase().split(/\s+/).filter(Boolean);
      let score = 0;
      tokens.forEach(t => { if (text.includes(t)) score += 0.4; });
      score += m.importance * 0.3;
      return { ...m, score: Math.min(1, score) };
    })
    .filter(m => m.score > 0.3)
    .sort((a, b) => b.score - a.score)
    .slice(0, 4);

  return (
    <div className="vt-frame" style={{ overflow: 'hidden', borderRadius: 3 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 10, padding: '12px 16px', background: 'var(--vt-panel)', borderBottom: '1px solid var(--vt-rule)', fontFamily: 'var(--vt-font-mono)', fontSize: 13 }}>
        <span style={{ color: 'var(--vt-accent)' }}>$</span>
        <span style={{ color: 'var(--vt-muted)' }}>vestige recall</span>
        <span style={{ color: 'var(--vt-ink)' }}>"</span>
        <input value={q} onChange={e => setQ(e.target.value)} style={{
          flex: 1, border: 'none', background: 'transparent', outline: 'none',
          fontFamily: 'var(--vt-font-mono)', fontSize: 13, color: 'var(--vt-ink)', padding: 0,
        }} />
        <span style={{ color: 'var(--vt-ink)' }}>"</span>
      </div>
      <div style={{ padding: '6px 0' }}>
        {results.length === 0 && (
          <div style={{ padding: '24px 18px', fontFamily: 'var(--vt-font-mono)', fontSize: 12, color: 'var(--vt-faint)' }}>no matches in this project scope</div>
        )}
        {results.map(m => {
          const ts = typeStyle[m.type];
          const isOpen = expanded === m.id;
          return (
            <div key={m.id} style={{ padding: '10px 18px', borderTop: '1px solid rgba(0,0,0,.06)' }}>
              <div style={{ display: 'flex', gap: 12, alignItems: 'baseline', cursor: 'pointer' }} onClick={() => setExpanded(isOpen ? null : m.id)}>
                <span style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 11.5, color: 'var(--vt-muted)', width: 56 }}>{m.id}</span>
                <span style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, color: ts.fg, background: ts.bg, padding: '2px 7px', borderRadius: 2, letterSpacing: 0.3, textTransform: 'uppercase', flexShrink: 0 }}>{m.type.replace('_', ' ')}</span>
                <span style={{ flex: 1, fontSize: 13.5, color: 'var(--vt-ink)', lineHeight: 1.45 }}>
                  <span style={{ fontWeight: 500 }}>{m.title}</span>
                  <span style={{ color: 'var(--vt-muted)', marginLeft: 8 }}>· {m.L1}</span>
                </span>
                <span style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, color: 'var(--vt-accent)', opacity: 0.85 }}>{m.score.toFixed(2)}</span>
                <span style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 11, color: 'var(--vt-muted)', width: 14, textAlign: 'center' }}>{isOpen ? '−' : '+'}</span>
              </div>
              {isOpen && (
                <div style={{ marginTop: 10, marginLeft: 68, paddingLeft: 14, borderLeft: '2px solid var(--vt-accent)', fontSize: 13, lineHeight: 1.6, color: 'var(--vt-ink-soft)', whiteSpace: 'pre-wrap' }}>
                  {m.L4}
                  <div style={{ marginTop: 10, fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, color: 'var(--vt-faint)' }}>sources: {m.L5}</div>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

// ── MCPFlow ──────────────────────────────────────────────
const MCP_FLOW = [
  { step: 1, tool: 'vestige_bootstrap', desc: 'Agent starts in repo. Pulls compact standing context.',
    req: '{ "max_items": 8, "include": ["summary","decisions","open_questions"] }',
    res: '{\n  "project": { "id": "vestige", "name": "Vestige" },\n  "context": "Project: Vestige\\nSummary: …\\nDecisions: 5\\nOpen: 1",\n  "memories": 6\n}' },
  { step: 2, tool: 'vestige_search', desc: 'Agent searches for relevant memories before acting.',
    req: '{\n  "query": "storage layer",\n  "limit": 4,\n  "depth": "one_liner"\n}',
    res: '{\n  "results": [\n    { "id": "mem_01", "title": "SQLite as canonical store", "score": 0.94 },\n    { "id": "mem_05", "title": "V0.5 ships an opt-in daemon", "score": 0.71 }\n  ]\n}' },
  { step: 3, tool: 'vestige_expand', desc: 'Agent expands the relevant card to the depth it needs.',
    req: '{ "memory_id": "mem_01", "depth": "compressed" }',
    res: '{\n  "id": "mem_01",\n  "depth": "compressed",\n  "content": "Decision: SQLite canonical store. Vector layer non-authoritative …"\n}' },
  { step: 4, tool: 'vestige_record_decision', desc: 'Agent records what it just decided. Inspectable, reversible.',
    req: '{\n  "decision": "Use sqlite-vec for V0.1 embeddings.",\n  "rationale": "Stays in-process; no extra service.",\n  "importance": 0.7\n}',
    res: '{ "id": "mem_07", "status": "active" }' },
];

function MCPFlow() {
  const [step, setStep] = vUseState(1);
  const cur = MCP_FLOW.find(s => s.step === step);
  return (
    <div className="vt-frame" style={{ overflow: 'hidden', borderRadius: 3 }}>
      <div style={{ display: 'flex', borderBottom: '1px solid var(--vt-rule)' }}>
        {MCP_FLOW.map((s, i) => (
          <button key={s.step} onClick={() => setStep(s.step)} style={{
            flex: 1, appearance: 'none', border: 'none',
            borderRight: i < MCP_FLOW.length - 1 ? '1px solid var(--vt-rule)' : 'none',
            background: step === s.step ? 'var(--vt-panel)' : 'transparent',
            borderBottom: step === s.step ? '2px solid var(--vt-accent)' : '2px solid transparent',
            padding: '14px 16px', cursor: 'pointer', textAlign: 'left',
          }}>
            <div style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 11, color: step === s.step ? 'var(--vt-accent)' : 'var(--vt-muted)', letterSpacing: 0.5, marginBottom: 4 }}>STEP {s.step}</div>
            <div style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 12, color: 'var(--vt-ink)', fontWeight: 500 }}>{s.tool}</div>
          </button>
        ))}
      </div>
      <div style={{ padding: '20px 22px' }}>
        <div style={{ fontSize: 14, color: 'var(--vt-ink-soft)', marginBottom: 18, lineHeight: 1.55 }}>{cur.desc}</div>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 16 }}>
          <div>
            <div style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, color: 'var(--vt-muted)', letterSpacing: 0.6, marginBottom: 6, textTransform: 'uppercase' }}>→ request</div>
            <pre className="vt-pre">{cur.req}</pre>
          </div>
          <div>
            <div style={{ fontFamily: 'var(--vt-font-mono)', fontSize: 10.5, color: 'var(--vt-muted)', letterSpacing: 0.6, marginBottom: 6, textTransform: 'uppercase' }}>← response</div>
            <pre className="vt-pre">{cur.res}</pre>
          </div>
        </div>
      </div>
    </div>
  );
}

Object.assign(window, { DisclosureLadder, RecallDemo, MCPFlow });
