// Minimal Unix-domain JSON-RPC 2.0 client for the Vestige daemon control socket.
//
// The V0.5.1 menu-bar MVP was status-file-only (read-only). V0.5.2 adds this client so the
// menu can *act*: kick a sweep, pause/resume, reload config. The daemon speaks
// newline-delimited JSON-RPC 2.0 over ~/.vestige/daemon.sock (one request → one response,
// short-lived connection) — see crates/vestige-daemon/src/ipc/server.rs.
//
// Implemented over POSIX sockets (Darwin) rather than Network.framework: a Unix-domain
// one-shot request/response is simplest and most predictable this way, and it keeps the
// dependency surface to Foundation + Darwin.

import Foundation

enum DaemonSocketError: LocalizedError {
    case connectFailed(String)
    case ioFailed(String)
    case decodeFailed(String)
    case rpc(code: Int, message: String)

    var errorDescription: String? {
        switch self {
        case .connectFailed(let m): return "Could not reach the daemon: \(m). Is it running?"
        case .ioFailed(let m): return "Daemon socket I/O failed: \(m)"
        case .decodeFailed(let m): return "Could not decode the daemon response: \(m)"
        case .rpc(let code, let message): return "Daemon error \(code): \(message)"
        }
    }
}

struct DaemonSocket {
    let path: String

    init(path: String? = nil) {
        if let path {
            self.path = path
        } else if let env = ProcessInfo.processInfo.environment["VESTIGE_SOCKET"] {
            self.path = env
        } else {
            self.path = FileManager.default.homeDirectoryForCurrentUser
                .appendingPathComponent(".vestige/daemon.sock").path
        }
    }

    // MARK: - High-level RPCs

    /// Run an embed sweep now across all supervised projects.
    func kickEmbed() async throws { _ = try await call(method: "daemon.kick", params: ["job": "embed"]) }

    /// Run a session-log scan now across all supervised projects (V0.5.4).
    func kickScan() async throws { _ = try await call(method: "daemon.kick", params: ["job": "scan"]) }

    /// Re-read the daemon config from disk and apply new cadences on the next tick.
    func reloadConfig() async throws { _ = try await call(method: "daemon.reload_config", params: [:]) }

    /// Pause scheduled ticks until an absolute instant. `until` is sent as RFC-3339 UTC.
    func pause(until: Date) async throws {
        let iso = ISO8601DateFormatter().string(from: until)
        _ = try await call(method: "daemon.pause", params: ["until": iso])
    }

    /// Clear any active pause so scheduled ticks resume immediately.
    func resume() async throws { _ = try await call(method: "daemon.resume", params: [:]) }

    // MARK: - Transport

    /// Send one JSON-RPC request and return the `result` object. Throws on connect/IO/decode
    /// failures or a JSON-RPC `error` envelope. Runs the blocking socket work off the main actor.
    @discardableResult
    func call(method: String, params: [String: Any]) async throws -> [String: Any] {
        let path = self.path
        return try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                do {
                    let result = try Self.roundTrip(path: path, method: method, params: params)
                    continuation.resume(returning: result)
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    /// Blocking connect → write → read → parse. Must be called off the main thread.
    private static func roundTrip(path: String, method: String, params: [String: Any]) throws -> [String: Any] {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw DaemonSocketError.connectFailed("socket() failed") }
        defer { close(fd) }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let pathBytes = Array(path.utf8)
        let capacity = MemoryLayout.size(ofValue: addr.sun_path)
        guard pathBytes.count < capacity else {
            throw DaemonSocketError.connectFailed("socket path too long")
        }
        withUnsafeMutablePointer(to: &addr.sun_path) { ptr in
            ptr.withMemoryRebound(to: CChar.self, capacity: capacity) { dst in
                for (i, b) in pathBytes.enumerated() { dst[i] = CChar(bitPattern: b) }
                dst[pathBytes.count] = 0
            }
        }

        let connectResult = withUnsafePointer(to: &addr) { ptr -> Int32 in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sa in
                connect(fd, sa, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard connectResult == 0 else {
            throw DaemonSocketError.connectFailed(String(cString: strerror(errno)))
        }

        // Build and send the request line.
        let request: [String: Any] = [
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        ]
        var line = try JSONSerialization.data(withJSONObject: request)
        line.append(0x0A) // newline
        try line.withUnsafeBytes { raw in
            var sent = 0
            let total = raw.count
            while sent < total {
                let n = write(fd, raw.baseAddress!.advanced(by: sent), total - sent)
                if n <= 0 { throw DaemonSocketError.ioFailed("write: \(String(cString: strerror(errno)))") }
                sent += n
            }
        }

        // Read until newline / EOF.
        var response = Data()
        var buffer = [UInt8](repeating: 0, count: 4096)
        readLoop: while true {
            let n = read(fd, &buffer, buffer.count)
            if n < 0 { throw DaemonSocketError.ioFailed("read: \(String(cString: strerror(errno)))") }
            if n == 0 { break }
            response.append(contentsOf: buffer[0..<n])
            if buffer[0..<n].contains(0x0A) { break readLoop }
        }

        guard let object = try? JSONSerialization.jsonObject(with: response) as? [String: Any] else {
            throw DaemonSocketError.decodeFailed("response was not a JSON object")
        }
        if let error = object["error"] as? [String: Any] {
            let code = (error["code"] as? Int) ?? -1
            let message = (error["message"] as? String) ?? "unknown error"
            throw DaemonSocketError.rpc(code: code, message: message)
        }
        return (object["result"] as? [String: Any]) ?? [:]
    }
}
