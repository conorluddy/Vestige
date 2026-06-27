// EXP-3 — centralised status-dot semantics.
//
// The health colour for a project used to live inline in `ProjectRow.statusColor`. It is now
// here, documented in one place, so the popover, the persistent window, and any future surface
// all agree on what green/amber/red mean.

import SwiftUI

/// The single source of truth for a project's health dot.
enum ProjectHealth {
    case healthy   // green  — embeddings current, no backlog
    case stale     // yellow — last embed > 7 days ago
    case backlog   // orange — 1–9 representations pending embed
    case urgent    // red    — 10+ representations pending embed
    case unknown   // gray   — never embedded

    /// Pending-embed backlog dominates staleness: queued work is more urgent than age.
    static func of(_ project: ProjectStatus, now: Date = Date()) -> ProjectHealth {
        if project.pendingEmbeds >= 10 { return .urgent }
        if project.pendingEmbeds >= 1 { return .backlog }
        guard let lastEmbed = project.lastEmbedRun else { return .unknown }
        return now.timeIntervalSince(lastEmbed) / 86_400 > 7 ? .stale : .healthy
    }

    var color: Color {
        switch self {
        case .healthy: return .green
        case .stale:   return .yellow
        case .backlog: return .orange
        case .urgent:  return .red
        case .unknown: return .gray
        }
    }

    /// Accessibility label for the dot.
    var label: String {
        switch self {
        case .healthy: return "Healthy"
        case .stale:   return "Embeddings stale"
        case .backlog: return "Embeddings pending"
        case .urgent:  return "Large embed backlog"
        case .unknown: return "Never embedded"
        }
    }
}
