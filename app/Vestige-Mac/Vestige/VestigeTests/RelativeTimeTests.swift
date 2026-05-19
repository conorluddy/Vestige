// Unit tests for RelativeTime.short — the three guard branches (nil, just-now, formatter).

import XCTest
@testable import Vestige

final class RelativeTimeTests: XCTestCase {

    func test_nilDate_returnsNever() {
        XCTAssertEqual(RelativeTime.short(from: nil), "never")
    }

    func test_recentPast_returnsJustNow() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        let twoSecondsAgo = now.addingTimeInterval(-2)
        XCTAssertEqual(RelativeTime.short(from: twoSecondsAgo, now: now), "just now")
    }

    func test_olderPast_returnsFormatterOutput() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        let tenMinutesAgo = now.addingTimeInterval(-600)
        let output = RelativeTime.short(from: tenMinutesAgo, now: now)
        // Locale-safe: we don't assert exact string ("10 min ago" vs "10m ago"
        // vs localised). We assert it routed to the formatter, not a guard.
        XCTAssertNotEqual(output, "never")
        XCTAssertNotEqual(output, "just now")
        XCTAssertTrue(output.contains(where: \.isNumber), "expected a number in \(output)")
    }

    func test_futureDate_returnsFormatterOutput() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        let inTenMinutes = now.addingTimeInterval(600)
        let output = RelativeTime.short(from: inTenMinutes, now: now)
        XCTAssertNotEqual(output, "never")
        XCTAssertNotEqual(output, "just now")
    }
}
