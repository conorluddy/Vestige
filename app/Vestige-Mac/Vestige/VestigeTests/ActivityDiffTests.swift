// EXP-1 — unit tests for the StatusFileWatcher activity-diff logic.
//
// The diff drives the menu-bar icon pulse: it must fire on real work (sweep completion,
// candidate growth) and stay quiet on no-op refreshes.

import XCTest
@testable import Vestige

final class ActivityDiffTests: XCTestCase {

    private func project(
        id: String = "proj_a",
        pendingEmbeds: UInt64 = 0,
        memoryCount: UInt64 = 0,
        candidateCount: UInt64 = 0,
        lastEmbedRun: Date? = nil
    ) throws -> ProjectStatus {
        let iso = ISO8601DateFormatter()
        let embed = lastEmbedRun.map { iso.string(from: $0) }
        let json = """
        {
          "project_id": "\(id)", "project_name": "A", "repo_root": "/tmp/a",
          \(embed.map { "\"last_embed_run\": \"\($0)\"," } ?? "")
          "pending_embeds": \(pendingEmbeds),
          "memory_count": \(memoryCount), "candidate_count": \(candidateCount)
        }
        """.data(using: .utf8)!
        return try DaemonStatus.recommendedDecoder.decode(ProjectStatus.self, from: json)
    }

    private func status(_ projects: [ProjectStatus]) throws -> DaemonStatus {
        // Build a DaemonStatus by decoding — keeps us honest about the Codable shape.
        let projectsJSON = try projects.map { p -> String in
            let data = try JSONEncoder().encode(EncodableProject(p))
            return String(data: data, encoding: .utf8)!
        }.joined(separator: ",")
        let json = """
        {
          "schema_version": 1, "version": "0.5.4", "pid": 1,
          "started_at": "2026-01-01T00:00:00Z", "uptime_secs": 0,
          "projects": [\(projectsJSON)], "next_jobs": []
        }
        """.data(using: .utf8)!
        return try DaemonStatus.recommendedDecoder.decode(DaemonStatus.self, from: json)
    }

    // Minimal re-encoder so we can compose a DaemonStatus from ProjectStatus values.
    private struct EncodableProject: Encodable {
        let p: ProjectStatus
        init(_ p: ProjectStatus) { self.p = p }
        func encode(to encoder: Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            try c.encode(p.projectId, forKey: .project_id)
            try c.encode(p.projectName, forKey: .project_name)
            try c.encode(p.repoRoot, forKey: .repo_root)
            try c.encode(p.pendingEmbeds, forKey: .pending_embeds)
            try c.encode(p.memoryCount, forKey: .memory_count)
            try c.encode(p.candidateCount, forKey: .candidate_count)
            if let d = p.lastEmbedRun {
                try c.encode(ISO8601DateFormatter().string(from: d), forKey: .last_embed_run)
            }
        }
        enum CodingKeys: String, CodingKey {
            case project_id, project_name, repo_root, pending_embeds, memory_count, candidate_count, last_embed_run
        }
    }

    func test_noChange_isNoActivity() throws {
        let p = try project(memoryCount: 10)
        let prev = try status([p])
        let next = try status([p])
        XCTAssertFalse(StatusFileWatcher.activityDetected(previous: prev, next: next))
    }

    func test_candidateGrowth_isActivity() throws {
        let prev = try status([try project(candidateCount: 1)])
        let next = try status([try project(candidateCount: 3)])
        XCTAssertTrue(StatusFileWatcher.activityDetected(previous: prev, next: next))
    }

    func test_memoryGrowth_isActivity() throws {
        let prev = try status([try project(memoryCount: 5)])
        let next = try status([try project(memoryCount: 6)])
        XCTAssertTrue(StatusFileWatcher.activityDetected(previous: prev, next: next))
    }

    func test_pendingDrop_isActivity() throws {
        let prev = try status([try project(pendingEmbeds: 4)])
        let next = try status([try project(pendingEmbeds: 0)])
        XCTAssertTrue(StatusFileWatcher.activityDetected(previous: prev, next: next))
    }

    func test_embedRunAdvances_isActivity() throws {
        let earlier = Date(timeIntervalSince1970: 1_000_000)
        let later = Date(timeIntervalSince1970: 2_000_000)
        let prev = try status([try project(lastEmbedRun: earlier)])
        let next = try status([try project(lastEmbedRun: later)])
        XCTAssertTrue(StatusFileWatcher.activityDetected(previous: prev, next: next))
    }
}
