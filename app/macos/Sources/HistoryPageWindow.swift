import Foundation

/// Bounded, deduplicating summary window. Anchors are derived from the rows
/// that remain in memory, so evicting either edge preserves both directions.
struct HistoryPageWindow {
    private(set) var rows: [ClipSummaryDto] = []
    private(set) var hasMoreOlder = false
    private(set) var hasMoreNewer = false
    let maximumCount: Int

    init(maximumCount: Int) {
        precondition(maximumCount > 0)
        self.maximumCount = maximumCount
    }

    var olderAnchor: HistoryCursorDto? { rows.last.map(Self.cursor) }
    var newerAnchor: HistoryCursorDto? { rows.first.map(Self.cursor) }

    mutating func reset(with page: HistoryPageDto) {
        rows = Array(page.items.prefix(maximumCount))
        hasMoreOlder = page.hasMore || page.items.count > maximumCount
        hasMoreNewer = false
    }

    mutating func appendOlder(_ page: HistoryPageDto) {
        var known = Set(rows.map(\.id))
        rows.append(contentsOf: page.items.filter { known.insert($0.id).inserted })
        if rows.count > maximumCount {
            rows.removeFirst(rows.count - maximumCount)
            hasMoreNewer = true
        }
        hasMoreOlder = page.hasMore
    }

    mutating func prependNewer(_ page: HistoryPageDto) {
        var known = Set(rows.map(\.id))
        let additions = page.items.filter { known.insert($0.id).inserted }
        rows.insert(contentsOf: additions, at: 0)
        if rows.count > maximumCount {
            rows.removeLast(rows.count - maximumCount)
            hasMoreOlder = true
        }
        hasMoreNewer = page.hasMore
    }

    mutating func markNewerAvailable() {
        if !rows.isEmpty {
            hasMoreNewer = true
        }
    }

    private static func cursor(_ item: ClipSummaryDto) -> HistoryCursorDto {
        HistoryCursorDto(lastUsedAtMs: item.lastUsedAtMs, id: item.id)
    }
}
