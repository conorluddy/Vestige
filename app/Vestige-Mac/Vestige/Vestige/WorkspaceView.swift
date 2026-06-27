// Persistent project workspace window (V0.5.2).
//
// A standard resizable window — not the transient MenuBarExtra popover — that keeps the
// per-project detail open while you work. Reuses ProjectRow for each project. Closing it
// returns to menu-bar-only operation; the daemon is unaffected.

import SwiftUI

struct WorkspaceView: View {
    var watcher: StatusFileWatcher

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                switch watcher.state {
                case .loading:
                    Text("Loading…").foregroundStyle(.secondary)
                case .notRunning:
                    Text("Vestige daemon not running.").font(.headline)
                case .running(let status), .stale(let status):
                    header(status)
                    Divider()
                    if status.projects.isEmpty {
                        Text("No projects supervised yet.").foregroundStyle(.secondary)
                    } else {
                        ForEach(status.projects, id: \.projectId) { project in
                            ProjectRow(project: project)
                            Divider()
                        }
                    }
                case .unsupportedSchema(let version):
                    Text("App is older than the daemon (schema v\(version)). Update the app.")
                        .foregroundStyle(.red)
                case .decodeError(let message):
                    Text("Couldn't read daemon status: \(message)").foregroundStyle(.red)
                }
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(minWidth: 420, minHeight: 320)
        .task { watcher.start() }
    }

    private func header(_ status: DaemonStatus) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Vestige")
                .font(.title2.bold())
            Text("v\(status.version) · pid \(status.pid)")
                .font(.caption)
                .foregroundStyle(.secondary)
            if status.isPaused, let until = status.pausedUntil {
                Label("Paused · resumes \(RelativeTime.short(from: until))", systemImage: "pause.circle.fill")
                    .font(.caption)
                    .foregroundStyle(.orange)
            }
        }
    }
}
