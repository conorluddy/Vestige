// primitives.jsx — small reusable UI primitives.
// Plain React components. No state, no fetching, no opinions.

const { useState, useEffect, useRef } = React;

// ── Brand mark (svg) ─────────────────────────────────────
function Mark({ size = 22, color }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <rect x="2" y="2" width="20" height="20" rx="3" fill="var(--vt-ink)" />
      <circle cx="12" cy="12" r="3" fill={color || 'var(--vt-mint)'} />
      <circle cx="12" cy="12" r="6.5" fill="none" stroke={color || 'var(--vt-mint)'} strokeOpacity=".5" strokeWidth="1.2" />
      <circle cx="12" cy="12" r="9.2" fill="none" stroke={color || 'var(--vt-mint)'} strokeOpacity=".22" strokeWidth="1" />
    </svg>
  );
}

// ── Section shell ────────────────────────────────────────
function Section({ id, n, title, lede, children }) {
  return (
    <section id={id} className="vt-section is-aside">
      <div>
        {n && <div className="vt-fig-num">FIG. {n}</div>}
        <h2>{title}</h2>
        {lede && <p className="vt-lede">{lede}</p>}
      </div>
      <div>{children}</div>
    </section>
  );
}

// ── Pill / badge ─────────────────────────────────────────
function Pill({ children, tone, style }) {
  const cls = 'vt-pill' + (tone ? ` is-${tone}` : '');
  return <span className={cls} style={style}>{children}</span>;
}

// ── Frame (bordered card, soft or hard) ──────────────────
function Frame({ hard, panel, style, children }) {
  const cls = 'vt-frame' + (hard ? ' hard' : '') + (panel ? ' panel' : '');
  return <div className={cls} style={style}>{children}</div>;
}

// ── Mono code blob ───────────────────────────────────────
function Pre({ children, style }) {
  return <pre className="vt-pre" style={style}>{children}</pre>;
}

// ── Stat (small kv tile) ─────────────────────────────────
function Stat({ k, v }) {
  return (
    <div className="vt-stat">
      <div className="vt-stat-k">{k}</div>
      <div className="vt-stat-v">{v}</div>
    </div>
  );
}

Object.assign(window, { Mark, Section, Pill, Frame, Pre, Stat });
