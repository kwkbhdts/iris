# Iris の花アイコン

承認済みの紫のアイリス。`iris-app.svg` は exe 用、`iris-tray.svg` は
黄色の細部を中央だけに整理した通知領域用。どちらも透明背景です。

ICO は 16 / 20 / 24 / 32 / 48 / 64 / 128 / 256 px の32bit透過PNGを収録。
SVG が編集用原本で、ICO と `iris.res` は生成物としてリポジトリに含めます。
通常ビルドに画像ツール・追加クレート・リソースコンパイラは不要です。
`build.rs` が通常版 `iris` の MSVC リンク時だけ `iris.res` を追加します。
グループID 101 が exe 用、102 がトレイ用。VERSIONINFO は含めません。

## 再生成

素材を変更するときだけ Python 3、Pillow、Inkscape を用意します。
リポジトリのルートで実行してください。

```sh
python scripts/generate-icons.py
python scripts/verify-icons.py
```

生成後は SVG・ICO・RES を一緒に更新します。通常版ビルド後の検証:

```sh
python scripts/verify-icons.py target/x86_64-pc-windows-msvc/release/iris.exe
```

検証は埋め込まれた2つのグループと16画像を元 ICO とバイト単位で比較します。
画像ツールのバージョンにより再生成のPNG圧縮結果が変わることがあります。
プレビューでは明暗背景・16/20/24/32pxを確認済みですが、Explorer と通知領域での
テーマ・DPI別の実表示は Windows 実機で確認してください。

RES は [Microsoft の RESOURCEHEADER](https://learn.microsoft.com/en-us/windows/win32/menurc/resourceheader)
と [アイコンリソース形式](https://learn.microsoft.com/en-us/windows/win32/menurc/resource-file-formats)
に従う、言語中立のアイコンのみのファイルです。
