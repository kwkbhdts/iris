# Iris

Windows 向けの小さな通知領域ツール。通常権限で動かし、修飾キーなしの **F1** を手動で押すと、前面が `Claude.exe` または `firefox.exe` の場合だけ Unicode の `OK` と Enter を送ります。AutoHotkey は不要です。

現在は開発中です。この 100ms 間隔の比較版は、Windows 実機・日本語 IME での動作をまだ確認していません。

## ブランチ方針

- `main`: 確認済みの区切りを置くブランチ。初回は案内のみです。
- `dev`: 実装・テスト・CI を進める開発ブランチです。

Windows 実機で確認できた区切りで、開発コードの `main` への取り込みを検討します。

## 使い方

1. ビルドした `iris.exe` を一つだけ起動します。初期状態は有効です。
2. 対象アプリで、**空の入力欄に自分でフォーカス**します。IME の未確定文字がないことを確認してください。
3. Shift / Ctrl / Alt / Windows キーを離し、F1 を押します。Unicode の `OK` を入力し、タイマーで 100ms 待ってから Enter を一度だけ送ります。長押しや待機中の連打で追加送信はしません。
4. 通知領域のアイコンをクリックすると、「有効」のチェックを切り替えるか「終了」を選べます。状態はアイコンのツールチップでも確認できます。

対象外、無効時、修飾キー付きでは通常の F1 を通します。押し始めに決めた抑止・通過をキーアップまで維持するため、押したまま前面や有効状態を変えても二重動作しません。終了時にフックと通知領域アイコンを解除します。

## 制約

- **空欄・フォーカス・IME 未確定文字なしは使用条件です。アプリは検査しません。** 日本語 IME オン／半角の両方を意図して `KEYEVENTF_UNICODE` を使用していますが、実機では未検証です。IME の状態変更や確定操作は行いません。
- 100ms の固定待機は、文字入力直後の Enter で送信されない現象の原因切り分け用です。Web アプリの更新完了を検知するものではなく、送信成功を保証しません。文字と Enter の間隔以外に、スキャンコード方式や IME 操作は変更していません。
- Firefox は実行ファイル名だけで判定します。すべてのサイト・入力欄・アドレスバーが対象になり得ます。URL 判定、自動フォーカス、通知検知、承認の自動化はありません。
- F1 の抑止は `WH_KEYBOARD_LL` で行います。プロセス名はフック外で確認し、前面変更イベントと 200 ms のタイマーで更新します。前面変更直後など対象が未確認の間は通常の F1 を通します。必要なら前面が落ち着いてから押し直してください。
- 文字と Enter のそれぞれを送る直前に、有効状態、元の前面ウィンドウ・PID・スレッド、実行ファイル名、修飾キー、F1 からの経過時間を再確認します。前面変更、待機中の修飾キー押下、無効化・終了、F1 から 250ms を超えた遅延では予約を破棄します。取消時に `OK` だけが残る場合があります。**SendInput は特定ウィンドウ宛てではないため、確認直後の切替や同一ウィンドウ内の入力フォーカス移動による誤送信を完全には防げません。**
- 他アプリが注入した F1 を含む注入イベントは通過させ、Iris のトリガーとして再処理しません。
- 管理者として動く対象への入力は UIPI により失敗する可能性があります。権限昇格や制限回避は実装しません。送信失敗・一部送信時は無効化し、エラーを表示します。自動再送はしません。入力内容とキー状態を確認してください。
- 実行ファイル名はセキュリティ上の本人確認にはなりません。フックが OS により解除された場合も自動復旧は保証しません。
- 自動起動、設定ファイル、汎用マクロ、通信機能はありません。多重起動の防止は未実装なので一つだけ起動してください。

## ビルドと検証

第一候補は Windows x64 と `x86_64-pc-windows-msvc`。ビルドには Rust と MSVC C++ ビルドツール／Windows SDK が必要です。

```sh
cargo fmt --check
cargo test --locked
cargo check --locked --target x86_64-pc-windows-msvc
cargo build --locked --release --target x86_64-pc-windows-msvc
```

出力は `target/x86_64-pc-windows-msvc/release/iris.exe`。`.cargo/config.toml` で CRT 静的リンクを指定し、追加ランタイム不要の単一 exe を目指します。Windows 標準 DLL は利用します。実際の DLL 依存関係は Windows ビルド後に確認が必要です。

Linux では `cargo test` が F1 の状態遷移と実行ファイル名判定を検証します。Windows 標準ライブラリを備えた Rust なら `cargo check --target x86_64-pc-windows-msvc` で Windows コードの型チェックも可能ですが、exe のリンクには別途 Windows 用のツール／ライブラリが必要です。

## CI と検証範囲

[GitHub Actions の Build](https://github.com/kwkbhdts/iris/actions/workflows/build.yml) は `main` / `dev` への push と、それらに向けた pull request で動きます。

- Linux ジョブ: `cargo fmt --check` と14件のロジックテストを実行します。100ms の待機、二重送信防止、取消、古いタイマー通知、期限切れも対象です。
- Windows ジョブ: `x86_64-pc-windows-msvc` の release exe をビルドします。CRT 静的リンクは `.cargo/config.toml` の指定を使用します。
- exe は各 run の **Artifacts → iris-windows-x64** から取得できます（保存期間14日）。開発用成果物であり、リリース版ではありません。

各コミットの CI 成否は Actions で確認してください。依存バージョンは `Cargo.lock` に固定しています。Linux 上での整形・ロジックテスト・Windows 向け型チェックは確認済みですが、型チェックには exe のリンクや実行は含まれません。

**Windows でビルドできても、実機の通知領域・フック・入力送信・日本語 IME の動作確認にはなりません。これらは未検証です。** CI は Iris の常駐開始や実際のチャット送信を行いません。

実機検証は、実際にチャットを送信しない Firefox のローカル HTML 入力欄などから始めてください。IME オン／半角（未確定文字なし）、長押し、連打、各修飾キー、対象外アプリ、前面切替、有効切替、終了後の F1、通常権限から高い権限の対象への失敗を確認します。Claude 固有の入力動作は別途、送信を許可した環境での確認が必要です。

この比較版では特に、待機中の前面切替・修飾キー押下・無効化・終了で Enter が取り消されること、取消後に対象へ戻っても古い Enter が届かないことを確認してください。タイマーの実際の発火時刻やブラウザのイベント処理は、Linux のロジックテストでは検証していません。

## API の根拠

- [LowLevelKeyboardProc](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelkeyboardproc): フックの抑止、タイムアウト、注入イベント。
- [SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput): 入力の挿入数、UIPI、現在のキー状態。
- [KEYBDINPUT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-keybdinput): Unicode 入力。
- [Microsoft windows-rs](https://github.com/microsoft/windows-rs): 使用する `windows-sys` バインディング。
