import Foundation

/// Every store call the history feed makes.
///
/// `HistoryStoreClient` is the production implementation. The protocol exists so
/// the feed's paging and refresh policy can be exercised against a stub, since
/// the real client resolves its own paths under Application Support and cannot
/// be pointed at a scratch directory.
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
