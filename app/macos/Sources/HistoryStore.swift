import Foundation

/// Reported when the feed is asked for data before the store has opened. The
/// panel is usable during startup, so calls can arrive with nowhere to go.
struct HistoryStoreUnavailableError: LocalizedError {
    var errorDescription: String? { "ストレージの準備が完了していません" }
}

/// Every store call the history feed makes.
///
/// `HistoryStoreClient` is the production implementation. The protocol exists so
/// the feed's paging and refresh policy can be exercised against a stub, since
/// the real client resolves its own paths under Application Support and cannot
/// be pointed at a scratch directory.
///
/// Every completion runs on the main queue. Rows, the image preview cache and
/// the table view are all touched from those completions without further
/// synchronisation, so a stub has to honour this too.
protocol HistoryStore: AnyObject {
    func capture(
        representations: [RepresentationDto],
        copiedAtMs: Int64,
        completion: @escaping (Result<PersistedCapture, Error>) -> Void
    )

    func recentPage(
        cursor: HistoryCursorDto?,
        direction: PageDirectionDto,
        limit: UInt32,
        completion: @escaping (Result<HistoryPageDto, Error>) -> Void
    )

    func searchPage(
        query: String,
        mode: SearchModeDto,
        cursor: HistoryCursorDto?,
        direction: PageDirectionDto,
        limit: UInt32,
        completion: @escaping (Result<HistoryPageDto, Error>) -> Void
    )

    func delete(id: Int64, completion: @escaping (Result<Bool, Error>) -> Void)

    func select(id: Int64, completion: @escaping (Result<[RepresentationDto], Error>) -> Void)

    func imagePreview(id: Int64, completion: @escaping (Result<RepresentationDto?, Error>) -> Void)
}

extension HistoryStoreClient: HistoryStore {}
