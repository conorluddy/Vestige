// Codable mirror of the Vestige daemon status schema. See crates/vestige-daemon/src/ipc/status_file.rs.

import Foundation

struct DaemonStatus: Codable, Equatable, Hashable {
    var schemaVersion: UInt32
    var version: String
    var pid: UInt32
    var startedAt: Date
    var uptimeSecs: UInt64
    var projects: [ProjectStatus]
    var nextJobs: [ScheduledJob]

    static let recommendedDecoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }()
}

struct ProjectStatus: Codable, Equatable, Hashable {
    var projectId: String
    var projectName: String
    var repoRoot: String
    var lastEmbedRun: Date?
    var lastPruneRun: Date?
    var lastTtlRun: Date?
    var pendingEmbeds: UInt64
}

struct ScheduledJob: Codable, Equatable, Hashable {
    var kind: JobKind
    var projectId: String?
    var at: Date
}

enum JobKind: String, Codable, Equatable, Hashable, CaseIterable {
    case embed = "embed"
    case prune = "prune"
    case candidateTtl = "candidate_ttl"
}
