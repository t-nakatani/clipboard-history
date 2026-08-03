import Foundation

/// Something worth telling the user about, reported as a state rather than as
/// formatted text so wording stays in the view layer.
///
/// A case whose `headline` is `nil` only refreshes the detail line and leaves
/// the headline to whatever runs next. Startup recovery uses that: its outcome
/// has to survive the history load that immediately follows it.
enum HistoryStatus {
    case preparingStorage
    case idle
    case recovering
    case recoveryRebuilt(quarantinePath: String?)
    case recoveryCompleted(reclaimedFiles: UInt64)
    case recoveryFailed(String)
    case capturing(types: [String], payloadBytes: Int)
    case captured(inserted: Bool, residentCount: Int)
    case newerAvailable
    case rejected(CaptureFilterDecisionDto)
    case rejectedOversized(CaptureRejectionReason)
    case loaded(count: Int, newestPreview: String?, keepsDetail: Bool)
    case searching
    case searchCompleted(count: Int)
    case restoring
    case restored(preview: String)
    case deleted(preview: String?)
    case failed(Error)

    /// How hard the status has to try to reach the user.
    ///
    /// The status row lives in the panel, which is closed most of the time. An
    /// `important` status raised while it is closed would otherwise be buried by
    /// the save and search chatter that follows before the user ever looks.
    enum Priority {
        case routine
        case important
    }

    var priority: Priority {
        switch self {
        // A silently missing clip is confusing, so the explanation has to
        // survive until the user next opens the panel.
        case .recoveryRebuilt, .recoveryFailed, .failed, .rejectedOversized:
            return .important
        default:
            return .routine
        }
    }

    var headline: String? {
        switch self {
        case .preparingStorage:
            return "ストレージを準備中…"
        case .idle:
            return "コピー待機中"
        case .recovering:
            return "前回の終了状態を検証中…"
        case .recoveryRebuilt:
            return "履歴が破損したため再構築しました"
        case .recoveryFailed:
            return "起動時チェックに失敗しました"
        // The load that follows recovery owns the headline; only the detail
        // below carries the outcome, and that load is asked to keep it.
        case .recoveryCompleted:
            return nil
        case .capturing:
            return "保存中…"
        case let .captured(inserted, _):
            return inserted ? "履歴へ保存" : "既存履歴を先頭へ移動"
        case .newerAvailable:
            return "新しい履歴あり"
        case .rejected:
            return "保存対象外"
        case .rejectedOversized:
            return "サイズ上限を超えるため保存対象外"
        case let .loaded(count, _, _):
            return "履歴 \(count)件を読み込み済み"
        case .searching:
            return "検索中…"
        case let .searchCompleted(count):
            return "検索結果 \(count)件"
        case .restoring:
            return "復元中…"
        case .restored:
            return "クリップボードへ復元"
        case .deleted:
            return "履歴を削除"
        case .failed:
            return "ストレージエラー"
        }
    }

    var detail: String? {
        switch self {
        case .preparingStorage, .idle, .recovering, .searching, .restoring:
            return nil
        case let .recoveryRebuilt(quarantinePath):
            return quarantinePath.map { "以前の履歴は復元できません。退避先: \($0)" }
                ?? "以前の履歴は復元できません。退避先を確認できませんでした。"
        case let .recoveryCompleted(reclaimedFiles):
            return reclaimedFiles > 0
                ? "起動時チェック完了 · 不要になったデータ \(reclaimedFiles)件を整理しました"
                : "起動時チェック完了 · 履歴に問題は見つかりませんでした"
        case let .recoveryFailed(description):
            return description
        case let .capturing(types, payloadBytes):
            return "\(Self.readableKind(for: types)) · \(Self.readableBytes(payloadBytes))"
        case let .captured(_, residentCount):
            return "履歴 \(residentCount)件"
        case .newerAvailable:
            return "上へスクロールすると表示されます"
        case let .rejected(decision):
            switch decision {
            case .rejectConcealed:
                return "パスワードなど秘匿指定された内容のため、中身を読み取らずに破棄しました。"
            case .rejectTransient:
                return "一時的な内容として指定されていたため、中身を読み取らずに破棄しました。"
            case .accept:
                return nil
            }
        case let .rejectedOversized(reason):
            let observed: UInt64
            let limit: UInt64
            let subject: String
            switch reason {
            case let .oversizedRepresentation(observedBytes, limitBytes):
                (observed, limit, subject) = (observedBytes, limitBytes, "形式のひとつ")
            case let .oversizedClip(observedBytes, limitBytes):
                (observed, limit, subject) = (observedBytes, limitBytes, "内容全体")
            }
            return "\(subject)が \(Self.readableBytes(observed)) で、"
                + "上限 \(Self.readableBytes(limit)) を超えています。復元できないため保存しませんでした。"
        case let .loaded(_, newestPreview, keepsDetail):
            guard !keepsDetail else { return nil }
            return newestPreview ?? "履歴はまだありません"
        case .searchCompleted:
            return "完全一致・前方一致・正確な部分一致のみ"
        case let .restored(preview):
            return preview
        case let .deleted(preview):
            return preview
        case let .failed(error):
            return error.localizedDescription
        }
    }

    private static func readableBytes<Value: BinaryInteger>(_ bytes: Value) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(clamping: bytes), countStyle: .file)
    }

    /// Plain-language equivalent of the captured UTI list for the status row.
    private static func readableKind(for types: [String]) -> String {
        let types = Set(types)
        if !types.isDisjoint(with: ["public.png", "public.tiff"]) { return "画像" }
        if types.contains("public.file-url") { return "ファイル" }
        if !types.isDisjoint(with: ["public.rtf", "public.html"]) { return "リッチテキスト" }
        return "テキスト"
    }
}
