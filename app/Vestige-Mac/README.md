# Vestige.app — macOS menu-bar UI

A SwiftUI menu-bar app for the installed Vestige daemon. Built with Xcode,
outside the Cargo workspace. Requires macOS 14 or later and a current Xcode
with Swift 6.2 or later.

## Build and run

```sh
app/Vestige-Mac/scripts/build-app.sh
open app/Vestige-Mac/dist/Vestige.app
```

The script builds an unsigned Release app. For a persistent installation,
copy `dist/Vestige.app` to `/Applications`. Signing and notarization remain
separate distribution work. The Xcode project includes all Swift sources,
a shared `Vestige` scheme, and the `VestigeTests` target; no manual target
setup is needed.

## What it does

- Shows daemon health, pause state, scheduled jobs, projects, and inbox counts.
- Sends kick, scan, pause/resume, and reload requests over the daemon socket.
- Opens the memory browser and candidate inbox in the active repository.
- Lists recent memories and searches through the installed CLI, including real
  hybrid recall when a real embedding provider is configured.
- Captures notes through `vestige note add`, scoped to the active project.
- Offers daemon enable/disable and an explicit Start at login toggle.

The active project is the supervised project with the most recent memory.
The capture window shows the destination before saving. The global ⌥⌘N
hotkey uses macOS event monitoring and needs Accessibility permission;
the menu's Quick capture action works without global key monitoring.

Quit closes the app without stopping the daemon. Starting the app does not
enable its login item unless explicitly requested with `vestige ui --login`.

## Data and processes

The status watcher reads `~/.vestige/daemon.status.json`, refreshed every
five seconds. Override it with `VESTIGE_STATUS_FILE` when launching the binary
directly. Controls use `~/.vestige/daemon.sock` (`VESTIGE_SOCKET` overrides it).
Memory reads and writes use the installed CLI, resolving common Homebrew and
Cargo binary paths. CLI failures appear in the memory peek instead of looking
like empty results.

No direct SQLite writes occur in the app. For real recall, install the
feature-enabled CLI and configure the project's provider as described in
[the embeddings guide](../../docs/embeddings.md).

## Tests and validation

```sh
xcodebuild -project app/Vestige-Mac/Vestige/Vestige.xcodeproj \
  -scheme Vestige -configuration Debug -destination 'platform=macOS' \
  CODE_SIGNING_ALLOWED=NO test
```

Tests cover status decoding and schema tolerance, pause state, activity diffs,
relative dates, and the CLI's array/list and envelope/search response shapes.

The Release app builds with a macOS 14 deployment target. A live validation
on the development Mac exercised the actual Swift bridge against the installed
CLI and daemon: recent memories, real semantic search, status decoding, and
pause/resume/reload. Quick capture was checked against an isolated database
copy. The app also launched successfully.

Click-through UI behavior, global hotkey permission handling, light/dark
appearance, and login across a logout/reboot still require a manual pass.
Accessibility automation was unavailable in the validation environment.

The Rust status schema lives in
`crates/vestige-daemon/src/ipc/status_file.rs`; evolve it additively.
