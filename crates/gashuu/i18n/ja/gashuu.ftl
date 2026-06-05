# Japanese Fluent catalog for gashuu.
#
# ID convention: <screen>-<element>[-<variant>], kebab-case.
# Prefixes: settings-, guide- (FirstRunGuide), carousel-, navbar-,
#           shortcuts-, viewer-pill-, stepper-, viewer- (status/dynamic),
#           notice-, common-.
# A11y-only strings get an -a11y suffix.
# Strings shared across screens live under the primary owner's prefix.
# These values were lifted byte-for-byte from the original gettext catalog and
# messages.rs Ja arms (both since removed); parity is pinned by the i18n
# byte-oracle tests.

# ---- settings ----

# SettingsDialog header: shown when editing the current book's per-book settings.
settings-book-title = この本の設定

# SettingsDialog header (global defaults) / NavBar settings-icon a11y /
# ViewerPill settings-icon a11y — one message, primary owner is SettingsDialog.
settings-title = 設定

# Section eyebrows
settings-section-reading = 読み方
settings-section-display = 表示
settings-section-performance = パフォーマンス
settings-section-general = 一般

# Reading section — Direction row
settings-direction-label = 方向
settings-direction-ltr = 左から右
settings-direction-rtl = 右から左
settings-direction-a11y = 読む方向

# Reading section — Spread row
settings-spread-label = ページ表示
settings-spread-single = 単ページ
settings-spread-double = 見開き
settings-spread-auto = 自動
settings-spread-a11y = 見開きモード

# Display section — Cover row
settings-cover-label = 表紙
settings-cover-standalone = 単独
settings-cover-paired = ペア
settings-cover-a11y = 表紙モード

# Display section — Fit row
settings-fit-label = フィット
settings-fit-whole = 全体
settings-fit-width = 幅
settings-fit-actual = 原寸
settings-fit-a11y = フィットモード

# Performance section — rows and footnote
settings-cache-label = キャッシュ（ページ数）
settings-cache-a11y = キャッシュサイズ（ページ数）
settings-preload-label = 先読みページ数
settings-preload-a11y = 先読みページ数
settings-track-recent-label = 最近のファイルを記録
settings-track-recent-a11y = 最近のファイルを記録
settings-performance-note = キャッシュと先読みは次に開いた本から適用されます。

# General section — Language row
settings-language-label = 言語
settings-language-a11y = 表示言語

# Footer: Shortcuts affordance label and its a11y label (also used as the
# ShortcutsOverlay panel header — shortcuts-title is the primary owner there).
settings-shortcuts-label = ⌨ ショートカット

# Footer: Reset to global (per-book settings only)
settings-reset-to-global = 全体設定に戻す

# ---- shortcuts ----

# ShortcutsOverlay panel header / SettingsDialog footer accessible-label.
shortcuts-title = キーボードショートカット

# Multi-line keyboard reference rendered read-only in ShortcutsOverlay.
# File indentation: section headers 4 spaces, body lines 6 spaces.
# Fluent strips the common 4-space prefix from block values, so delivered text
# has: headers 0 spaces (flush), body lines 2 spaces — formerly matched the deleted messages.rs arms.
# Blank lines between sections are preserved naturally.
# Line count must equal the English arm (test: key_bindings_help_is_translated_with_matching_shape).
shortcuts-help =
    ナビゲーション:
      Space = 次のページ    Backspace = 前のページ
      矢印キーは読む方向に従います (左から右: → が次 / 右から左: ← が次)

    モード:
      D = ページ表示 (単ページ → 見開き → 自動)
      R = 読む方向 (左から右 / 右から左)
      C = 表紙レイアウト (単独 / ペア)

    ズームとフィット:
      + / - = ズームイン / アウト    0 = 表示リセット    1 = 原寸    f = フィット切替

    表示:
      T = サムネイル一覧の表示切替

    ライブラリ:
      Up = ライブラリに戻る

    選択:
      x = 選択モードへ    Space = 選択を切替
      Cmd/Ctrl+A = 表示中をすべて選択 / すべて解除
      Delete / Backspace = 選択した本を削除
      Esc = 選択モードを終了

