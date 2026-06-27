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
                        .fill(health.color)
                        .frame(width: 8, height: 8)
                        .accessibilityLabel(health.label)

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

            // EXP-4: memory-growth sparkline (app-local history). Hidden until ≥ 2 samples.
            let series = MemoryHistoryStore.shared.history(for: project.projectId)
            if series.count >= 2 {
                HStack(spacing: 8) {
                    Text("Growth")
                        .frame(width: 44, alignment: .leading)
                        .foregroundStyle(.tertiary)
                    Sparkline(values: series, color: health.color)
                }
            }

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

    // EXP-3: status-dot semantics live in ProjectHealth, not inline here.
    private var health: ProjectHealth { ProjectHealth.of(project) }

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
