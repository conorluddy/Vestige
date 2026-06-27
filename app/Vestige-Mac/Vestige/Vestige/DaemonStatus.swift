// Codable mirror of the Vestige daemon status schema. See crates/vestige-daemon/src/ipc/status_file.rs.

import Foundation

struct DaemonStatus: Codable, Equatable, Hashable {
    let schemaVersion: UInt32
    let version: String
    let pid: UInt32
    let startedAt: Date
    let uptimeSecs: UInt64
    let projects: [ProjectStatus]
    let nextJobs: [ScheduledJob]

    // Additive (V0.5.2): RFC-3339 instant until which scheduled ticks are suppressed.
    // Absent / null when the daemon is running normally. Synthesized Codable decodes an
    // Optional via decodeIfPresent, so older daemons that omit the field decode to `nil`.
    let pausedUntil: Date?

    /// `true` when the daemon is paused and the pause has not yet elapsed.
    var isPaused: Bool {
        guard let pausedUntil else { return false }
        return pausedUntil > Date()
    }

    static let recommendedDecoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }()
}

struct ProjectStatus: Codable, Equatable, Hashable {
    let projectId: String
    let projectName: String
    let repoRoot: String
    let lastEmbedRun: Date?
    let lastPruneRun: Date?
    let lastTtlRun: Date?
    let pendingEmbeds: UInt64

    // Additive fields — older daemons (pre-0.5.x enrichment) omit these.
    let memoryCount: UInt64
    let candidateCount: UInt64
    let lastMemoryAt: Date?

    enum CodingKeys: String, CodingKey {
        case projectId, projectName, repoRoot
        case lastEmbedRun, lastPruneRun, lastTtlRun
        case pendingEmbeds, memoryCount, candidateCount, lastMemoryAt
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        projectId = try c.decode(String.self, forKey: .projectId)
        projectName = try c.decode(String.self, forKey: .projectName)
        repoRoot = try c.decode(String.self, forKey: .repoRoot)
        lastEmbedRun = try c.decodeIfPresent(Date.self, forKey: .lastEmbedRun)
        lastPruneRun = try c.decodeIfPresent(Date.self, forKey: .lastPruneRun)
        lastTtlRun = try c.decodeIfPresent(Date.self, forKey: .lastTtlRun)
        pendingEmbeds = try c.decode(UInt64.self, forKey: .pendingEmbeds)
        memoryCount = try c.decodeIfPresent(UInt64.self, forKey: .memoryCount) ?? 0
        candidateCount = try c.decodeIfPresent(UInt64.self, forKey: .candidateCount) ?? 0
        lastMemoryAt = try c.decodeIfPresent(Date.self, forKey: .lastMemoryAt)
    }
}

struct ScheduledJob: Codable, Equatable, Hashable {
    let kind: JobKind
    let projectId: String?
    let at: Date
}

enum JobKind: String, Codable, Equatable, Hashable, CaseIterable {
    case embed = "embed"
    case prune = "prune"
    case candidateTtl = "candidate_ttl"
    case sessionLogScan = "session_log_scan"
}
