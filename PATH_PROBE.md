# Iris 祖先経路診断 v1

`iris-path-probe.exe` は ChatGPT Desktop内のdots通常入力欄を調べる、独立した読み取り専用試作です。**フォーカス移動・文字・Enter送信・IME変更はありません。** 入力欄から前面ルートまでの一本だけを読み、全画面列挙はしません。dotsかどうか、空欄かどうかは自動判定せず利用者が確認します。Windows実機は未検証です。

## 一回だけ採取する手順

1. 通常のIris・古い試作を終了し、このexeを起動。`build=path-readonly-v1` と `stage=ready` を確認します。
2. ChatGPT Desktop内のdotsの**通常の空の入力欄**をクリック。IME未確定文字なし・マウス停止を確認し、修飾キーなしの **F1を一回押して離し、約2秒無操作**で待ちます。
3. コンソールに戻り、**表示内容を本人が確認してから**、起動行〜最後の `stage=end` を必要な範囲だけコピーして共有します。私的な値があればその値を `[redacted]` に置き換え、どの属性を伏せたか残してください。スクリーンショットや本文は不要です。

終了はコンソールでEnterです。F2登録は不要。F1長押しでは連続実行せず、ChatGPT以外は通常動作を通します。処理中のマウス・キーボード操作・前面変更は中止理由です。`stopped=true` / `restart_required=true` なら終了してください。

## ログに出す範囲

参照はEditであることを確認します。まず親だけを辿り、前面ルートへ最大64要素以内で到達できた場合に限って、その経路の次の属性を読みます。

- `ControlType`
- `ClassName`、`AutomationId`、`FrameworkId` **の実値**
- `IsControlElement` / `IsContentElement` と、それらによるControl/Content View所属

Name・Value・Textの内容・タイトル・URL・ARIA自由文・他の枝は取得しません。入力値を読まないため、空欄は検査しません。アプリ照合と介入監視には従来のWin32情報を使いますが、PID・HWND・実行パス自体は出力しません。

**識別属性にも、アプリが私的なIDや情報を埋め込む可能性があります。匿名化は保証しません。** 従来の有無だけの診断と異なり、今回は実値を表示するので共有前に確認してください。表示はローカルコンソールだけで、ログファイル保存・自動アップロード・通信はありません。処理中はメモリー内で保持し、終了後にまとめて表示します。通常のプロセスメモリーやコンソール履歴からの安全な消去を保証する機能はありません。

各文字列の表示は最大128 UTF-16単位に制限し、改行・端末制御文字・引用符などをエスケープします。有効なサロゲートペアは上限位置で分断しません。`*_truncated=true` は省略、`*_invalid_utf16=true` は不正文字を置換した値です。これは出力整形であり、私的情報の除去ではありません。UIA自体は属性全体を返すため、取得する元文字列の長さを128に制限するものではありません。

## 期待する形式

次は形式例で、識別文字列は実測値ではありません。

```text
stage=path.scope ... nodes=35 root_reached=true max_nodes=64
stage=path.node ... depth=0 control_type=50004 root_match=false class_name="ExampleClass" class_name_truncated=false class_name_invalid_utf16=false automation_id="" automation_id_truncated=false automation_id_invalid_utf16=false framework_id="ExampleFramework" framework_id_truncated=false framework_id_invalid_utf16=false control_view=true content_view_property=true content_view=Some(true)
...
stage=path.node ... depth=34 ... root_match=true ...
stage=path.summary ... path_complete=true nodes=35 metadata_complete=true other_branches_read=false chat_input_identified=false
stage=end ... result=OBSERVED stopped=false truncated=false
```

`depth=0` が選んだ入力欄です。深さは観測位置であり、固定の識別条件ではありません。`""` は空の取得値、`unknown` と数値HRESULTは取得失敗です。Control View所属は `control_view`、Content View所属は両プロパティがtrueの場合の `content_view=Some(true)` です。判別不能なら `None` です。

`path_complete=true` は前面ルートまでの経路を観測できたこと、`metadata_complete=true` は指定属性に不明・省略・不正文字置換がなかったことです。`OBSERVED` は読み取り完了であり、固定指定ルールやdots識別の成立ではありません。

64要素上限・前面／操作介入・API失敗・フォーカス変更では `INCOMPLETE`。属性の一部が不明・省略でも、期限内なら他の指定属性は読み、最後に `metadata_unknown_or_truncated` とします。総期限1500ms、UIA接続／トランザクション200ms、出力最大320行＋最終行を維持します。OSの応答時間の厳密な保証やツリー全体の不可分なスナップショットではありません。

## この観測の目的

入力欄そのもののClassName、または祖先のID／ClassNameと型の組合せが固定目印に使えるか、一度の記録で判断します。FrameworkIdは基盤の補助情報です。汎用クラスや動的IDしかなければ用途識別には不足します。dots画面を区別する根拠と入力欄を区別する根拠は別であり、このログだけで他画面への誤一致がないとは判断しません。

## 開発・配布

既存の公式windowsバインディングを利用し、新規依存はありません。

```sh
cargo fmt --check
cargo test --locked --features focus-probe
cargo check --locked --features focus-probe --target x86_64-pc-windows-msvc
cargo build --locked --release --features focus-probe --bin iris-path-probe --target x86_64-pc-windows-msvc
```

CIの `iris-path-probe-windows-x64` にexe・この説明・依存ライセンス・SHA-256を同梱します。CRTは静的リンク、Windows標準DLLは使います。CIはexeを起動せず、ビルド成功は実機検証を意味しません。通常版・既存診断の動作・main・正式リリースは変更しません。
