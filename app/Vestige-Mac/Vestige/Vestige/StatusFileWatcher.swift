// File-system watcher for ~/.vestige/daemon.status.json. Re-decodes on every parent-dir write event.

import Foundation
import Observation
import os

@MainActor
@Observable
final class StatusFileWatcher {

    // MARK: - Types

    enum State: Equatable {
        case loading
        case notRunning
        case running(DaemonStatus)
        case stale(DaemonStatus)
        case unsupportedSchema(UInt32)
        case decodeError(String)
    }

    // Matches DaemonStatus::SCHEMA_VERSION in crates/vestige-daemon/src/ipc/status_file.rs.
    static let supportedSchemaVersion: UInt32 = 1

    // MARK: - Public state

    private(set) var state: State = .loading

    /// SF Symbol for the menu-bar label. Shows a paused affordance (V0.5.2) when the daemon
    /// is paused, otherwise the standard brain glyph.
    var menuBarSymbol: String {
        switch state {
        case .running(let status), .stale(let status):
            return status.isPaused ? "pause.circle" : "brain"
        default:
            return "brain"
        }
    }

    /// Monotonic counter bumped whenever a status refresh shows real work happening (EXP-1).
    /// `VestigeApp` drives a one-shot `symbolEffect` pulse on changes to this value.
    private(set) var activityTick: Int = 0

    /// Aggregate unreviewed candidate count across all projects (EXP-2 badge). `0` ⇒ no badge.
    var candidateBadge: Int {
        switch state {
        case .running(let status), .stale(let status):
            return Int(status.projects.reduce(0) { $0 + $1.candidateCount })
        default:
            return 0
        }
    }

    // MARK: - Private

    private let statusPath: URL
    private let logger = Logger(subsystem: "app.vestige.menubar", category: "StatusFileWatcher")

    private var dirSource: DispatchSourceFileSystemObject?
    private var timerSource: DispatchSourceTimer?
    private var dirFd: Int32 = -1

    private var lastDecoded: DaemonStatus?
    private var lastDecodedAt: Date = .distantPast

    // MARK: - Init

    init(path: URL? = nil) {
        if let path {
            statusPath = path
        } else if let envPath = ProcessInfo.processInfo.environment["VESTIGE_STATUS_FILE"] {
            statusPath = URL(fileURLWithPath: envPath)
        } else {
            statusPath = FileManager.default.homeDirectoryForCurrentUser
                .appendingPathComponent(".vestige/daemon.status.json")
        }
    }

    // MARK: - Public API

    func start() {
        reload()

        let parentPath = statusPath.deletingLastPathComponent().path
        let fd = open(parentPath, O_EVTONLY)
        guard fd >= 0 else {
            // Parent directory absent — daemon has never been installed; stay in current state.
            return
        }
        dirFd = fd

        let source = DispatchSource.makeFileSystemObjectSource(
            fileDescriptor: fd,
            eventMask: [.write, .delete, .rename, .extend],
            queue: .main
        )
        source.setEventHandler { [weak self] in
            self?.reload()
        }
        source.setCancelHandler { [weak self] in
            guard let self else { return }
            if self.dirFd >= 0 {
                close(self.dirFd)
                self.dirFd = -1
            }
        }
        source.resume()
        dirSource = source

        let timer = DispatchSource.makeTimerSource(queue: .main)
        timer.schedule(deadline: .now() + 5, repeating: 5)
        timer.setEventHandler { [weak self] in
            self?.checkStaleness()
        }
        timer.resume()
        timerSource = timer
    }

    func stop() {
        dirSource?.cancel()
        dirSource = nil
        timerSource?.cancel()
        timerSource = nil
    }

    // No deinit: watcher is @State on the App scene; Swift 6 nonisolated deinit can't call main-actor stop().

    // MARK: - Private helpers

    private func reload() {
        guard FileManager.default.fileExists(atPath: statusPath.path) else {
            transition(to: .notRunning)
            return
        }

        let data: Data
        do {
            data = try Data(contentsOf: statusPath)
        } catch {
            transition(to: .decodeError(error.localizedDescription))
            return
        }

        let decoded: DaemonStatus
        do {
            decoded = try DaemonStatus.recommendedDecoder.decode(DaemonStatus.self, from: data)
        } catch {
            transition(to: .decodeError(error.localizedDescription))
            return
        }

        if decoded.schemaVersion > Self.supportedSchemaVersion {
            transition(to: .unsupportedSchema(decoded.schemaVersion))
            return
        }

        // EXP-1: pulse the icon when this refresh reflects real work since the last snapshot.
        if let previous = lastDecoded, Self.activityDetected(previous: previous, next: decoded) {
            activityTick &+= 1
        }
        // EXP-4: record per-project memory counts for the sparkline history.
        MemoryHistoryStore.shared.record(decoded)

        lastDecoded = decoded
        lastDecodedAt = Date()
        transition(to: .running(decoded))
    }

    /// Pure diff (EXP-1): did real work happen between two snapshots? `true` when any project
    /// completed a sweep (its `lastEmbedRun` advanced or `pendingEmbeds` dropped) or gained
    /// candidates. No-op refreshes (identical counts) return `false`.
    static func activityDetected(previous: DaemonStatus, next: DaemonStatus) -> Bool {
        var prevById: [String: ProjectStatus] = [:]
        for p in previous.projects { prevById[p.projectId] = p }
        for p in next.projects {
            guard let old = prevById[p.projectId] else {
                // A newly-supervised project with content is activity.
                if p.memoryCount > 0 || p.candidateCount > 0 { return true }
                continue
            }
            if p.candidateCount > old.candidateCount { return true }
            if p.memoryCount > old.memoryCount { return true }
            if p.pendingEmbeds < old.pendingEmbeds { return true }
            if let newEmbed = p.lastEmbedRun, old.lastEmbedRun.map({ newEmbed > $0 }) ?? true {
                return true
            }
        }
        return false
    }

    private func checkStaleness() {
        guard case .running(let current) = state else { return }
        if Date().timeIntervalSince(lastDecodedAt) > 30 {
            transition(to: .stale(current))
        }
    }

    private func transition(to next: State) {
        guard next != state else { return }
        logger.debug("state: \(String(describing: self.state)) → \(String(describing: next))")
        state = next
    }
}
