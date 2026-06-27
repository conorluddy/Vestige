// Top-level menu content rendered from StatusFileWatcher.state.

import SwiftUI
import AppKit

struct MenuView: View {
    var watcher: StatusFileWatcher
    var actions: DaemonActions
    var loginItem: LoginItemController
    @State private var copied = false
    @State private var showInactive = false
    @Environment(\.openWindow) private var openWindow

    private static let inactiveThreshold: TimeInterval = 30 * 24 * 60 * 60  // 30 days

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            stateBody
            controlsSection
            if let message = actions.lastMessage {
                Divider()
                Label(message, systemImage: "checkmark.circle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 12)
                    .padding(.top, 6)
                    .accessibilityIdentifier("toast")
            }
            Divider()
            Button("Quit Vestige") { NSApplication.shared.terminate(nil) }
                .keyboardShortcut("q")
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .accessibilityIdentifier("quit")
        }
        .frame(width: 320)
        .padding(.vertical, 12)
        .task { watcher.start() }
    }

    // MARK: - Controls (V0.5.2)

    /// Action + settings controls, shown only when the daemon is running.
    @ViewBuilder
    private var controlsSection: some View {
        if let status = currentStatus {
            Divider().padding(.top, 4)
            VStack(alignment: .leading, spacing: 4) {
                menuButton("Kick embed sweep now", systemImage: "bolt", id: "kick-embed") {
                    actions.kickEmbed()
                }
                menuButton("Scan sessions now", systemImage: "doc.text.magnifyingglass", id: "kick-scan") {
                    actions.kickScan()
                }

                if status.isPaused {
                    menuButton("Resume", systemImage: "play.circle", id: "resume") {
                        actions.resume()
                    }
                } else {
                    Menu {
                        Button("For 1 hour") { actions.pause(for: 3600, label: "for 1 hour") }
                            .accessibilityIdentifier("pause-1h")
                        Button("Until tomorrow morning") { actions.pauseUntilMorning() }
                            .accessibilityIdentifier("pause-morning")
                    } label: {
                        Label("Pause…", systemImage: "pause.circle")
                    }
                    .menuStyle(.borderlessButton)
                    .font(.caption)
                    .padding(.horizontal, 12)
                    .accessibilityIdentifier("pause-menu")
                }

                menuButton("Reload config", systemImage: "arrow.clockwise", id: "reload") {
                    actions.reloadConfig()
                }
                menuButton("Open Vestige window", systemImage: "macwindow", id: "open-window") {
                    openWindow(id: "workspace")
                }
                menuButton("Open browser…", systemImage: "list.bullet.rectangle", id: "open-browser") {
                    actions.openBrowser()
                }
                menuButton("Open daemon log…", systemImage: "doc.plaintext", id: "open-log") {
                    actions.openLog()
                }
                menuButton("Run doctor…", systemImage: "stethoscope", id: "doctor") {
                    actions.runDoctor()
                }

                Divider().padding(.vertical, 2)

                Toggle(isOn: daemonEnabledBinding) {
                    Text("Daemon enabled").font(.caption)
                }
                .toggleStyle(.switch)
                .controlSize(.mini)
                .padding(.horizontal, 12)
                .accessibilityIdentifier("daemon-toggle")

                Toggle(isOn: loginItemBinding) {
                    Text("Start at login").font(.caption)
                }
                .toggleStyle(.switch)
                .controlSize(.mini)
                .padding(.horizontal, 12)
                .accessibilityIdentifier("login-toggle")
            }
            .disabled(actions.isBusy)
        }
    }

    private func menuButton(_ title: String, systemImage: String, id: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(title, systemImage: systemImage)
                .font(.caption)
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 12)
        .accessibilityIdentifier(id)
    }

    private var currentStatus: DaemonStatus? {
        switch watcher.state {
        case .running(let s), .stale(let s): return s
        default: return nil
        }
    }

    private var daemonEnabledBinding: Binding<Bool> {
        Binding(
            get: { currentStatus != nil },
            set: { enabled in enabled ? actions.enableDaemon() : actions.disableDaemon() }
        )
    }

    private var loginItemBinding: Binding<Bool> {
        Binding(
            get: { loginItem.isEnabled },
            set: { loginItem.setEnabled($0) }
        )
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
                .padding(.bottom, 2)

            if status.isPaused, let until = status.pausedUntil {
                Label("Paused · resumes \(RelativeTime.short(from: until))", systemImage: "pause.circle.fill")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 4)
                    .accessibilityIdentifier("pause-affordance")
            }

            if let aggregate = aggregateLine(status: status) {
                Text(aggregate)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 8)
            }

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

    private func aggregateLine(status: DaemonStatus) -> String? {
        let memories = status.projects.reduce(0) { $0 + $1.memoryCount }
        let candidates = status.projects.reduce(0) { $0 + $1.candidateCount }
        guard memories > 0 || candidates > 0 else { return nil }
        let memoryNoun = memories == 1 ? "memory" : "memories"
        if candidates > 0 {
            return "\(memories) \(memoryNoun) · \(candidates) candidates"
        }
        return "\(memories) \(memoryNoun)"
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
