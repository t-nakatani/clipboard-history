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
    case capturing(types: [String], payloadBytes: Int, identity: String)
    case captured(inserted: Bool, id: Int64, residentCount: Int)
    case newerAvailable
    case rejected(CaptureFilterDecisionDto)
    case loaded(count: Int, newestPreview: String?)
    case searching
    case searchCompleted(count: Int)
    case restoring
    case restored(preview: String)
    case deleted
    case failed(Error)

    var headline: String? {
        switch self {
        case .preparingStorage:
            return "ストレージを準備中…"
        case .idle:
            return "コピー待機中"
        case .recovering:
            return "前回の終了状態を検証中…"
        case .recoveryRebuilt, .recoveryCompleted, .recoveryFailed:
            return nil
        case .capturing:
            return "保存中…"
        case let .captured(inserted, _, _):
            return inserted ? "履歴へ保存" : "既存履歴を先頭へ移動"
        case .newerAvailable:
            return "新しい履歴あり"
        case .rejected:
            return "保存対象外"
        case let .loaded(count, _):
            return "履歴 \(count)件を読み込み済み"
        case .searching:
            return "検索中…"
        case let .searchCompleted(count):
            return "検索結果 \(count)件"
        case .restoring:
            return "復元中…"
        case .restored:
            return "Pasteboardへ復元"
        case .deleted:
            return "履歴を削除"
        case .failed:
            return "ストレージエラー"
        }
    }

    var detail: String? {
        switch self {
        case .preparingStorage, .idle, .recovering, .newerAvailable, .searching, .restoring, .deleted:
            return nil
        case let .recoveryRebuilt(quarantinePath):
            return "破損した履歴を隔離して再構築 · \(quarantinePath ?? "隔離先を確認できません")"
        case let .recoveryCompleted(reclaimedFiles):
            return "起動時リカバリー完了 · 孤児payload \(reclaimedFiles)件を回収"
        case let .recoveryFailed(description):
            return "起動時リカバリーに失敗: \(description)"
        case let .capturing(types, payloadBytes, identity):
            let shortIdentity = String(identity.prefix(12))
            return "\(types.joined(separator: ", "))\n\(payloadBytes) bytes · hash \(shortIdentity)…"
        case let .captured(_, id, residentCount):
            return "clip #\(id) · 最近\(residentCount)件をメモリ保持"
        case let .rejected(decision):
            switch decision {
            case .rejectConcealed:
                return "concealed markerを型一覧から検知しました。payload bytesは読み取っていません。"
            case .rejectTransient:
                return "transient markerを型一覧から検知しました。payload bytesは読み取っていません。"
            case .accept:
                return nil
            }
        case let .loaded(_, newestPreview):
            return newestPreview
        case .searchCompleted:
            return "完全一致・前方一致・正確な部分一致のみ"
        case let .restored(preview):
            return preview
        case let .failed(error):
            return error.localizedDescription
        }
    }
}
