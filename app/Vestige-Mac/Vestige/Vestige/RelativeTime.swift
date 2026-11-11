// Short relative-time formatter for menu-bar UI: "3m ago", "just now", "never".

import Foundation

enum RelativeTime {
    public static nonisolated func short(from date: Date?, now: Date = Date()) -> String {
        guard let date else { return "never" }
        let elapsed = now.timeIntervalSince(date)
        if abs(elapsed) < 5 { return "just now" }
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: date, relativeTo: now)
    }
}
