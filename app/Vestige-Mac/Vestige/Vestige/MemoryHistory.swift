// EXP-4 — per-project memory-count history + a tiny sparkline.
//
// History source decision (the open question in #117): the app keeps its own local ring
// buffer keyed by project id, persisted in UserDefaults. This avoids a daemon schema change
// — the daemon stays a thin status surface — at the cost of only accumulating history while
// the app has been running. Capped length keeps storage bounded.

import SwiftUI

/// App-local ring buffer of per-project memory counts, persisted across launches.
final class MemoryHistoryStore {
    static let shared = MemoryHistoryStore()

    /// Max samples retained per project.
    private let cap = 60
    private let defaultsKey = "memoryHistory.v1"
    private let defaults = UserDefaults.standard

    /// projectId → [memoryCount] oldest-first.
    private var series: [String: [Int]]

    private init() {
        if let raw = defaults.data(forKey: defaultsKey),
           let decoded = try? JSONDecoder().decode([String: [Int]].self, from: raw) {
            series = decoded
        } else {
            series = [:]
        }
    }

    /// Append the current memory counts from a status snapshot. Coalesces no-op samples (an
    /// unchanged count is not re-appended) so the sparkline reflects change, not poll cadence.
    func record(_ status: DaemonStatus) {
        var changed = false
        for project in status.projects {
            let count = Int(project.memoryCount)
            var samples = series[project.projectId] ?? []
            if samples.last != count {
                samples.append(count)
                if samples.count > cap { samples.removeFirst(samples.count - cap) }
                series[project.projectId] = samples
                changed = true
            }
        }
        if changed, let raw = try? JSONEncoder().encode(series) {
            defaults.set(raw, forKey: defaultsKey)
        }
    }

    /// The retained samples for a project (oldest-first), or `[]` if none yet.
    func history(for projectId: String) -> [Int] {
        series[projectId] ?? []
    }
}

/// A minimal sparkline of integer samples. Degrades gracefully: < 2 points renders nothing.
struct Sparkline: View {
    let values: [Int]
    var color: Color = .accentColor

    var body: some View {
        GeometryReader { geo in
            if values.count >= 2 {
                path(in: geo.size)
                    .stroke(color, style: StrokeStyle(lineWidth: 1.5, lineJoin: .round))
            }
        }
        .frame(height: 18)
        .accessibilityHidden(true)
    }

    private func path(in size: CGSize) -> Path {
        let minV = values.min() ?? 0
        let maxV = values.max() ?? 1
        let range = max(1, maxV - minV)
        let stepX = size.width / CGFloat(values.count - 1)

        var path = Path()
        for (i, v) in values.enumerated() {
            let x = CGFloat(i) * stepX
            // Invert Y so growth goes up.
            let y = size.height - (CGFloat(v - minV) / CGFloat(range)) * size.height
            if i == 0 { path.move(to: CGPoint(x: x, y: y)) }
            else { path.addLine(to: CGPoint(x: x, y: y)) }
        }
        return path
    }
}
