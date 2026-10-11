# Iris 読み取り専用診断 v2

`iris-discovery-probe.exe` は ChatGPT Desktop (`ChatGPT.exe`) 専用です。**SetFocus・文字入力・Enter送信・IME変更は行いません。** F2登録もありません。利用者が現在選んでいる要素を「観測時のフォーカス」として、UIA上の構造・対応パターンと他のEditとの違いを読み取ります。チャット欄を自動識別できたという試作ではありません。Windows実機は未検証です。

## 今回は検索画面で一回

1. 通常のIrisと他の試作を終了して起動し、`build=discovery-readonly-v2` と `stage=ready` を確認します。
2. ChatGPT Desktopの検索画面で、文字入力部分をクリックしてキャレットがある状態にします。IMEの未確定文字なし・マウス停止を確認し、修飾キーなしの **F1を一回押して離し、約2秒何も操作せず待ちます**。
3. コンソールへ戻り、起動行から最後の `stage=end` までコピーします。「検索画面」、アプリの版と、意図せずフォーカス・内容が変化しなかったかを添えてください。

終了はコンソールでEnterです。長押しは連続実行せず、ChatGPT以外ではF1の通常動作を保ちます。F2は通常動作です。操作介入・前面変更で観測を中止します。`stopped=true` / `restart_required=true` では終了してください。実行中のコンソール選択・コピーは避けてください。

同じ画面での繰り返しは不要です。まず今回の一回で、ListItem参照の祖先と検索画面内のEdit候補数を確認します。会話本文やスクリーンショットの共有は不要です。

## 読む情報と読まない情報

- 参照がEditの場合に限り、そのenabled・keyboard-focusable・offscreen・password、Value/Text/Text2パターンの対応有無。パターンオブジェクトを照会して直ちに解放するだけで、内容や操作メソッドを呼びません。取得失敗は `None` / `unknown` と数値HRESULTです。
- 参照がListItemなど非Editでも、前面ルートまでの祖先ControlTypeと深さ。固定深度を入力欄の識別条件にはしません。
- 前面ルート配下をRaw Viewで辿り、Editごとの一時的なノード番号・親番号・深さ・Document配下か・正解との一致・同じ操作可能条件を記録します。先頭Editで探索を終えません。ルート自身のDocumentは文書所属の根拠にしません。
- AutomationId / ClassNameだけは参照Editと列挙したEditから一時的に取得します。出力は**空でないか・正解と同値か**だけで、元の文字列・ハッシュ・長さは出しません。要求をまたいで保持せずファイル保存もしません。未確認の固定値を組み込んで識別できたとは扱いません。
- **Name・Value・Textの内容・タイトル・URL・ARIA自由文・会話本文は取得しません。** ブラウザーは対象外です。PID・HWND・実行パスの数値や文字列も出力しません。

## 結果の読み方

例は形式説明で、候補数などは実測値ではありません。

```text
stage=reference ... control_type=50007 expected_edit=false
stage=reference ... reference_role=observed_focus reference_is_edit=false reference_identity_applicable=false chat_input_identified=false
stage=reference ... reference_details=skipped_non_edit
stage=reference.ancestors ... depth=0 control_type=50007 root_match=false
stage=edit ... control_type=50004 same_reference=false
stage=survey ... complete=true reason=exhausted ... candidates=1 reference_matches=0 ... reference_identity_applicable=false unique_candidate=false chat_input_identified=false
stage=end ... result=OBSERVED stopped=false truncated=false
```

50007はListItemです。v1では検索欄にキャレットがある状態でもこの型が返り、探索前に停止しました。v2では非Edit参照の編集属性・識別文字列は取得せず、祖先が前面ルートへ到達したことを確認してEdit列挙を実行します。**ListItemを候補に加える変更ではありません。** 参照はチャット欄の正解とは認定せず、候補の用途は常に未確認です。

`candidates` はDocument配下の操作可能な非password・非offscreen Editの数です。`reference_matches` はそのうち観測時の参照とUIA上で同じ要素の数、`id_matches` / `class_matches` は非空の識別属性が正解と等しい候補の数です。空文字列同士は一致根拠に数えません。`identity_unknown` があれば属性による区別は未確定です。非Edit参照では `reference_identity_applicable=false`、識別属性の比較は `None` となり、候補の `identity_unknown` に数えます。これは属性取得エラーではなく比較対象外を含みます。候補自身の属性有無は引き続き記録します。

`unique_candidate=true` は**探索完了・参照がEdit・候補1・参照一致1**の場合だけです。参照が非Editなら常にfalseです。参照がEditの検索欄なら成立し得るので、チャット欄と識別できたという意味ではありません。`OBSERVED` は読み取り完了であり、フォーカス成功ではありません。終了時には実フォーカスが最初の参照のままかも照合します。UIAツリーは不可分なスナップショットではないため、アプリ内部の変化を完全に検出する保証はありません。

最大2048ノード、深さ0〜63、属性調査するEditは24件、全体1500ms、各UIA接続・トランザクション200msです。祖先調査も同じ全体期限に含めます。上限・API失敗・介入では `complete=false / result=INCOMPLETE / unique_candidate=false`。件数は取得済みの部分だけで、未取得部分を0件とはしません。`node_limit` / `depth_limit` / `edit_limit` / `deadline_exceeded` などで区別します。出力も最大320行＋最終行に制限します。OSやプロバイダー応答の厳密な時間保証ではなく、watchdog時には無効化して再起動を求めます。

## この一回から判断すること

通常画面はv1で1837ノードを889msで完全探索し、候補1件と指定したEditが一致しました。検索画面では今回、独立したEditがあるか、その所属や候補数がどう異なるかを確認します。列挙したEditが検索入力欄なのか、背後のチャット欄なのかは候補数だけでは確定できません。非Edit参照と候補の識別属性比較は行わないため、必要な追加観測は結果を見て限定します。AutomationIdが非空で正解だけを区別できても、埋め込みルールに使う実際の値はこのログにはありません。その場合の次に必要な観測は、**通常チャット欄として手動指定した一要素のAutomationIdに限定**できます。IDがなくClassNameだけで区別できる場合は、その一要素のClassNameです。両方空または他用途と共通なら、この診断だけで識別ルールは作れません。自由文や全ツリーの文字列収集へ自動拡大せず、必要な属性と対象を先に報告します。

既存v4の「Firefoxは遅延したフォーカス一致」「Desktop/ChromeはSetFocus中の前面通知で事後観測前に失効」という結果は保持します。この読み取り診断にはSetFocus自体がないため、前面通知の例外・世代リセット・成功への読み替えはありません。

## 開発と配布

`cargo fmt --check`、`cargo test --locked --features focus-probe`、`cargo check --locked --features focus-probe --target x86_64-pc-windows-msvc` を使用します。独立バイナリは `cargo build --locked --release --features focus-probe --bin iris-discovery-probe --target x86_64-pc-windows-msvc` です。既存の公式windowsバインディングを使い、新規依存はありません。

CIの `iris-discovery-probe-windows-x64` にexe・この説明・依存ライセンス・SHA-256を同梱します。MSVCのCRTは静的リンク、Windows標準DLLは使用します。CIはexeを起動せず、ビルド成功はWindows実機確認を意味しません。通常版の送信・IMEコード、main、正式リリースは変更しません。
