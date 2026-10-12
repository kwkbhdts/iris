#![allow(dead_code)]

include!(concat!(env!("OUT_DIR"), "/build_info.rs"));

/// 固定の製品・ビルド情報だけを表示用に整える。
pub fn display() -> String {
    format!("Iris {}\nファイル版: {}\n{}\n\nローカル版・変更済みソースは配布 CI 版ではありません。\nコピーはトレイの「バージョン情報をコピー」から行えます。", env!("CARGO_PKG_VERSION"), FILE_VERSION, ID)
}
