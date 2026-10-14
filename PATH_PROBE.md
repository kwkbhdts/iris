# Claude 祖先経路診断 v2

`iris-path-probe.exe` は Claude Desktop (`Claude.exe`) の手動フォーカスした空のEditから、前面ルートまでの一本だけを調べます。旧ChatGPT向けpath v1を置き換える限定診断です。通常版・オート送信版の対象は変更しません。**SetFocus・文字入力・Enter送信・IME書込みはありません。** 自動探索や会話欄の用途判定は行いません。

## 採取手順

1. 通常のIrisと他の試作を終了し、本exeを起動します。`build=claude-path-readonly-v2`、`targets=Claude.exe`、`stage=ready` を確認します。
2. Claudeの**空の会話入力欄そのもの**をクリックし、未確定文字なし・マウス停止を確認。約2秒待って修飾キーなしのF1を一回押して離し、その後も約2秒無操作で待ちます。今回は本文クリック状態では採取しません。
3. 終了後にコンソールへ戻り、表示内容を本人が確認してから、起動行から `stage=end` までを共有します。ClassNameに私的な値があれば、その値を `[redacted]` として、どの属性を伏せたか残してください。本文・会話名・スクリーンショットは不要です。
4. 別会話の空の入力欄、Claude再起動後の空の入力欄で同じ採取を各一回行うと、目印の安定性を比較できます。画面のラベルとClaudeの版だけを手元で添えます。

終了はコンソールでEnter。F2登録はありません。対象外のF1とF2は通常動作です。長押しでは連続採取しません。移動・ホイールも介入として中止するため、採取中は操作しないでください。`stopped=true` / `restart_required=true` なら終了します。非Edit・未検出でも自動再試行やフォーカス移動はしません。

検索画面でも手動で空の検索欄を選んで一回比較できますが、UIAがListItem等を返す場合は `reference_not_edit` で止まり、ClassNameを取得しません。その結果は検索画面の全構造を比較できた意味ではありません。別途の陰性確認なしに会話欄識別ルールを確定しません。

## 取得・表示範囲

先にRaw Viewの親だけを辿り、**入力要素と前面ルートを含む最大64要素**で所属確認できた場合だけ、その経路について以下を読みます。他の枝や画面全体は列挙しません。

- ControlType、IsControlElement。
- ClassNameの実値。最大128 UTF-16単位まで表示し、改行・端末制御文字等をエスケープします。省略・不正UTF-16・取得失敗は明示し、不完全なメタデータを成功扱いしません。
- AutomationIdの有無だけ（`true` / `false` / `unknown`）。判定のためAPIから値を一時取得しますが、生値・長さ・ハッシュは表示せず、要求をまたいで保持しません。

Name・Value・Text・本文・入力値・会話名・タイトル・URL・FrameworkId・ARIA自由文は取得しません。空欄は利用者の確認条件です。PID・HWND・実行パスも表示しません。深さは観測位置であり、固定selectorには使いません。

**ClassNameにも私的な情報が含まれる可能性があります。共有前に確認してください。** 表示はローカルコンソールだけで、自動ログ保存・通信・アップロードはありません。128単位は表示上限であり、APIが返す元文字列の取得量の上限ではありません。メモリーやコンソール履歴からの安全な消去・匿名化は保証しません。

前面・介入ガード、全体1500ms、UIA接続／トランザクション200ms、最後の実フォーカス一致確認を維持します。API応答期限やツリーの不可分な観測を保証するものではありません。`OBSERVED` は診断完了だけを示し、`chat_input_identified=false` を維持します。

## 検証と配布

`cargo fmt --check`、`cargo test --locked --features focus-probe`、`cargo check --locked --features focus-probe --all-targets --target x86_64-pc-windows-msvc` を使用します。Windows実機動作は未検証です。

CI artifact `iris-path-probe-windows-x64` にexe・本手順書・ライセンス2種・SHA256SUMSを同梱します。Windows版情報とアップロード直前のファイル一覧・ハッシュをCIで検証します。
