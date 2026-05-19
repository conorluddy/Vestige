// Short relative-time formatter for menu-bar UI: "3m ago", "just now", "never".

import Foundation

enum RelativeTime {
    private static let formatter: RelativeDateTimeFormatter = {
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .abbreviated
        return f
    }()

    public static nonisolated func short(from date: Date?, now: Date = Date()) -> String {
        guard let date else { return "never" }
        let elapsed = now.timeIntervalSince(date)
        if elapsed < 5 { return "just now" }
        return formatter.localizedString(for: date, relativeTo: now)
    }
}
