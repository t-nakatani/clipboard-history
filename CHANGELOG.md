# Changelog

このファイルは[Keep a Changelog](https://keepachangelog.com/ja/1.1.0/)の形式に従い、バージョンは[Semantic Versioning](https://semver.org/lang/ja/)に従います。

## [Unreleased]

## [0.1.0-alpha.1] - 2026-08-03

最初の配布可能なalphaです。署名・notarizationは未対応で、Apple Silicon向けのunsigned buildだけを配布します。

### Added

- メニューバーからの1クリックで開く半透明のAppKit履歴panel
- Pasteboard type一覧をpayload読取前に検査する二段階capture
- BLAKE3によるclip identityと`recopy = touch`
- SQLite + FTS5 trigramによる部分一致検索
- 100,000件のtransactional retentionと16KiB超payloadのcontent-addressed file保存
- tombstone queueによる遅延GCと孤児payload回収
- 50件単位の双方向keyset paging（保持summaryは最大200件）
- 最大96pxのJPEG画像previewと上限付きdecoded image cache
- Periodic/Idle/DeepIdleのbackground maintenance（WAL checkpoint、bounded incremental vacuum、条件付きFTS5 optimize）
- unclean shutdown時だけ実行するbackground recoveryと破損storeの隔離・再構築
- ホバーでの行選択、シングルクリックまたはReturnでの復元、検索欄が空のときのDelete/Backspaceでの削除
- tag pushでunsigned appを公開するrelease workflow

### Changed

- 検索モードの選択をやめ、常に正確な部分一致で検索する。modeの判断は`QueryPlanner`がneedle長から行う
- substring検索のLIKEに対するESCAPE句を、needleがワイルドカードを含むときだけ出すようにした。無条件のESCAPEがtrigram索引を無効化していた
- 検索で大文字小文字を区別しない

### Fixed

- orphan GCが不正なファイル名でパニックする問題
- capture境界でサイズ制限を強制し、oversizedなクリップを拒否する
- 削除したクリップの内容をSQLiteとpayload storageから確実に除去する
- 復元前にpayloadの整合性とディスク上のサイズを検証する
- concealed/transient pasteboardのフィルタ範囲を拡大する

### Known issues

- 履歴を復元するとその書き込み自体をcaptureし直すため、`copy_count`と並び順が意図せず変化する
- 操作の失敗がUIへ表示されない。store初期化、capture、restore、delete、searchの失敗は無言で落ちる
- 署名・notarizationがないため、ダウンロードしたappはGatekeeperに隔離される
- Apple Silicon向けbuildのみ。Intel Macは対象外
- ディスク容量上限、pin、global shortcut、設定画面は未実装
- 複数Pasteboard itemは先頭itemだけを保存する

[Unreleased]: https://github.com/t-nakatani/clipboard-history/compare/v0.1.0-alpha.1...HEAD
[0.1.0-alpha.1]: https://github.com/t-nakatani/clipboard-history/releases/tag/v0.1.0-alpha.1
