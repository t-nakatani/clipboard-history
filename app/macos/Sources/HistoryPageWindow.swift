import Foundation

/// Bounded, deduplicating summary window. Cursor state always belongs to the
/// oldest page currently reached, even after newer rows are evicted.
struct HistoryPageWindow {
    private(set) var rows: [ClipSummaryDto] = []
    private(set) var nextCursor: HistoryCursorDto?
    private(set) var hasMore = false
    let maximumCount: Int

    init(maximumCount: Int) {
        precondition(maximumCount > 0)
        self.maximumCount = maximumCount
    }

    mutating func apply(_ page: HistoryPageDto, reset: Bool) {
        if reset {
            rows = Array(page.items.prefix(maximumCount))
        } else {
            var known = Set(rows.map(\.id))
            for item in page.items where known.insert(item.id).inserted {
                rows.append(item)
            }
            if rows.count > maximumCount {
                rows.removeFirst(rows.count - maximumCount)
            }
        }
        nextCursor = page.nextCursor
        hasMore = page.hasMore
    }
}
