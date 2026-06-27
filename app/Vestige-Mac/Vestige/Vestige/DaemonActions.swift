// Action controller for the menu-bar app: socket RPCs + `vestige` shell-outs.
//
// V0.5.2 turns the read-only MVP into a control surface. Socket actions (kick / pause /
// resume / reload) go through DaemonSocket; lifecycle and convenience actions (daemon
// enable/disable, open browser/log/doctor) shell out to the `vestige` binary via Process so
// GUI-only users never touch `launchctl`. A transient `lastMessage` drives a toast/checkmark.

import Foundation
import AppKit

@MainActor
@Observable
final class DaemonActions {
    /// Transient confirmation/error message shown briefly after an action.
    private(set) var lastMessage: String?
    /// True while an action is in flight (to disable buttons / show progress).
    private(set) var isBusy = false

    private let socket = DaemonSocket()

    // MARK: - Socket actions

    func kickEmbed() { run("Embed sweep queued") { try await self.socket.kickEmbed() } }
    func kickScan() { run("Session scan queued") { try await self.socket.kickScan() } }
    func reloadConfig() { run("Config reloaded") { try await self.socket.reloadConfig() } }
    func resume() { run("Resumed") { try await self.socket.resume() } }

    func pause(for interval: TimeInterval, label: String) {
        let until = Date().addingTimeInterval(interval)
        run("Paused \(label)") { try await self.socket.pause(until: until) }
    }

    /// Pause until ~8am tomorrow (local).
    func pauseUntilMorning() {
        let until = Self.nextMorning()
        run("Paused until tomorrow") { try await self.socket.pause(until: until) }
    }

    // MARK: - Lifecycle shell-outs

    func enableDaemon() { shellOut("Daemon enabled", args: ["daemon", "install"]) }
    func disableDaemon() { shellOut("Daemon disabled", args: ["daemon", "uninstall"]) }

    // MARK: - Convenience shell-outs

    /// Open `vestige browse` inside Terminal.app.
    func openBrowser() {
        guard let bin = Self.resolveBinary() else { reportMissingBinary(); return }
        let script = "tell application \"Terminal\" to do script \"\(bin) browse\""
        runAppleScript(script, success: "Opening browser…")
    }

    /// Open the most recent daemon log in the default `.log` handler.
    func openLog() {
        let dir = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".vestige/logs")
        let logs = (try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: [.contentModificationDateKey]))?
            .filter { $0.lastPathComponent.hasPrefix("daemon.log") }
            .sorted { (a, b) in
                let da = (try? a.resourceValues(forKeys: [.contentModificationDateKey]))?.contentModificationDate ?? .distantPast
                let db = (try? b.resourceValues(forKeys: [.contentModificationDateKey]))?.contentModificationDate ?? .distantPast
                return da > db
            }
        if let latest = logs?.first {
            NSWorkspace.shared.open(latest)
        } else {
            flash("No daemon log found")
        }
    }

    /// Open `vestige browse --tab candidates` in Terminal.app (EXP-2 one-click review).
    func reviewCandidates() {
        guard let bin = Self.resolveBinary() else { reportMissingBinary(); return }
        let script = "tell application \"Terminal\" to do script \"\(bin) browse --tab candidates\""
        runAppleScript(script, success: "Opening inbox…")
    }

    /// Run `vestige daemon doctor` in Terminal.app.
    func runDoctor() {
        guard let bin = Self.resolveBinary() else { reportMissingBinary(); return }
        let script = "tell application \"Terminal\" to do script \"\(bin) daemon doctor\""
        runAppleScript(script, success: "Running doctor…")
    }

    // MARK: - Private

    private func run(_ success: String, _ action: @escaping () async throws -> Void) {
        isBusy = true
        Task {
            do {
                try await action()
                flash(success)
            } catch {
                flash(error.localizedDescription)
            }
            isBusy = false
        }
    }

    private func shellOut(_ success: String, args: [String]) {
        guard let bin = Self.resolveBinary() else { reportMissingBinary(); return }
        isBusy = true
        Task.detached { [weak self] in
            let process = Process()
            process.executableURL = URL(fileURLWithPath: bin)
            process.arguments = args + ["--no-ui"]
            var message = success
            do {
                try process.run()
                process.waitUntilExit()
                if process.terminationStatus != 0 {
                    message = "`vestige \(args.joined(separator: " "))` exited \(process.terminationStatus)"
                }
            } catch {
                message = error.localizedDescription
            }
            await MainActor.run {
                self?.flash(message)
                self?.isBusy = false
            }
        }
    }

    private func runAppleScript(_ source: String, success: String) {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
        process.arguments = ["-e", source]
        do {
            try process.run()
            flash(success)
        } catch {
            flash(error.localizedDescription)
        }
    }

    private func reportMissingBinary() {
        flash("`vestige` binary not found in PATH — install it first")
    }

    private func flash(_ message: String) {
        lastMessage = message
        Task {
            try? await Task.sleep(for: .seconds(2.5))
            if lastMessage == message { lastMessage = nil }
        }
    }

    /// Resolve the `vestige` binary across the common install locations.
    static func resolveBinary() -> String? {
        let candidates = [
            "/opt/homebrew/bin/vestige",
            "/usr/local/bin/vestige",
            FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".cargo/bin/vestige").path,
        ]
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0) }
    }

    /// The next 8:00am in the user's local time zone.
    static func nextMorning() -> Date {
        let calendar = Calendar.current
        let now = Date()
        var components = calendar.dateComponents([.year, .month, .day], from: now)
        components.hour = 8
        components.minute = 0
        let todayMorning = calendar.date(from: components) ?? now.addingTimeInterval(3600)
        return todayMorning > now ? todayMorning : calendar.date(byAdding: .day, value: 1, to: todayMorning) ?? todayMorning
    }
}
