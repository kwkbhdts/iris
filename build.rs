mod build_support;

use std::{env, fs, path::PathBuf, process::Command};

/// Git の固定メタデータだけを取得し、パスや作者情報は埋め込まない。
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// 環境とソースから版を生成し、各 exe に VERSIONINFO を埋め込む。
fn main() {
    for path in [
        "build.rs",
        "build_support.rs",
        "Cargo.toml",
        "Cargo.lock",
        "src",
        "assets",
        "scripts",
        ".github",
        "README.md",
        "AGENTS.md",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    for key in [
        "CI",
        "GITHUB_ACTIONS",
        "GITHUB_RUN_NUMBER",
        "GITHUB_RUN_ID",
        "GITHUB_RUN_ATTEMPT",
        "GITHUB_SHA",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    for name in ["HEAD".to_owned(), "index".to_owned()]
        .into_iter()
        .chain(git(&["symbolic-ref", "-q", "HEAD"]))
    {
        if let Some(path) = git(&["rev-parse", "--git-path", &name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let tree = match git(&["status", "--porcelain", "--untracked-files=normal"]) {
        Some(s) if s.is_empty() => "clean",
        Some(_) => "dirty",
        None => "unknown",
    };
    let github = env::var("GITHUB_ACTIONS").as_deref() == Ok("true");
    assert!(
        github || env::var("CI").map_or(true, |s| s != "true"),
        "Only GitHub CI identity is supported; do not label unknown CI builds as local"
    );
    let number = env::var("GITHUB_RUN_NUMBER").unwrap_or_default();
    let run = env::var("GITHUB_RUN_ID").unwrap_or_default();
    let attempt = env::var("GITHUB_RUN_ATTEMPT").unwrap_or_default();
    if github {
        assert_eq!(
            env::var("GITHUB_SHA").as_deref(),
            Ok(commit.as_str()),
            "CI checkout must match GITHUB_SHA"
        );
    }
    let info = build_support::identity(
        &env::var("CARGO_PKG_VERSION").unwrap(),
        github.then_some((number.as_str(), run.as_str(), attempt.as_str())),
        &commit,
        tree,
    )
    .expect("Invalid Iris build identity");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(
        out.join("build_info.rs"),
        format!(
            "pub const ID: &str = {:?};\npub const FILE_VERSION: &str = {:?};\n",
            info.label, info.file,
        ),
    )
    .unwrap();

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    assert_eq!(
        env::var("CARGO_CFG_TARGET_ENV").as_deref(),
        Ok("msvc"),
        "Windows MSVC target required"
    );
    let icons = fs::read("assets/icons/iris.res").unwrap();
    for bin in [
        "iris",
        "iris-focus-probe",
        "iris-discovery-probe",
        "iris-path-probe",
        "iris-auto-focus-probe",
        "iris-auto-send-probe",
    ] {
        let mut resource = if bin == "iris" {
            icons.clone()
        } else {
            icons[..32].to_vec()
        };
        resource.extend(build_support::version_resource(&info, bin));
        let path = out.join(format!("{bin}.res"));
        fs::write(&path, resource).unwrap();
        println!("cargo:rustc-link-arg-bin={bin}={}", path.display());
    }
}