# ---- guide ----

# FirstRunGuide overlay
guide-welcome = gashuu へようこそ
guide-intro = かんたんな使い方ガイド:
guide-open = 開く: ツールバーの「フォルダーを開く…」「アーカイブを開く…」(CBZ/ZIP/CBR/RAR) ボタンを使います。
guide-turn-pages = ページ送り: Space = 次へ、Backspace = 前へ。矢印キーは読む方向に従います。
guide-modes = モード: D = ページ表示 (単ページ → 見開き → 自動)、R = 読む方向 (左から右 / 右から左)、C = 表紙レイアウト。
guide-zoom-fit = ズームとフィット: + / - でズーム、0 でリセット、1 で原寸、f でフィット切替。ホイールはカーソル位置でズーム、ドラッグで移動します。
guide-thumbnails = サムネイル: T で一覧の表示を切替。サムネイルをクリックするとそのページへ移動します。
guide-settings = 設定: ツールバーから設定ダイアログを開くと、これらの設定をいつでも変更できます。
guide-got-it = わかりました

# ---- carousel ----

# Empty library state (0 books)
carousel-empty-title = ライブラリは空です
carousel-empty-subtitle = 本を追加して本棚を始めましょう。
carousel-empty-cta = 追加するフォルダー / ファイルを選択
# Files-only CTA wording for platforms without the combined picker (non-macOS).
carousel-empty-cta-files = 追加するファイルを選択

# No-results state (library has books but active filter matches none)
carousel-no-results-title = 一致する本がありません
carousel-no-results-hint = 別のキーワードで検索してください。

# Idle bottom-strip label: total library size, shown when no transient notice
# occupies the strip. { $n } は蔵書の総冊数（n > 0 のときのみ合成）。
# Japanese has no plural inflection, so a single *[other] variant carries every
# count. The select wrapper mirrors the English plural shape so the en/ja
# argument-set parity test sees a matching select selector on both sides.
library-count =
    { $n ->
       *[other] { $n } 冊
    }

# ---- navbar ----

# SearchField placeholder and a11y labels (all three uses in NavBar.slint)
navbar-search-placeholder = ライブラリを検索
navbar-search-a11y = ライブラリを検索

# NavItem a11y labels for the action capsules. add-books labels the single
# combined capsule on macOS (one NSOpenPanel picks files and folders);
# add-files/add-folder label the two separate capsules on other platforms.
navbar-add-files-a11y = ファイルを追加
navbar-add-folder-a11y = フォルダーを追加
navbar-add-books-a11y = 本を追加

# NavBar settings capsule a11y — deduped to settings-title.

# ---- viewer-pill ----

# PageJumpField a11y label
viewer-pill-goto-page-a11y = ページへ移動

# Thumbnail capsule a11y label
viewer-pill-thumbnails-a11y = サムネイル表示を切替

# Settings capsule a11y — deduped to settings-title.

# ---- stepper ----

# Accessible labels for the decrease/increase buttons; { $label } is the
# parent SettingRow's a11y-label (e.g. "キャッシュサイズ（ページ数）").
# Named arg handles word order: Japanese is verb-final ({ $label }を減らす).
stepper-decrease = { $label }を減らす
stepper-increase = { $label }を増やす

# ---- common ----

# Close button used in SettingsDialog and ShortcutsOverlay footers.
common-close = 閉じる

# ---- confirm-delete ----

# Cancel button in the bulk-delete confirmation dialog.
confirm-delete-cancel = キャンセル

# Dialog title: { $n } は削除する本の冊数。
confirm-delete-title = { $n } 冊の本を削除しますか？

# Overflow line when the preview list is truncated: { $n } は表示されていない本の冊数。
confirm-delete-more = …ほか { $n } 冊

# Explanatory body: what gets removed and what stays on disk.
confirm-delete-keep-files = ライブラリ登録・読書位置・ページ設定・カバーキャッシュを削除します。元のファイルはディスクに残ります。

