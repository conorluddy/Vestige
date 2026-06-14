# Vestige — landing page (GitHub Pages)

A static, single-page landing site for **Vestige** — a local-first, repo-pinned memory layer for coding agents. Built as a self-contained design system: tokens, primitives, and page sections live in plain files. No build step.

## Run it

Open `docs/index.html` directly in a browser.

Published at <https://conorluddy.github.io/Vestige/>. GitHub Pages is configured to deploy from `main` / `/docs`, so anything merged here ships on the next build.

## What's in here

```
docs/
├── index.html          ← the page
├── tokens.css          ← color, type, space, radius, motion variables
├── styles.css          ← page layout, base elements
├── README.md           ← this file
└── src/
    ├── data.js         ← memories, layers, MCP tools, commands, roadmap
    ├── primitives.jsx  ← reusable UI: Button, Mono, Pill, Frame, Rule, Mark
    ├── diagrams.jsx    ← SystemSchematic, RecallPipeline, StorageLayout, Schema, EmbeddingLifecycle
    ├── interactive.jsx ← DisclosureLadder, RecallDemo, MCPFlow
    ├── sections.jsx    ← Hero, Thesis, OperatingLoop, Disclosure, Recall, MCP, Skills, Storage, Schema, Embeddings, Provenance, Browser, Features, CLI, Roadmap, Footer
    └── app.jsx         ← page composition + mount
```

## Design system

### Palette

| Token             | Hex       | Use                                  |
| ----------------- | --------- | ------------------------------------ |
| `--vt-bg`         | `#fbfcfa` | Page surface                         |
| `--vt-panel`      | `#eef3ef` | Tinted panels, inset blocks          |
| `--vt-ink`        | `#101414` | Headlines, hard rules                |
| `--vt-ink-soft`   | `#26302d` | Body copy                            |
| `--vt-muted`      | `#26302d99` | Secondary text                     |
| `--vt-faint`      | `#26302d55` | Annotations, decoration            |
| `--vt-rule`       | `#10141422` | Soft hairline                      |
| `--vt-accent`     | `#339989` | Teal — primary accent                |
| `--vt-mint`       | `#7de2d1` | Mint — highlight surface, status pos |
| `--vt-accent-bg`  | `#33998917` | Tinted accent fill                 |
| `--vt-mint-bg`    | `#7de2d133` | Tinted mint fill                   |

### Type

- **Sans:** Geist (300/400/500/600/700)
- **Mono:** JetBrains Mono (400/500/600)
- **Display:** Geist 600 with tight tracking (`-0.04em` to `-0.025em`)

Type scale: `11 / 12 / 13 / 14.5 / 16 / 22 / 64 px`.

### Space & radius

- 4px grid. Sections: 64px vertical padding, 32px horizontal.
- Radius: 0 (sharp), 3 (cards), 999 (pills).

### Motion

- 150ms standard ease for hover/state.
- 250ms `vstg-fade` for content swaps (disclosure layer, MCP step).
- No parallax, no scroll-jacking.

## Editing

Each `src/*.jsx` file is one concern. To change copy, edit `src/data.js`. To restyle a section, edit `src/sections.jsx`. To swap a diagram, edit `src/diagrams.jsx`. The page never imports a bundler — every file is a `<script type="text/babel">`.

## License

MIT.
