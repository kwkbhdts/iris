use std::{env, path::PathBuf};

/// 通常版だけに、生成済みの花アイコンを静的に埋め込む。
fn main() {
    println!("cargo:rerun-if-changed=assets/icons/iris.res");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    assert_eq!(
        env::var("CARGO_CFG_TARGET_ENV").as_deref(),
        Ok("msvc"),
        "Iris icon resources require the supported Windows MSVC target"
    );
    let resource =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("assets/icons/iris.res");
    println!("cargo:rustc-link-arg-bin=iris={}", resource.display());
}