# Warning shown when the currently open book is in the delete set.
confirm-delete-open-book = 開いている本が含まれます。削除すると閉じます。

# Warning shown when the selection spans books outside the current search filter.
# { $n } は検索フィルターの外にある本の冊数。
confirm-delete-outside-search = 現在の検索に含まれない { $n } 冊を含みます。

# ---- viewer ----
# Dynamic status-line messages (mapped to the former msg_* functions of the deleted src/messages.rs).

# Static status strings
viewer-no-folder = フォルダーが開かれていません
viewer-no-images = フォルダーに画像がありません
# Library-screen status when Down is pressed but no book is open yet.
viewer-no-open-book = 開いている本がありません

# Compact spread-mode labels for the status line's [mode · direction] tail
viewer-spread-single = 単ページ
viewer-spread-double = 見開き
viewer-spread-auto = 自動

# Compact reading-direction labels for the status line
viewer-direction-ltr = 左→右
viewer-direction-rtl = 右→左

# Parameterized status/error strings
viewer-open-error = エラー: { $error }
viewer-page-unavailable = （ページ { $page } は表示できません）
viewer-decode-error = デコードエラー: { $error }

# ---- bookmark-ribbon ----

# BookmarkRibbon アクセシブルラベル — ライブラリカバーの「続きから読む」サインポスト。
continue-reading = 続きから読む

# ---- notice ----
# Parameterized notice strings (mapped to the former msg_* functions of the deleted src/messages.rs).

# No leading space for Japanese: full-width parens act as the separator.
# (Matched the deleted messages.rs Ja arm exactly: "（zip-slip または上限超過）")
notice-skipped-detail-archive = （zip-slip または上限超過）
# Space between { $n } and 件 matched the deleted messages.rs Ja arm exactly.
notice-entries-skipped = { $n } 件のエントリをスキップしました{ $detail }
notice-failed-save-settings = 設定を保存できませんでした: { $error }
notice-failed-save-library = ライブラリを保存できませんでした: { $error }
notice-could-not-save-settings = 設定を保存できませんでした: { $error }
notice-load-failed = { $what } を読み込めませんでした。初期状態で起動します。
# Em dash (U+2014) preserved byte-identically from messages.rs.
notice-already-in-library = すでにライブラリにあります — 新しい本は追加されませんでした。
# Notice when the NavBar bookmark capsule is clicked but no continue-reading
# bookmark is registered (or it points at a book no longer in the library).
notice-bookmark-none = ブックマークが登録されていません
notice-added-books = { $n } 冊の本を追加しました
notice-added-books-save-failed = { $n } 冊の本を追加しましたが、ライブラリを保存できませんでした: { $error }

# Notice after a successful bulk delete. { $n } は削除した本の冊数。
notice-deleted-books = { $n } 冊を削除しました

# Notice when the library save failed after a bulk delete, meaning nothing was
# actually removed. { $error } はエラーの詳細。
notice-delete-save-failed = ライブラリを保存できませんでした: { $error } — 削除は行われていません。ファイルは無傷です。

# ---- selection ----

# Toolbar count label shown when zero books are selected (just entered selection mode).
selection-mode-label = 選択モード
# Toolbar count label when all selected books are in the current visible projection.
selection-count = { $n } 件選択中
# Toolbar count label when some selected books are outside the visible search projection.
# $m is the number of selected books that are outside (off-screen / filtered out).
selection-count-outside = { $n } 件選択中（うち { $m } 件は検索外）
# Toolbar toggle button: select all visible books.
selection-select-all = すべて選択
# Toolbar toggle button: deselect all visible books (shown when all visible are selected).
selection-deselect-all = 選択を解除
# NavBar button that enters selection mode.
selection-enter = 選択
# A11y label for the toolbar button that exits selection mode.
selection-exit-a11y = 選択モードを終了

# DangerButton label in the SelectionToolbar; also the confirm-button label in
# the bulk-delete ConfirmDialog (bound via Strings.selection-delete).
selection-delete = 削除
