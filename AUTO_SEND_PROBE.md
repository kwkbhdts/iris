# Iris dots向け一回押し送信試作 v1

**このexeは `OK` と Enter を実際に送ります。** `iris-auto-send-probe.exe` は ChatGPT Desktop内dotsの通常画面向けの独立試作です。F1一回で入力欄探索→SetFocus→新しい対象検証→IME確認→一度の入力投入を行います。F2登録は不要です。通常manual版、既存focus-only版は残します。Windows実機での統合動作は未検証です。

## 半角・日本語を各一回だけ試す

1. **送信してよい試験用dots会話**を用意し、通常版・古い試作を終了します。このexeを通常権限で起動し、`build=auto-send-v1` / `sends_OK_Enter=true` / `stage=ready` を確認します。
2. 入力欄が**空、IME未確定文字なし**と確認し、半角の状態にして会話本文をクリック。マウスを止め、修飾キーなしのF1を一回押して離し、約2秒無操作で待ちます。Enterキーも離してください。
3. `OK` が一回送られたか・フォーカスが戻ったか・IME状態を手動確認します。ログの起動行〜`stage=end` と「半角／目視結果」を記録します。
4. 異常や手動確認要求がない場合だけ、空欄を確認して日本語IMEオン・未確定文字なしにし、同じ操作を一回。送信後にIMEが元へ戻ったかも確認します。

`manual_check=true` / `stopped=true` / `restart_required=true` があれば終了し、入力・送信結果・押下状態・IMEを手動確認してください。試作は自動復旧・自動再送しません。コンソールでEnterを押すと終了します。実行中のコピーやマウス操作は避け、本文やスクリーンショットの共有は不要です。

## 探索と限定した移行条件

前面 `ChatGPT.exe` 内で、Control Viewの操作可能な非password・非offscreen Editを検索します。`ProseMirror` の完全空白トークン、Raw祖先の `conversation-footer` → `thread-pane` → 同一要素上の `messaging-root` + `messaging-embedded` → Document → 前面ルートという順序を確認します。深度・focusedクラス・CSSハッシュ・動的IDは固定しません。候補は厳密に一つ、開始時フォーカスも同じDocument所属が必要です。SetFocus直前にも再検索・領域比較し、SetFocusは一回だけです。

SetFocus後に次のどちらかの場合のみ、新しい送信前検証へ移ります。

- 元の要求に介入がなく、世代が変わっていない。
- **原要求開始以降の履歴が欠落なく、SetFocus呼び出し中に受信した前面通知が厳密に2件だけ**。両通知の対象PIDが元と一致し、受信中の実前面と一致すると取得でき、通知対象の順序が「元と異なる非NULL HWND→元HWND」である。受信時刻が不明・区間外・逆順なら拒否。

この2件のパターンは実測に基づく限定ルールで、自己誘発の因果を証明するものではありません。マウス・他キー・他の注入キー・別のF1要求・watchdog・他プロセス・不明情報・欠落・別の最終前面は送信停止です。最終前面はHWND/PID/TIDが一致する必要があります。

元の世代・失効記録は保持し、`original_interrupted=true` のまま `limited_transition=true` と記録します。新しい検証の `permit_epoch` は一度だけ固定し、後の介入を世代の取り直しで消しません。UIA一致は即時・SetFocus完了から約50ms・150msの有界観測で確認し、一致がなければ送信しません。時刻経過だけで通過させません。

## 送信直前とIME

前面・一意な構造一致候補・同じ入力要素・footer/thread/messaging/Document・実フォーカスを改めて検証します。その後、新しいWin32フォーカス／IME所有者／レイアウトのスナップショットを取得します。F1押下前の古いフォーカスはIME処理へ流用しません。

既存manual版と同じモデル・Win32バックエンドで、元IME応答0ならオフ確認のみ、1なら一時オフと応答確認、不明なら入力しません。**IME操作後、SendInput直前にもUIAの全候補検索・領域比較・実フォーカス確認を行い**、さらにWin32対象・固定世代・期限・レイアウトを照合して既存SendInputを呼びます。

