// Top-level menu content rendered from StatusFileWatcher.state.

import SwiftUI
import AppKit

struct MenuView: View {
    var watcher: StatusFileWatcher
    @State private var copied = false
    @State private var showInactive = false

    private static let inactiveThreshold: TimeInterval = 30 * 24 * 60 * 60  // 30 days

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            stateBody
            Divider()
            Button("Quit Vestige") { NSApplication.shared.terminate(nil) }
                .keyboardShortcut("q")
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
        }
        .frame(width: 320)
        .padding(.vertical, 12)
        .task { watcher.start() }
    }

    // MARK: - State bodies

    @ViewBuilder
    private var stateBody: some View {
        switch watcher.state {
        case .loading:
            loadingBody

        case .notRunning:
            notRunningBody

        case .running(let status):
            runningBody(status: status, isStale: false)

        case .stale(let status):
            runningBody(status: status, isStale: true)

        case .unsupportedSchema(let version):
            unsupportedSchemaBody(version: version)

        case .decodeError(let message):
            decodeErrorBody(message: message)
        }
    }

    private var loadingBody: some View {
        Text("Loading…")
            .font(.caption)
            .foregroundStyle(.secondary)
            .padding(.horizontal, 12)
            .padding(.bottom, 8)
    }

    private var notRunningBody: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Vestige daemon not running.")
                .font(.headline)
            Text("Run the following command to install the daemon:")
                .font(.caption)
                .foregroundStyle(.secondary)
            HStack {
                Text("vestige daemon install")
                    .font(.system(.caption, design: .monospaced))
                Spacer()
                Button(copied ? "Copied!" : "Copy") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString("vestige daemon install", forType: .string)
                    copied = true
                    Task {
                        try? await Task.sleep(for: .seconds(1.5))
                        copied = false
                    }
                }
                .font(.caption)
            }
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 8)
    }

    private func runningBody(status: DaemonStatus, isStale: Bool) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            if isStale {
                Label("Daemon may have stopped — status file is stale", systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.yellow)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 6)
            }

            Text(formatHeader(status: status))
                .font(.headline)
                .padding(.horizontal, 12)
                .padding(.bottom, 8)

            Divider()

            projectList(status.projects)

            if let nextJob = status.nextJobs.first {
                Text("Next: \(nextJob.kind.rawValue) \(RelativeTime.short(from: nextJob.at))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 12)
                    .padding(.top, 6)
                    .padding(.bottom, 8)
            }
        }
    }

    private func unsupportedSchemaBody(version: UInt32) -> some View {
        Text("This Vestige.app is older than the daemon (schema v\(version) > supported v\(StatusFileWatcher.supportedSchemaVersion)). Update the app.")
            .font(.caption)
            .foregroundStyle(.red)
            .padding(.horizontal, 12)
            .padding(.bottom, 8)
    }

    private func decodeErrorBody(message: String) -> some View {
        Text("Couldn't read daemon status: \(message)")
            .font(.caption)
            .foregroundStyle(.red)
            .padding(.horizontal, 12)
            .padding(.bottom, 8)
    }

    // MARK: - Project list

    @ViewBuilder
    private func projectList(_ projects: [ProjectStatus]) -> some View {
        if projects.isEmpty {
            Text("No projects supervised yet.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .padding(.horizontal, 12)
                .padding(.top, 8)
        } else {
            let cutoff = Date().addingTimeInterval(-Self.inactiveThreshold)
            let active = projects.filter { isActive($0, cutoff: cutoff) }
            let inactive = projects.filter { !isActive($0, cutoff: cutoff) }

            if active.isEmpty {
                Text("No active projects in the last 30 days.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 12)
                    .padding(.top, 8)
            } else {
                ForEach(active, id: \.projectId) { project in
                    ProjectRow(project: project)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 4)
                }
            }

            if !inactive.isEmpty {
                Divider().padding(.vertical, 4)
                Button {
                    showInactive.toggle()
                } label: {
                    HStack(spacing: 4) {
                        Image(systemName: showInactive ? "chevron.down" : "chevron.right")
                            .font(.caption2)
                        Text("\(showInactive ? "Hide" : "Show") \(inactive.count) inactive")
                            .font(.caption)
                    }
                    .foregroundStyle(.secondary)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .padding(.horizontal, 12)

                if showInactive {
                    ForEach(inactive, id: \.projectId) { project in
                        ProjectRow(project: project)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 4)
                            .opacity(0.7)
                    }
                }
            }
        }
    }

    private func isActive(_ project: ProjectStatus, cutoff: Date) -> Bool {
        if project.pendingEmbeds > 0 { return true }
        guard let lastEmbed = project.lastEmbedRun else { return false }
        return lastEmbed >= cutoff
    }

    // MARK: - Private helpers

    private func formatHeader(status: DaemonStatus) -> String {
        "v\(status.version) · pid \(status.pid) · up \(formatUptime(status.uptimeSecs))"
    }
}

private func formatUptime(_ seconds: UInt64) -> String {
    if seconds < 60 { return "\(seconds)s" }
    let minutes = seconds / 60
    if minutes < 60 { return "\(minutes)m" }
    let hours = minutes / 60
    let remainingMinutes = minutes % 60
    if hours < 24 {
        return remainingMinutes > 0 ? "\(hours)h \(remainingMinutes)m" : "\(hours)h"
    }
    let days = hours / 24
    let remainingHours = hours % 24
    return remainingHours > 0 ? "\(days)d \(remainingHours)h" : "\(days)d"
}
