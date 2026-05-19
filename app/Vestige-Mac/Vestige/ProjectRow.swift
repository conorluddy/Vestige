// Single row in the supervised-projects list showing embed health at a glance.

import SwiftUI

struct ProjectRow: View {
    let project: ProjectStatus

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(embedColor)
                .frame(width: 8, height: 8)

            VStack(alignment: .leading, spacing: 2) {
                Text(project.projectName)
                    .font(.body)
                Text("embedded \(RelativeTime.short(from: project.lastEmbedRun))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Spacer()

            if project.pendingEmbeds > 0 {
                Text("\(project.pendingEmbeds) pending")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    // MARK: - Private helpers

    private var embedColor: Color {
        switch project.pendingEmbeds {
        case 0: .green
        case 1..<10: .orange
        default: .red
        }
    }
}
