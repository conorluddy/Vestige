// @main entry — MenuBarExtra + persistent workspace window + quick-capture window.

import SwiftUI

@main
struct VestigeApp: App {
    @State private var watcher = StatusFileWatcher()
    @State private var actions = DaemonActions()
    @State private var loginItem = LoginItemController()
    @State private var quickCapture = QuickCaptureController()
    @Environment(\.openWindow) private var openWindow

    init() {
        // Honour `--enable-login-item` (passed by `vestige ui --login` and the init /
        // daemon-install boot prompt): register the Login Item once on first launch.
        LoginItemController().registerOnLaunchIfRequested()
    }

    var body: some Scene {
        MenuBarExtra {
            MenuView(watcher: watcher, actions: actions, loginItem: loginItem)
        } label: {
            menuBarLabel
        }
        .menuBarExtraStyle(.window)

        // V0.5.2: a real window kept open independent of the popover.
        Window("Vestige", id: "workspace") {
            WorkspaceView(watcher: watcher)
        }

        // EXP-7: quick-capture window opened by the ⌥⌘N global hotkey.
        Window("Quick Capture", id: "quick-capture") {
            QuickCaptureView(watcher: watcher)
        }
        .windowResizability(.contentSize)
        .defaultPosition(.center)
    }

    /// Menu-bar label: brain/pause glyph with an EXP-1 activity pulse and an EXP-2 candidate
    /// badge overlay.
    private var menuBarLabel: some View {
        Image(systemName: watcher.menuBarSymbol)
            .symbolEffect(.bounce, value: watcher.activityTick)
            .overlay(alignment: .topTrailing) {
                if watcher.candidateBadge > 0 {
                    Text("\(watcher.candidateBadge)")
                        .font(.system(size: 8, weight: .bold))
                        .foregroundStyle(.white)
                        .padding(2)
                        .background(Circle().fill(.red))
                        .offset(x: 6, y: -6)
                        .accessibilityIdentifier("candidate-badge")
                }
            }
            .onAppear { quickCapture.registerHotkey() }
            .onChange(of: quickCapture.openRequests) { _, _ in
                openWindow(id: "quick-capture")
            }
    }
}
