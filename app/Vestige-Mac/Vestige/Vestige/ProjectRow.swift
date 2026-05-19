// Expandable row for a supervised project: glance state, click to reveal full daemon detail.

import SwiftUI
import AppKit

struct ProjectRow: View {
    let project: ProjectStatus
    @State private var isExpanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button(action: { isExpanded.toggle() }) {
                HStack(spacing: 8) {
                    Circle()
                        .fill(statusColor)
                        .frame(width: 8, height: 8)

                    VStack(alignment: .leading, spacing: 2) {
                        Text(project.projectName)
                            .font(.body)
                        Text(subtitle)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }

                    Spacer()

                    if project.pendingEmbeds > 0 {
                        Text("\(project.pendingEmbeds) pending")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }

                    Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if isExpanded {
                detail
                    .padding(.leading, 16)
                    .transition(.opacity)
            }
        }
        .animation(.easeInOut(duration: 0.12), value: isExpanded)
    }

    // MARK: - Detail panel

    private var detail: some View {
        VStack(alignment: .leading, spacing: 4) {
            detailRow("ID", project.projectId)
            detailRow("Path", tildePath(project.repoRoot))
            detailRow("Last memory", absoluteOrNever(project.lastMemoryAt))
            detailRow("Embed", absoluteOrNever(project.lastEmbedRun))
            detailRow("Prune", absoluteOrNever(project.lastPruneRun))
            detailRow("TTL",   absoluteOrNever(project.lastTtlRun))

            Button {
                let url = URL(fileURLWithPath: project.repoRoot)
                NSWorkspace.shared.activateFileViewerSelecting([url])
            } label: {
                Label("Reveal in Finder", systemImage: "folder")
                    .font(.caption)
            }
            .buttonStyle(.borderless)
            .padding(.top, 2)
        }
        .font(.caption)
        .foregroundStyle(.secondary)
    }

    private func detailRow(_ label: String, _ value: String) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Text(label)
                .frame(width: 44, alignment: .leading)
                .foregroundStyle(.tertiary)
            Text(value)
                .textSelection(.enabled)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 0)
        }
    }

    // MARK: - Formatting

    private var subtitle: String {
        if project.memoryCount == 0 && project.candidateCount == 0 {
            return "embedded \(RelativeTime.short(from: project.lastEmbedRun))"
        }
        let memories = "\(project.memoryCount) \(project.memoryCount == 1 ? "memory" : "memories")"
        if project.candidateCount > 0 {
            return "\(memories) · \(project.candidateCount) candidates"
        }
        return memories
    }

    private var statusColor: Color {
        // Pending-embed backlog dominates: anything queued is more urgent than staleness.
        if project.pendingEmbeds >= 10 { return .red }
        if project.pendingEmbeds >= 1 { return .orange }

        // Age-of-last-embed: amber after 7 days, hidden by inactive-collapse after 30.
        guard let lastEmbed = project.lastEmbedRun else { return .gray }
        let ageDays = Date().timeIntervalSince(lastEmbed) / 86_400
        if ageDays > 7 { return .yellow }
        return .green
    }

    private func tildePath(_ path: String) -> String {
        NSString(string: path).abbreviatingWithTildeInPath
    }

    private func absoluteOrNever(_ date: Date?) -> String {
        guard let date else { return "never" }
        return Self.absoluteFormatter.string(from: date)
    }

    private static let absoluteFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateStyle = .short
        formatter.timeStyle = .short
        return formatter
    }()
}
