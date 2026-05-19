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

## Xcode setup

Open `Vestige/Vestige.xcodeproj` in Xcode. The Swift sources next to it on disk (`Vestige/Vestige/*.swift`) need to be added to the project target:

1. In the Xcode project navigator, right-click the `Vestige` group → **Add Files to "Vestige"…**
2. Select every `.swift` file under `Vestige/Vestige/` except `VestigeApp.swift` (already in the project), and the `Assets.xcassets/MenuBarIcon.symbolset` folder. Make sure "Copy items if needed" is **off** and "Add to targets: Vestige" is **on**.
3. In the target's **Info** tab, set `Application is agent (UIElement)` = `YES` (menu-bar-only, no Dock icon).
4. Deployment target: macOS 14.0+.
5. Build & run (⌘R). The 🧠 menu-bar icon should appear.

## File layout

```
app/Vestige-Mac/
├── README.md
├── .gitignore
└── Vestige/
    ├── Vestige.xcodeproj/
    └── Vestige/
        ├── VestigeApp.swift          # @main MenuBarExtra
        ├── DaemonStatus.swift        # Codable mirror of daemon JSON
        ├── StatusFileWatcher.swift   # @Observable file-watch model
        ├── MenuView.swift            # menu content
        ├── ProjectRow.swift          # one row per project
        ├── RelativeTime.swift        # "3m ago" formatter
        └── Assets.xcassets/
```

`xcuserdata/` and `*.xcuserstate` are gitignored.

## Status file schema

Source of truth: `crates/vestige-daemon/src/ipc/status_file.rs` (`DaemonStatus`). Evolve additively only — Codable in Swift tolerates unknown fields, but removed/renamed fields break this app.
