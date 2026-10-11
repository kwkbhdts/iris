# Iris 登録不要フォーカス試作 v1

`iris-auto-focus-probe.exe` は ChatGPT Desktop内dotsの通常画面を対象に、本文から **F1で入力欄へ戻る**独立した試作です。F2登録は不要で、毎回探索します。**文字入力・Enter送信・IME変更はしません。** 通常の手動送信版Irisとは別のexeです。Windows実機は未検証です。

## 最初の一回

1. 通常版と古い試作を終了し、このexeを通常権限で起動。`build=auto-focus-v1` と `stage=ready` を確認します。
2. ChatGPT Desktop内dotsの通常会話を開き、入力欄が空・IME未確定文字なしを確認して**会話本文をクリック**します。
3. マウスを止めて修飾キーなしの **F1を一回押して離し、約2秒無操作**で待ちます。F2は押しません。
4. その後コンソールへ戻り、起動行から `stage=end` までコピーし、目視で入力欄に戻ったかを添えてください。

終了はコンソールでEnterです。F1長押しで連続実行せず、処理中に次の要求を予約しません。ChatGPT以外・修飾キー付きのF1は通常動作を保ち、F2も通常動作です。`stopped=true` / `restart_required=true` なら終了してください。本文や画面全体のスクリーンショットは不要です。

## 暫定の固定ルール

- 前面の実行ファイル名が `ChatGPT.exe`。これは製品のセキュリティ上の本人確認ではありません。
- 前面ルート配下で、Control View所属のEdit、enabled、keyboard-focusable、非offscreen、非passwordをUIA条件検索します。
- ClassNameが完全な空白区切りトークン `ProseMirror` を持つこと。`ProseMirror-focused` の有無は問いません。
- Raw祖先を内側から辿り、別々の要素で `conversation-footer` → `thread-pane` → `messaging-root` と `messaging-embedded` の両方、さらに外側のDocument、前面ルートへの到達を確認します。中間要素は許容し、深度・兄弟番号は固定しません。
- 全候補の検証が完了し、このルールの一致が**厳密に1件**の場合だけ進みます。先頭Editは選びません。0件・複数件・未完了・取得失敗は停止します。本文側フォーカスも同じ前面ルートとDocumentに属することが必要です。

根拠は一回の祖先属性観測です。CSSモジュールのハッシュ、動的なID、位置調整用クラスは固定しません。**この構造が他画面には出ないこと、今後の版でも変わらないことは保証しません。** dots画面のサービス識別やURL判定を実装したものではなく、通常ChatGPT・他サービスへの対応を主張しません。入力欄の空欄・未確定文字なしも利用者の確認条件です。

Control Viewで省かれる祖先があるため、候補検索とRaw祖先検証を使い分けます。会話本文のRaw View全ノードをクライアント側で列挙する処理はありません。ただしUIAの条件検索は内部で広範囲を走査する可能性があります。

## 再検証と範囲制限

SetFocus直前に同じ検索をもう一度行い、一意性、同じ入力要素、同じfooter・thread・messaging領域・Documentであることを比較します。実フォーカスが操作開始時から変わっていないことと編集属性も再確認します。各UIA呼び出し前に前面・介入・期限を確認します。

- 要求全体1500ms（再検索・遅延観測も含む）、UIA接続／トランザクション各200ms。
- 条件検索から返ったEditは最大24件、各祖先経路は入力要素を含め最大64要素。
- ClassNameは最大4096 UTF-16単位で解析し、超過・不正UTF16は検索を失敗させます。元文字列はログに出しません。
- 24件上限はUIA検索の内部訪問件数や返却前の割当を制限するものではありません。APIの遅延中に厳密な取消はできず、watchdogでは試作を無効化します。期限切れや中断後は新しい操作を行いません。

SetFocusは一要求につき一回だけです。確認と呼び出しは不可分ではなく、直後の画面更新・競合や呼び出し後の遅い反映は完全には防げません。終了しても既に要求済みのフォーカス移動を取り消せる保証はありません。再試行・クリック・Tab操作への代替はしません。

## 結果は四つに分ける

`stage=summary` の主な項目は次です。

- `search=unique / no_match / ambiguous / incomplete`：最初の探索結果。`revalidated=true` は直前の再検証も通ったこと。
- `set_focus=not_called / returned_ok / returned_error`：SetFocusの応答。`returned_ok`だけで移動確認済みとは扱いません。
- `immediate_focus=unknown / match / mismatch`、`late_focus=not_run / match / mismatch`：登録ではなく今回選んだ要素とのUIA一致。最後に完了した観測を表し、即時結果は遅延結果で書き換えません。
- `interrupted=true` と `reason`：前面通知・キー／マウス介入・期限などによる中断。

SetFocus直後に不一致で介入がない場合だけ、完了から約50ms・150msを目標に最大2回読み取り観測します。一致したら追加観測を止め、SetFocusは再実行しません。待機は最大5msずつ区切り、同じ総期限とガードを使います。`FOCUS_OBSERVED`は即時または遅延時点でUIA一致を観測した意味で、カーソルの利用可能性・将来のフォーカス維持・送信可能性の保証ではありません。

既存v4ではDesktopでSetFocus中に同一プロセスの前面通知がありました。**今回もそれを一括で無視せず、世代リセットもしません。** 例えば目視で戻っても `set_focus=returned_ok / immediate_focus=unknown / interrupted=true / result=INTERRUPTED` になる可能性があります。これは呼び出し後の安全確認を中止した意味で、「動かなかった」とは断定しません。未確認は成功表示にせず、観測が不一致のままなら `FOCUS_UNCONFIRMED` です。

## 取得する情報と開発

UIAの型・所属・編集属性・ClassNameを読みますが、Name・Value・Textの内容・タイトル・URL・AutomationId・ARIA自由文は取得しません。ログは固定分類・数値・真偽値・HRESULTだけで、ClassNameやPID/HWND/実行パス自体は出しません。ログファイル・通信・自動アップロード・設定ファイルはありません。

既存の公式windowsバインディングのOle/Variant機能を条件検索用に有効化しています。新しいソフトや追加ランタイムの導入は不要です。CRTは静的リンク、Windows標準DLLは使います。

```sh
cargo fmt --check
cargo test --locked --features focus-probe
cargo check --locked --features focus-probe --target x86_64-pc-windows-msvc
cargo build --locked --release --features focus-probe --bin iris-auto-focus-probe --target x86_64-pc-windows-msvc
```

CIの `iris-auto-focus-probe-windows-x64` にexe・この説明・依存ライセンス・SHA-256を同梱します。CIはexeを起動しません。通常版のコード・main・正式リリースは変更しません。
