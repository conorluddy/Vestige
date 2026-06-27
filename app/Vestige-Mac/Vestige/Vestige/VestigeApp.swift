// @main entry — MenuBarExtra + persistent workspace window for the Vestige daemon.

import SwiftUI

@main
struct VestigeApp: App {
    @State private var watcher = StatusFileWatcher()
    @State private var actions = DaemonActions()
    @State private var loginItem = LoginItemController()

    init() {
        // Honour `--enable-login-item` (passed by `vestige ui --login` and the init /
        // daemon-install boot prompt): register the Login Item once on first launch.
        LoginItemController().registerOnLaunchIfRequested()
    }

    var body: some Scene {
        MenuBarExtra {
            MenuView(watcher: watcher, actions: actions, loginItem: loginItem)
        } label: {
            Image(systemName: watcher.menuBarSymbol)
        }
        .menuBarExtraStyle(.window)

        // V0.5.2: a real window kept open independent of the popover.
        Window("Vestige", id: "workspace") {
            WorkspaceView(watcher: watcher)
        }
    }
}
