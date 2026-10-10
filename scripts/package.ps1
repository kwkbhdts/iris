$ErrorActionPreference = 'Stop'

# ビルドに使用した Rust のライセンス原文を配布物に含める。
$sysroot = (& rustc --print sysroot).Trim()
if ($LASTEXITCODE -ne 0) { throw 'rustc sysroot lookup failed' }
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target/x86_64-pc-windows-msvc/release/iris.exe dist/iris.exe
Copy-Item THIRD_PARTY_NOTICES.txt dist/THIRD_PARTY_NOTICES.txt
Copy-Item "$sysroot/share/doc/rust/COPYRIGHT-library.html" dist/RUST-LIBRARY.html

# 添付するファイルの SHA-256 を固定した順序で記録する。
$lines = foreach ($name in @('iris.exe', 'THIRD_PARTY_NOTICES.txt', 'RUST-LIBRARY.html')) {
    $hash = (Get-FileHash "dist/$name" -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $name"
}
$lines | Set-Content dist/SHA256SUMS.txt -Encoding utf8NoBOM
