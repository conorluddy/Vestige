// Read/write bridge to memory data for the experience layer (EXP-5/6/7).
//
// Open question from #88 resolved: the bridge is a **CLI shell-out**, not a new daemon socket
// method. The daemon stays a control surface (V0.5.2 non-goal: "no memory browser / new write
// path on the socket"). Reads run `vestige recall/search --json`; the one write path (EXP-7
// quick capture) runs `vestige note add` — both route through existing core APIs, never raw
// SQL. Every invocation runs with the project's repo root as cwd so `vestige` resolves the
// right project from its `.vestige/config.toml`.

import Foundation

/// A compact memory card returned by `vestige recall/search --json`.
struct MemoryCardDTO: Identifiable, Hashable {
    let id: String
    let title: String
    let kind: String
}

enum MemoryBridge {
    /// Recent memories for the project rooted at `repoRoot` (EXP-5 ticker).
    static func recent(repoRoot: String, limit: Int = 8) async throws -> [MemoryCardDTO] {
        let out = try await runJSON(repoRoot: repoRoot, args: ["list", "--json", "--limit", "\(limit)"])
        return parseCards(out)
    }

    /// Search memories in the project rooted at `repoRoot` (EXP-6 ⌘-search).
    static func search(_ query: String, repoRoot: String, limit: Int = 15) async throws -> [MemoryCardDTO] {
        guard !query.trimmingCharacters(in: .whitespaces).isEmpty else { return [] }
        let out = try await runJSON(repoRoot: repoRoot, args: ["search", query, "--json", "--limit", "\(limit)"])
        return parseCards(out)
    }

    /// Capture a note into the project rooted at `repoRoot` (EXP-7 quick capture). The one
    /// write path — routes through `vestige note add`, never raw SQL. Returns the new id.
    @discardableResult
    static func captureNote(_ body: String, repoRoot: String) async throws -> String {
        let out = try await runJSON(repoRoot: repoRoot, args: ["note", "add", body, "--json"])
        guard let obj = out as? [String: Any], let id = (obj["memory_id"] as? String) ?? (obj["id"] as? String) else {
            throw BridgeError.captureFailed
        }
        return id
    }

    // MARK: - Private

    enum BridgeError: LocalizedError {
        case binaryNotFound
        case captureFailed
        case commandFailed(Int32)
        var errorDescription: String? {
            switch self {
            case .binaryNotFound: return "`vestige` binary not found in PATH"
            case .captureFailed: return "capture did not return a memory id"
            case .commandFailed(let status): return "Vestige command failed (exit \(status))"
            }
        }
    }

    /// Run a `vestige` subcommand with `--json` in `repoRoot` and return the parsed object.
    private static func runJSON(repoRoot: String, args: [String]) async throws -> Any {
        guard let bin = await DaemonActions.resolveBinary() else { throw BridgeError.binaryNotFound }
        return try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                let process = Process()
                process.executableURL = URL(fileURLWithPath: bin)
                process.arguments = args
                process.currentDirectoryURL = URL(fileURLWithPath: repoRoot)
                let pipe = Pipe()
                process.standardOutput = pipe
                process.standardError = FileHandle.nullDevice
                do {
                    try process.run()
                    let data = pipe.fileHandleForReading.readDataToEndOfFile()
                    process.waitUntilExit()
                    guard process.terminationStatus == 0 else {
                        throw BridgeError.commandFailed(process.terminationStatus)
                    }
                    continuation.resume(returning: try JSONSerialization.jsonObject(with: data))
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    /// Parse the list array and search results envelope emitted by the CLI.
    static func parseCards(_ response: Any) -> [MemoryCardDTO] {
        let object = response as? [String: Any] ?? [:]
        let array = (response as? [[String: Any]])
            ?? (object["memories"] as? [[String: Any]])
            ?? (object["results"] as? [[String: Any]])
            ?? (object["cards"] as? [[String: Any]])
            ?? []
        return array.compactMap { row in
            guard let id = row["id"] as? String else { return nil }
            let title = (row["title"] as? String)
                ?? (row["one_liner"] as? String)
                ?? (row["body"] as? String)
                ?? id
            let kind = (row["type"] as? String) ?? (row["kind"] as? String) ?? "note"
            return MemoryCardDTO(id: id, title: title, kind: kind)
        }
    }
}

/// Resolve the "active" project from a status snapshot for the read/write bridge: the project
/// with the most recent `lastMemoryAt`, falling back to the first supervised project.
enum ActiveProject {
    static func resolve(from status: DaemonStatus?) -> ProjectStatus? {
        guard let projects = status?.projects, !projects.isEmpty else { return nil }
        return projects.max { a, b in
            (a.lastMemoryAt ?? .distantPast) < (b.lastMemoryAt ?? .distantPast)
        }
    }
}
