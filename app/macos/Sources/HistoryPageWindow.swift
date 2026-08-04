import Foundation

/// Bounded, deduplicating summary window. Anchors are derived from the rows
/// that remain in memory, so evicting either edge preserves both directions.
struct HistoryPageWindow {
    private(set) var rows: [ClipSummaryDto] = []
    private(set) var hasMoreOlder = false
    private(set) var hasMoreNewer = false
    private var olderContinuation: HistoryCursorDto?
    private var newerContinuation: HistoryCursorDto?
    let maximumCount: Int

    init(maximumCount: Int) {
        precondition(maximumCount > 0)
        self.maximumCount = maximumCount
    }

    var olderAnchor: HistoryCursorDto? { olderContinuation ?? rows.last.map(Self.cursor) }
    var newerAnchor: HistoryCursorDto? { newerContinuation ?? rows.first.map(Self.cursor) }

    mutating func reset(with page: HistoryPageDto) {
        rows = Array(page.items.prefix(maximumCount))
        hasMoreOlder = page.hasMore || page.items.count > maximumCount
        hasMoreNewer = false
        olderContinuation = page.continuationCursor
        newerContinuation = nil
    }

    mutating func appendOlder(_ page: HistoryPageDto) {
        var known = Set(rows.map(\.id))
        rows.append(contentsOf: page.items.filter { known.insert($0.id).inserted })
        if rows.count > maximumCount {
            rows.removeFirst(rows.count - maximumCount)
            hasMoreNewer = true
            newerContinuation = rows.first.map(Self.cursor)
        }
        hasMoreOlder = page.hasMore
        olderContinuation = page.continuationCursor
    }

    mutating func prependNewer(_ page: HistoryPageDto) {
        // A recopy keeps its clip ID but receives a newer timestamp. Remove
        // the stale occurrence before inserting the returned row at its new position.
        let incomingIds = Set(page.items.map(\.id))
        rows.removeAll { incomingIds.contains($0.id) }
        rows.insert(contentsOf: page.items, at: 0)
        if rows.count > maximumCount {
            rows.removeLast(rows.count - maximumCount)
            hasMoreOlder = true
            olderContinuation = rows.last.map(Self.cursor)
        }
        hasMoreNewer = page.hasMore
        newerContinuation = page.continuationCursor
    }

    /// Drops a row that no longer exists, leaving the rest of the window where
    /// it is.
    ///
    /// Paging state is deliberately untouched. Both anchors are derived from the
    /// rows that remain, and removing one row from the middle changes neither
    /// edge the window knows about, so what is resident stays resident and the
    /// reader keeps their place.
    mutating func remove(id: Int64) {
        rows.removeAll { $0.id == id }
    }

    mutating func markNewerAvailable() {
        if !rows.isEmpty {
            hasMoreNewer = true
            newerContinuation = rows.first.map(Self.cursor)
        }
    }

    private static func cursor(_ item: ClipSummaryDto) -> HistoryCursorDto {
        HistoryCursorDto(lastUsedAtMs: item.lastUsedAtMs, id: item.id)
    }
}
