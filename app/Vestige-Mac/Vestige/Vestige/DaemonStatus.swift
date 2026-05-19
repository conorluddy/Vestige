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
}
