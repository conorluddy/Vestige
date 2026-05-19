// Fixture-driven decode test. Catches Rust-side schema drift at Swift compile/test time.
// Rust source of truth: crates/vestige-daemon/src/ipc/status_file.rs.

import XCTest
@testable import Vestige

final class DaemonStatusTests: XCTestCase {

    private func loadFixture(_ name: String) throws -> Data {
        let bundle = Bundle(for: DaemonStatusTests.self)
        guard let url = bundle.url(forResource: name, withExtension: "json", subdirectory: "Fixtures")
            ?? bundle.url(forResource: name, withExtension: "json") else {
            XCTFail("missing fixture: \(name).json")
            return Data()
        }
        return try Data(contentsOf: url)
    }

    func test_decode_v1_fixture() throws {
        let data = try loadFixture("daemon.status.v1")
        let status = try DaemonStatus.recommendedDecoder.decode(DaemonStatus.self, from: data)

        XCTAssertEqual(status.schemaVersion, 1)
        XCTAssertEqual(status.version, "0.5.0")
        XCTAssertEqual(status.pid, 12345)
        XCTAssertEqual(status.uptimeSecs, 3600)
        XCTAssertEqual(status.projects.count, 2)
        XCTAssertEqual(status.nextJobs.count, 2)
    }

    func test_decode_projectFields() throws {
        let data = try loadFixture("daemon.status.v1")
        let status = try DaemonStatus.recommendedDecoder.decode(DaemonStatus.self, from: data)

        let vestige = status.projects[0]
        XCTAssertEqual(vestige.projectId, "proj_vestige")
        XCTAssertEqual(vestige.projectName, "Vestige")
        XCTAssertEqual(vestige.repoRoot, "/Users/test/Development/Vestige")
        XCTAssertNotNil(vestige.lastEmbedRun)
        XCTAssertNil(vestige.lastPruneRun)
        XCTAssertEqual(vestige.pendingEmbeds, 0)

        let grapla = status.projects[1]
        XCTAssertNil(grapla.lastEmbedRun, "null timestamps must decode as nil, not distantPast")
        XCTAssertEqual(grapla.pendingEmbeds, 7)
    }

    func test_decode_jobKindRawValues() throws {
        let data = try loadFixture("daemon.status.v1")
        let status = try DaemonStatus.recommendedDecoder.decode(DaemonStatus.self, from: data)

        XCTAssertEqual(status.nextJobs[0].kind, .embed)
        XCTAssertNil(status.nextJobs[0].projectId, "absent project_id must decode as nil")

        XCTAssertEqual(status.nextJobs[1].kind, .candidateTtl, "snake_case 'candidate_ttl' must map to .candidateTtl")
        XCTAssertEqual(status.nextJobs[1].projectId, "proj_grapla")
    }

    func test_decode_toleratesUnknownFields() throws {
        // Additive evolution rule: a future daemon adding a field must not break this app.
        let json = """
        {
          "schema_version": 1, "version": "0.6.0", "pid": 1, "started_at": "2026-01-01T00:00:00Z",
          "uptime_secs": 0, "projects": [], "next_jobs": [],
          "future_unknown_field": "this should not break decoding"
        }
        """.data(using: .utf8)!
        XCTAssertNoThrow(try DaemonStatus.recommendedDecoder.decode(DaemonStatus.self, from: json))
    }
}
