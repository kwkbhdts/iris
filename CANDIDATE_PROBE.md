# Iris 共通Edit候補数・祖先診断 v2

`iris-candidate-probe.exe` は4アプリ共通の簡単な候補条件を観測する**読み取り専用**試作です。対象は前面 `ChatGPT.exe`、`Claude.exe`、`firefox.exe`、`chrome.exe`。SetFocus・文字入力・Enter送信・IME書込みはありません。既存の通常版・オート送信版の動作や許可対象は変更しません。

## 試す手順

1. 他のIrisを終了し、ZIPを展開して `iris-candidate-probe.exe` を通常権限で起動。`build=candidate-count-readonly-v2` と `stage=ready` を確認します。起動引数やF2登録は不要です。
2. 対象アプリの比較したい画面を開きます。入力欄へのフォーカスは不要です。約2秒待ってマウスを止め、修飾キーなしのF1を一回押して離し、約2秒無操作で待ちます。
3. 処理終了後にコンソールへ戻り、起動行とその要求の `stage=end` までを共有します。「通常会話／検索／ダイアログ」「アドレスバーのみ／ページ入力欄あり」等のラベルとアプリ版だけ添えてください。本文・URL・会話名・スクリーンショットは不要です。
4. 終了はコンソールでEnter。`stopped=true` なら採取を続けず終了します。

4アプリで各一回、ブラウザーはアドレスバーとページ入力欄が両方ある状態も比較してください。検索・ダイアログを開いた状態でも採取できます。マウス移動・縦横ホイールも既存の読み取り診断ガードでは中断要因です。採取中のクリック・キー・前面変更・コピー操作は避けてください。長押しは連続採取せず、自動再試行もありません。対象外のF1、修飾キー付きF1、F2は通常動作を維持します。

## 数える条件

前面HWNDが最上位ルートであることを確認し、そのUIA子孫について、以下の5条件だけをANDで検索します。

- ControlType = Edit
- IsOffscreen = false
- IsEnabled = true
- IsKeyboardFocusable = true
- IsPassword = false

**IsControlElement、Class、Document、祖先構造、初期フォーカスは絞り込み条件にしません。** アドレスバー・検索欄・別サイトも特別に除外しません。2件以上から都合のよい候補を一つ選ぶことはありません。候補要素のName・Value・Text・URL・本文・ClassName・AutomationIdは取得しません。候補の個別内容や位置もログへ出しません。

検索前後に、前面ルート自身と全子孫の `IsOffscreen=false AND (IsDialog=true OR WindowIsModal=true)` を調べます。どちらかで1件以上見えたら、後で消えてもその要求は対象不可です。対象可否のための削除・フォーカス移動・ダイアログを閉じる操作は行いません。

## 結果の読み方

- `complete=true candidate_state=zero`：候補0件、対象不可。
- `complete=true candidate_state=one`：候補は厳密に1件。公開ダイアログ/modalが0件のときだけ `target_eligible=true`。
- `complete=true candidate_state=multiple`：2件以上、対象不可。
- `complete=false candidate_state=incomplete`：取得失敗・介入・期限・ルート変化等で未完了、対象不可。`candidate_count=Some(...)` があっても途中件数です。`None` は未取得で、0件ではありません。

`visible_dialog_or_modal_max` は検索前後で観測した件数の最大値で、画面をまたぐ累積数ではありません。`result=OBSERVED` は診断完了であり、対象可を意味しません。`target_eligible=true` も新しい5条件による診断上の可否であり、送信許可・会話欄識別・空欄確認ではありません。アドレスバーや検索欄だけが候補なら対象可になり得ます。このexeは結果にかかわらず送信しません。

## 候補ごとの祖先診断

件数判定の後、同じ検索結果の候補を最大24件取得し、Raw Viewの親方向をたどります。各経路は候補自身とルートを含め最大64要素、全体の1500ms期限内です。候補自身と前面ルート自身はDocument祖先に数えません。内容・名前・Class等は読まず、祖先のControlTypeとルートとの一致だけを確認します。

- `candidate_index`：今回の検索結果内の0始まりの番号。要求間の同一性は保証しません。
- `root_reached=true ancestry_complete=true has_document_ancestor=true/false`：前面ルートへの到達を確認し、その内側にDocument祖先があった／なかった。
- `ancestry_complete=false has_document_ancestor=unknown`：親なし・API失敗・上限・介入等で未完了。途中でDocumentを見ても有無は確定しません。
- `details_complete`／`details_reason`：追加診断全体の完了状態。24件を超えた残りは `candidate_details_omitted` で報告します。開始前に失敗した候補には個別行がない場合もあります。

`complete`、`candidate_count`、`target_eligible` は追加診断開始前の件数判定を保持します。Documentなしやunknownの候補を除外しません。追加診断中の介入・期限・最後のルート再確認失敗等では、件数判定がcomplete=trueでもdetails_complete=false、最終result=INCOMPLETEになります。先に出た個別行はその時点の観測であり、後続の失敗を打ち消しません。Documentありは会話欄の証明ではなく、Documentなしはアドレスバーの証明でもありません。5属性の追加再取得やGetFocusedElementによる比較も行いません。

## 観測の限界

UIAに公開されないEditは数えられず、属性はプロバイダーの申告に依存します。`IsOffscreen=false` は覆われていない保証ではありません。ダイアログ属性が未対応・未公開の検索パネルは検出できず、0件からオーバーレイ不存在は証明できません（`unreported_overlay_state=unknown`）。検索結果は画面全体の不可分なスナップショットではなく、観測後も変化します。

件数はUIAの完了したFindAllとLengthの結果であり、候補が多いから除外して減らす処理はありません。FindAll内部の訪問数・割当量は厳密には制限できません。全体1500ms、UIA接続／トランザクション200ms、前面・介入・終了時の同一ルート確認を維持しますが、API呼出し中の厳密な取消は保証しません。通信・自動保存・アップロード・自動起動登録はありません。

## 開発とCI成果物

ローカル確認は `cargo fmt --check`、`cargo test --locked --features focus-probe`、`cargo check --locked --features focus-probe --all-targets --target x86_64-pc-windows-msvc`。Windows実機動作は未検証です。

devへのpushで既存Build CIが `iris-candidate-probe-windows-x64` を作成します。exe、本手順書、THIRD_PARTY_NOTICES.txt、RUST-LIBRARY.html、SHA256SUMS.txtの5ファイルです。Windows版情報とアップロード直前のファイル構成・ハッシュを検証します。CIはexeを実行しません。CI成果物には保持期限があります。起動時の `build=candidate-count-readonly-v2` とコミット・版番号で祖先診断版を確認してください。
