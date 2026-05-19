// @main entry — MenuBarExtra hosting the Vestige daemon status menu.

import SwiftUI

@main
struct VestigeApp: App {
    @State private var watcher = StatusFileWatcher()

    var body: some Scene {
        MenuBarExtra { MenuView(watcher: watcher) } label: { Image(systemName: "brain") }
            .menuBarExtraStyle(.window)
    }
}
