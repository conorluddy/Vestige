import XCTest
@testable import Vestige

final class MemoryBridgeTests: XCTestCase {
    func testRecentListAcceptsTheCLIArray() {
        let cards = MemoryBridge.parseCards([["id": "mem_a", "one_liner": "A decision", "type": "decision"]])
        XCTAssertEqual(cards.count, 1)
        XCTAssertEqual(cards.first?.title, "A decision")
        XCTAssertEqual(cards.first?.kind, "decision")
    }

    func testSearchAcceptsTheResultsEnvelope() {
        let cards = MemoryBridge.parseCards(["results": [["id": "mem_a", "title": "Found", "type": "note"]]])
        XCTAssertEqual(cards.first?.title, "Found")
        XCTAssertEqual(cards.first?.id, "mem_a")
    }

    func testEmptyAndUnrelatedResponsesDoNotInventMemories() {
        XCTAssertTrue(MemoryBridge.parseCards([]).isEmpty)
        XCTAssertTrue(MemoryBridge.parseCards(["warnings": []]).isEmpty)
    }
}
