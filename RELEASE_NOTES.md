Iris の初回リリースです。Windows x64 向けの通常版のみを配布します。

- 対象：Claude Desktop、Firefox、Chrome、ChatGPT Desktop。
- フォーカス済みの空欄で、IME の未確定文字がないことを確認して F1 を押すと、OK と Enter を一度送ります。
- OK→Enter の追加待機なし。元が IME オンの場合のみ、一時オフ確認後に入力し、投入後の暫定100msを経て条件付きで復元します。
- 通知領域から有効／無効・終了を操作できます。対象外の F1 は透過します。
- 診断・比較用アプリは配布しません。

`iris.exe` と `THIRD_PARTY_NOTICES.txt`、`RUST-LIBRARY.html` を取得してください。`SHA256SUMS.txt` で整合性を確認できます。AutoHotkey や追加ランタイムのインストールは不要です。

Firefox／Chrome はブラウザ全体が対象です。URL 限定や自動フォーカスはありません。入力先・未確定文字なしは利用者の確認事項です。復元用100msや SendInput の成功は、アプリの処理完了・送信成功を保証しません。介入や状態不明時は手動確認が必要です。

利用者による Claude／Firefox の半角・日本語での送信成功、ChatGPT Desktop／Chrome の送信成功報告があります。後者の IME 条件の網羅確認は未実施です。全環境での動作保証ではありません。詳細・制約は README を参照してください。