Unicode `O`、`K`、Enterの各押下・解放、計6イベントを一括投入します。間に待機せず、部分投入や失敗で再送しません。自分のタグ付き注入イベントだけを既存判定で介入から除外します。このタグはセキュリティ上の本人確認ではありません。他の注入入力は介入です。

元IMEオンで全イベントを投入した場合、既存の暫定100msの復元前待機を行います。待機中も固定世代・前面・期限等を確認し、IME所有者・Win32フォーカス・レイアウト・オフ応答が保たれる場合だけ元オンへ戻して確認します。復元不能・介入・部分投入なら無効化し、手動確認を求めます。終了・期限切れ後に盲目的な復元をしません。IMEをオフにした後に送信前検証が失敗し、オフのまま残る可能性があります。

F1長押しは一要求です。処理中に次の入力を予約せず、長押しの2回目送信・入力投入の再試行・SetFocus再実行・クリックやTabへの代替はありません。

## 期限と結果表示

全体は元のF1から1500ms、IME／入力段階は新しいスナップショットから500msが上限です。IME開始前に全体期限の残り500ms以上を要求し、期限は延長しません。UIA接続／トランザクションは各200ms、IMEメッセージは各50ms、返却候補最大24件、祖先最大64要素、ClassName解析最大4096 UTF-16単位です。UIA検索内部の訪問数やOS応答の厳密な上限、呼び出し中の取消は保証しません。途中終了・エラーは成功としません。

ログは `transition`（元失効と限定移行）、`verify.focus`（一致観測）、`pre_input.verify`（IME後の検証）、`input.result`、`summary`を区別します。`permit_epoch`は新しい検証の基準であり、元要求の失効を取り消す意味ではありません。

- `input_attempted=false / input_inserted=0 / result=NO_INPUT`：投入なし。
- `input_attempted=true / input_inserted=6 / result=INPUT_ENQUEUED`：OSへの6イベント投入。**チャット送信成功の確認ではありません。** `app_send_result=unknown` と表示します。
- 部分投入・0件投入失敗は `INPUT_PARTIAL_OR_FAILED`、自動再送なし。
- `manual_check=true` は入力やIME復元を手動確認する必要があります。投入6件でもその後の介入でこの表示になり得ます。

## 制約

- **空欄・未確定IMEなしは利用者の確認条件**です。本文・入力値を読まないため検査しません。条件が違えば既存文字への追記や未確定文字の確定・取消・送信が起こり得ます。legacy IME応答はTSF全体の状態保証ではありません。
- 同じプロセス内の手動操作が限定した2通知と同じ形になる可能性を排除できません。確認とSendInputは不可分ではなく、SendInputは特定ウィンドウ宛てではありません。最後の確認直後の切替による誤送信、同じHWND内の変化、送信後のアプリ動作は完全には防げません。
- 構造ルールは一回のdots属性観測に基づく暫定版です。他画面との非一致や将来の更新耐性は保証しません。URL・サービス識別ではなく、他アプリや通常ChatGPTへの展開は対象外です。
- 通常権限のみで、昇格・UIAccess回避はしません。既に要求した入力やIME変更は取り消せず、強制終了時の復元も保証しません。
- Name・Value・Textの内容・タイトル・URL・AutomationId・ARIA自由文は取得しません。構造照合にClassNameは読みますが文字列は出力せず、固定分類・数値・真偽値・HRESULTのみをログにします。通信・ログファイル・自動アップロード・自動起動・設定エンジンはありません。

## 開発・配布

```sh
cargo fmt --check
cargo test --locked --features focus-probe
cargo check --locked --features focus-probe --target x86_64-pc-windows-msvc
cargo build --locked --release --features focus-probe --bin iris-auto-send-probe --target x86_64-pc-windows-msvc
```

CIの `iris-auto-send-probe-windows-x64` にexe・この説明・依存ライセンス・SHA-256を同梱します。CRTは静的リンク、追加ランタイム不要、Windows標準DLLは使用します。CIはexeを起動せず、実際の送信・IME復元は利用者の実機試験まで未検証です。通常manual版の動作・main・正式リリースは維持します。
