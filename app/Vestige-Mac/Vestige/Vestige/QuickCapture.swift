// EXP-7 — global-hotkey quick capture.
//
// The one new GUI write path (deliberate, opted-in per #120). A global hotkey (⌥⌘N) opens a
// tiny input; submit files a note into the **active project** via MemoryBridge.captureNote —
// `vestige note add`, never raw SQL. Soft-delete + project-scope rules still bind.
//
// Open questions parked in #120, resolved here as the pragmatic default:
//   • active-project resolution → ActiveProject.resolve (most-recent lastMemoryAt) from the
//     daemon status file, the same bridge the read peek uses.
//   • hotkey conflicts → uses NSEvent monitors (global + local). Global keyDown monitoring
//     needs Accessibility permission; a hardened Carbon RegisterEventHotKey implementation is
//     the follow-up. The in-app ⌘N command always works regardless.

import SwiftUI
import AppKit

@MainActor
@Observable
final class QuickCaptureController {
    /// Bumped when the hotkey fires; `VestigeApp` watches this to open the capture window.
    private(set) var openRequests = 0

    private var globalMonitor: Any?
    private var localMonitor: Any?

    /// Register ⌥⌘N as the quick-capture hotkey. Idempotent.
    func registerHotkey() {
        guard globalMonitor == nil else { return }
        let handler: (NSEvent) -> Void = { [weak self] event in
            guard event.modifierFlags.intersection(.deviceIndependentFlagsMask) == [.command, .option],
                  event.charactersIgnoringModifiers?.lowercased() == "n" else { return }
            self?.requestOpen()
        }
        globalMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown, handler: handler)
        // Local monitor so the hotkey also works when one of our own windows has focus.
        localMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            handler(event)
            return event
        }
    }

    func requestOpen() {
        NSApp.activate(ignoringOtherApps: true)
        openRequests &+= 1
    }

    deinit {
        if let g = globalMonitor { NSEvent.removeMonitor(g) }
        if let l = localMonitor { NSEvent.removeMonitor(l) }
    }
}

/// The minimal capture input. Resolves the active project from the shared watcher, submits a
/// note, and closes.
struct QuickCaptureView: View {
    var watcher: StatusFileWatcher
    @Environment(\.dismiss) private var dismiss

    @State private var text = ""
    @State private var status: String?
    @State private var submitting = false
    @FocusState private var focused: Bool

    private var activeProject: ProjectStatus? {
        switch watcher.state {
        case .running(let s), .stale(let s): return ActiveProject.resolve(from: s)
        default: return nil
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let project = activeProject {
                Text("Capture to \(project.projectName)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                TextField("Memory…", text: $text, axis: .vertical)
                    .textFieldStyle(.plain)
                    .lineLimit(1...4)
                    .focused($focused)
                    .onSubmit { submit(project: project) }
                    .accessibilityIdentifier("quick-capture-field")
                HStack {
                    if let status { Text(status).font(.caption2).foregroundStyle(.secondary) }
                    Spacer()
                    Button("Cancel") { dismiss() }
                        .keyboardShortcut(.cancelAction)
                    Button("Capture") { submit(project: project) }
                        .keyboardShortcut(.defaultAction)
                        .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || submitting)
                        .accessibilityIdentifier("quick-capture-submit")
                }
            } else {
                Text("No active project — open one in a terminal first.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Button("Close") { dismiss() }
            }
        }
        .padding(16)
        .frame(width: 360)
        .onAppear { focused = true }
    }

    private func submit(project: ProjectStatus) {
        let body = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !body.isEmpty else { return }
        submitting = true
        status = "Saving…"
        Task {
            do {
                _ = try await MemoryBridge.captureNote(body, repoRoot: project.repoRoot)
                dismiss()
            } catch {
                status = error.localizedDescription
                submitting = false
            }
        }
    }
}
