// EXP-5 (recent-memories ticker) + EXP-6 (⌘-search) — read-only memory peek.
//
// Both are read-only and explicitly opted-in (they cross the V0.5.2 "control surface, not a
// browser" non-goal deliberately, per #88's note). They route through MemoryBridge (CLI
// shell-out) against the resolved active project.

import SwiftUI

struct MemoryPeekView: View {
    var watcher: StatusFileWatcher

    @State private var query = ""
    @State private var recent: [MemoryCardDTO] = []
    @State private var results: [MemoryCardDTO] = []
    @State private var searching = false
    @FocusState private var searchFocused: Bool

    private var activeProject: ProjectStatus? {
        switch watcher.state {
        case .running(let s), .stale(let s): return ActiveProject.resolve(from: s)
        default: return nil
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            // EXP-6: ⌘-search field.
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Search memories…", text: $query)
                    .textFieldStyle(.plain)
                    .focused($searchFocused)
                    .onSubmit { runSearch() }
                    .accessibilityIdentifier("memory-search-field")
                if searching { ProgressView().controlSize(.small) }
            }
            .padding(8)
            .background(.quaternary, in: RoundedRectangle(cornerRadius: 8))

            if !query.isEmpty {
                cardList(results, empty: "No matches.")
            } else {
                // EXP-5: recent-memories ticker.
                Text("Recent")
                    .font(.caption.bold())
                    .foregroundStyle(.secondary)
                cardList(recent, empty: "No memories yet.")
            }
        }
        .onChange(of: query) { _, newValue in
            if newValue.isEmpty { results = [] }
        }
        .task(id: activeProject?.projectId) { await loadRecent() }
        // ⌘F focuses the search field.
        .onAppear { }
    }

    @ViewBuilder
    private func cardList(_ cards: [MemoryCardDTO], empty: String) -> some View {
        if cards.isEmpty {
            Text(empty).font(.caption).foregroundStyle(.tertiary)
        } else {
            ForEach(cards) { card in
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(card.kind.uppercased())
                        .font(.system(.caption2, design: .monospaced))
                        .foregroundStyle(.tertiary)
                        .frame(width: 64, alignment: .leading)
                    Text(card.title)
                        .font(.caption)
                        .lineLimit(2)
                    Spacer(minLength: 0)
                }
                .accessibilityIdentifier("memory-card")
            }
        }
    }

    private func loadRecent() async {
        guard let project = activeProject else { recent = []; return }
        recent = await MemoryBridge.recent(repoRoot: project.repoRoot)
    }

    private func runSearch() {
        guard let project = activeProject else { return }
        searching = true
        Task {
            results = await MemoryBridge.search(query, repoRoot: project.repoRoot)
            searching = false
        }
    }
}
