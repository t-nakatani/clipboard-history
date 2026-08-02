import Foundation

/// What the resident rows currently represent.
///
/// Previously this was implicit: a nil `activeSearchQuery` meant "recent", and
/// three separate places re-derived the same answer by trimming the search
/// field. Naming the state keeps that decision in one place.
enum HistoryQuery: Equatable {
    case recent
    case search(text: String, mode: SearchModeDto)
}

/// Owns the history feed: which query is live, which pages are resident, and
/// every call into the store.
///
/// Deliberately free of AppKit. The view layer supplies the one piece of
/// context the refresh policy needs — whether the user is reading rows away
/// from the newest — through `shouldHoldNewestUpdate`.
final class HistoryFeedModel {
    /// Absent until the store finishes opening. The panel is built and can be
    /// shown before that, so every call here tolerates having nowhere to go.
    private var store: HistoryStore?
    private let onStoreActivity: () -> Void
    private let pageSize: UInt32
    private let searchDebounce: TimeInterval

    private var window: HistoryPageWindow
    private var query: HistoryQuery = .recent
    /// Bumped whenever the live query changes, so a page from a superseded
    /// query cannot land on top of the current rows.
    private var generation = 0
    private var isLoadingPage = false
    private var searchTimer: Timer?

    /// Answers whether a newly captured clip should stay off screen because the
    /// user is reading older rows. Absent, the feed always jumps to newest.
    var shouldHoldNewestUpdate: (() -> Bool)?
    /// Wraps a row mutation so the view can keep scroll position and selection
    /// steady across it. `reset` is true when the whole window is replaced, and
    /// `apply` performs the mutation. Unset, the mutation simply runs.
    var updateRows: ((_ reset: Bool, _ apply: () -> Void) -> Void)?
    var statusDidChange: ((HistoryStatus) -> Void)?

    var rows: [ClipSummaryDto] { window.rows }
    var hasMoreNewer: Bool { window.hasMoreNewer }
    var hasMoreOlder: Bool { window.hasMoreOlder }

    init(
        store: HistoryStore? = nil,
        onStoreActivity: @escaping () -> Void,
        pageSize: UInt32 = 50,
        residentRowLimit: Int = 200,
        searchDebounce: TimeInterval = 0.12
    ) {
        self.store = store
        self.onStoreActivity = onStoreActivity
        self.pageSize = pageSize
        self.searchDebounce = searchDebounce
        window = HistoryPageWindow(maximumCount: residentRowLimit)
    }

    deinit {
        searchTimer?.invalidate()
    }

    /// Hands the feed its store once it has finished opening.
    func attach(store: HistoryStore) {
        self.store = store
    }

    /// Records interaction with the store so background maintenance stays out
    /// of the way. Routing every store call through this type is what keeps
    /// this from having to be repeated at each call site in the UI.
    func markActivity() {
        onStoreActivity()
    }

    func cancelPendingSearch() {
        searchTimer?.invalidate()
        searchTimer = nil
    }

    /// Re-runs whatever query is live, from its newest edge.
    func reload() {
        switch query {
        case .recent:
            loadRecent()
        case let .search(text, mode):
            search(text: text, mode: mode)
        }
    }

    /// Debounces a query change from the search controls. Empty text falls back
    /// to the recent feed.
    func scheduleSearch(text: String, mode: SearchModeDto) {
        // Typing is activity even though the request is still debounced. Waiting
        // for the timer would leave a window in which the scheduler still
        // believes the app is deep idle and can put an FTS optimize on the store
        // queue that the imminent search then has to wait behind.
        markActivity()
        searchTimer?.invalidate()
        searchTimer = Timer.scheduledTimer(
            withTimeInterval: searchDebounce,
            repeats: false
        ) { [weak self] _ in
            self?.search(text: text, mode: mode)
        }
    }

