# ADR 0006: menu bar iconの1クリックで履歴を表示する

## Status

Accepted

## Context

Maccyは`NSStatusItem.button.action`を`performStatusItemClick`へ直接接続し、通常クリックから`panel.toggle(..., at: .statusItem)`を呼ぶ。menuを開いてから履歴項目を選ぶ二段階操作ではない。

application shellの初期実装はstatus itemへ`NSMenu`を設定し、「状態を表示」を選ばないと履歴panelが開かなかったため、通常導線が2クリックになっていた。

参照したMaccy revisionは`abaeb23f57c30711526f0a2b81ced9dfa2750e63`の`Maccy/AppDelegate.swift`である。

## Decision

- status itemの左クリックactionを履歴panelの直接toggleへ割り当てる。
- panelはstatus item直下へ配置し、表示と同時に検索欄へfocusする。
- 通常windowや`NSMenu`ではなく、`.nonactivatingPanel`のカスタム`NSPanel`を使う。これによりmenu風の表示と、検索欄・履歴tableの通常のkeyboard操作を両立する。
- panelはタイトルバーを持たない半透明の角丸表示とし、status itemの画面座標を基準に毎回配置する。画面端では`visibleFrame`内へclampする。
- panelは外部click、Escape、履歴復元で閉じる。表示中はstatus itemをhighlightする。
- panelが表示中なら、同じ左クリック1回で閉じる。
- 終了menuは右クリックcontext menuだけに置き、通常の履歴表示導線へ介在させない。
- store初期化中でもpanel自体は開き、準備状態を表示する。初期化完了後は最近50件を表示しておく。

## Consequences

- menu bar iconから履歴一覧までのクリック数はMaccyと同じ1回になる。
- status itemの通常menuへ設定項目を追加しない。設定UIが必要になった場合も右クリックかpanel内へ置く。
- global shortcutを追加する場合も同じpanel toggleを再利用する。
