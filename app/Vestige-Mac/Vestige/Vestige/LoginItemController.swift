// "Start at login" backed by SMAppService.mainApp (macOS 13+).
//
// Registration is idempotent. The menu's toggle reflects `.status`; `vestige ui --login` and
// the init/daemon-install boot prompt launch the app with `--enable-login-item`, which
// `VestigeApp` forwards to `registerOnLaunchIfRequested()`.

import Foundation
import ServiceManagement

@MainActor
@Observable
final class LoginItemController {
    /// Whether the app is currently a registered Login Item.
    private(set) var isEnabled: Bool = false

    init() {
        refresh()
    }

    /// Re-read the current registration status from `SMAppService`.
    func refresh() {
        isEnabled = SMAppService.mainApp.status == .enabled
    }

    /// Register (enable) or unregister (disable) the Login Item. Idempotent.
    func setEnabled(_ enabled: Bool) {
        do {
            if enabled {
                if SMAppService.mainApp.status != .enabled {
                    try SMAppService.mainApp.register()
                }
            } else {
                if SMAppService.mainApp.status == .enabled {
                    try SMAppService.mainApp.unregister()
                }
            }
        } catch {
            NSLog("Vestige: Login Item \(enabled ? "register" : "unregister") failed: \(error)")
        }
        refresh()
    }

    /// Honour the `--enable-login-item` launch argument by registering once on first launch.
    func registerOnLaunchIfRequested() {
        if CommandLine.arguments.contains("--enable-login-item") {
            setEnabled(true)
        }
    }
}