    /// Runs a query change immediately, bypassing the debounce.
    func search(text: String, mode: SearchModeDto) {
        cancelPendingSearch()
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            // `loadRecent` records the activity and takes the next generation.
            loadRecent()
            return
        }
        markActivity()
        generation += 1
        guard let store else { return }
        query = .search(text: text, mode: mode)
        let issued = generation
        isLoadingPage = true
        statusDidChange?(.searching)
        store.searchPage(
            query: text,
            mode: mode,
            cursor: nil,
            direction: .older,
            limit: pageSize
        ) { [weak self] result in
            guard let self, issued == self.generation else { return }
            self.isLoadingPage = false
            switch result {
            case let .success(page):
                self.apply(page: page, reset: true)
                self.statusDidChange?(.searchCompleted(count: page.items.count))
                if page.truncated {
                    DispatchQueue.main.async { self.loadPage(.older) }
                }
            case let .failure(error):
                self.statusDidChange?(.failed(error))
            }
        }
    }

    func loadPage(_ direction: PageDirectionDto) {
        guard !isLoadingPage, let store else { return }
        markActivity()
        let cursor: HistoryCursorDto?
        switch direction {
        case .older:
            guard window.hasMoreOlder else { return }
            cursor = window.olderAnchor
        case .newer:
            guard window.hasMoreNewer else { return }
            cursor = window.newerAnchor
        }
        guard let cursor else { return }
        isLoadingPage = true
        let issued = generation
        let completion: (Result<HistoryPageDto, Error>) -> Void = { [weak self] result in
            guard let self, issued == self.generation else { return }
            self.isLoadingPage = false
            switch result {
            case let .success(page):
                self.apply(page: page, reset: false, direction: direction)
                if page.truncated {
                    // The scan hit its budget before filling the page. Continue
                    // from the returned cursor rather than showing a short page.
                    DispatchQueue.main.async { self.loadPage(direction) }
                }
            case let .failure(error):
                self.statusDidChange?(.failed(error))
            }
        }
        switch query {
        case .recent:
            store.recentPage(
                cursor: cursor,
                direction: direction,
                limit: pageSize,
                completion: completion
            )
        case let .search(text, mode):
            store.searchPage(
                query: text,
                mode: mode,
                cursor: cursor,
                direction: direction,
                limit: pageSize,
                completion: completion
            )
        }
    }

    func capture(_ candidate: CapturedClipboardCandidate) {
        guard let store else { return }
        markActivity()
        statusDidChange?(.capturing(
            types: candidate.representationTypes,
            payloadBytes: candidate.payloadBytes,
            identity: candidate.identity
        ))
        let copiedAtMs = Int64(Date().timeIntervalSince1970 * 1_000)
        store.capture(
            representations: candidate.representations,
            copiedAtMs: copiedAtMs
        ) { [weak self] result in
            guard let self else { return }
            switch result {
            case let .success(capture):
                self.statusDidChange?(.captured(
                    inserted: capture.result.inserted,
                    id: capture.result.id,
                    residentCount: capture.recentPage.items.count
                ))
                self.absorb(capture)
            case let .failure(error):
                self.statusDidChange?(.failed(error))
            }
        }
    }

    func delete(id: Int64) {
        guard let store else { return }
        markActivity()
        store.delete(id: id) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(true):
                self.statusDidChange?(.deleted)
                self.reload()
            case .success(false):
                self.reload()
            case let .failure(error):
                self.statusDidChange?(.failed(error))
            }
        }
    }

    /// Reads back everything needed to put a clip on the pasteboard. The caller
    /// shows progress while this runs, so a missing store has to complete with a
    /// failure rather than leave that progress on screen forever.
    func representations(
        for id: Int64,
        completion: @escaping (Result<[RepresentationDto], Error>) -> Void
    ) {
        guard let store else {
            completion(.failure(HistoryStoreUnavailableError()))
            return
        }
        markActivity()
        store.select(id: id, completion: completion)
    }

    func imagePreview(id: Int64, completion: @escaping (RepresentationDto?) -> Void) {
        guard let store else {
            completion(nil)
            return
        }
        markActivity()
        store.imagePreview(id: id) { result in
            completion(try? result.get())
        }
    }

    /// Decides what a freshly captured clip does to the visible rows.
    private func absorb(_ capture: PersistedCapture) {
        if case .search = query {
            // The new clip may or may not match; re-running the search is the
            // only way to find out without duplicating the matcher here.
            reload()
            return
        }
        if shouldHoldNewestUpdate?() == true {
            // Keep the generation so an in-flight edge page can still land.
            window.markNewerAvailable()
            statusDidChange?(.newerAvailable)
            return
        }
        generation += 1
        isLoadingPage = false
        query = .recent
        apply(page: capture.recentPage, reset: true)
    }

    private func loadRecent() {
        query = .recent
        guard let store else { return }
        markActivity()
        generation += 1
        let issued = generation
        isLoadingPage = true
        store.recentPage(
            cursor: nil,
            direction: .older,
            limit: pageSize
        ) { [weak self] result in
            guard let self, issued == self.generation else { return }
            self.isLoadingPage = false
            switch result {
            case let .success(page):
                self.apply(page: page, reset: true)
                self.statusDidChange?(.loaded(
                    count: page.items.count,
                    newestPreview: page.items.first.map { $0.preview ?? $0.kind }
                ))
            case let .failure(error):
                self.statusDidChange?(.failed(error))
            }
        }
    }

    private func apply(
        page: HistoryPageDto,
        reset: Bool,
        direction: PageDirectionDto = .older
    ) {
        let mutate = {
            if reset {
                self.window.reset(with: page)
            } else {
                switch direction {
                case .older: self.window.appendOlder(page)
                case .newer: self.window.prependNewer(page)
                }
            }
        }
        guard let updateRows else {
            mutate()
            return
        }
        updateRows(reset, mutate)
    }
}
