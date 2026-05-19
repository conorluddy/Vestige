# Vestige.app — macOS menu-bar UI

A tiny SwiftUI menu-bar app that surfaces Vestige daemon health at a glance. Lives outside the Cargo workspace — built with Xcode, not `cargo`.

## What it shows

- Daemon pid, uptime, version, next scheduled job.
- Per-project: name, last embed run (relative), pending embed backlog.
- "Daemon not running" CTA with a one-click copy of `vestige daemon install`.
- Stale-daemon badge when the status file stops being refreshed.

Read-only MVP. Mutations (kick / register / reload) come in a follow-up.

## Data source

Reads `~/.vestige/daemon.status.json` (atomically rewritten by the daemon every ~5s). No socket access in the MVP.

Override the path for local testing: `VESTIGE_STATUS_FILE=/tmp/fake.json open Vestige.app`.

## First-time Xcode setup

The repo intentionally does not check in an `.xcodeproj` — pbxproj files are noisy and brittle to hand-edit. Create the project once locally:

1. Open Xcode → **File → New → Project…**
2. Pick **macOS → App**. Name it `Vestige`, interface SwiftUI, language Swift, no tests, no Core Data.
3. Save it inside this `app/Vestige-Mac/` directory. Xcode will create `Vestige.xcodeproj` here.
4. In the new Xcode project, delete the auto-generated `ContentView.swift` and `VestigeApp.swift` (the template ones).
5. Drag every file from this folder's `Vestige/` subdirectory into the Xcode project navigator. Choose "Create groups", target = Vestige.
6. In the target's **Info** tab, set `LSUIElement = YES` (menu-bar-only, no Dock icon). Add it under "Custom macOS Application Target Properties" if Info.plist isn't visible.
7. Deployment target: macOS 14.0+.
8. Build & run (⌘R). The menu-bar icon should appear.

## File layout

```
app/Vestige-Mac/
├── README.md
├── .gitignore
└── Vestige/
    ├── VestigeApp.swift          # @main MenuBarExtra
    ├── DaemonStatus.swift        # Codable mirror of daemon JSON
    ├── StatusFileWatcher.swift   # @Observable file-watch model
    ├── MenuView.swift            # menu content
    ├── ProjectRow.swift          # one row per project
    ├── RelativeTime.swift        # "3m ago" formatter
    └── Assets.xcassets/
```

## Status file schema

Source of truth: `crates/vestige-daemon/src/ipc/status_file.rs` (`DaemonStatus`). Evolve additively only — Codable in Swift tolerates unknown fields, but removed/renamed fields break this app.
