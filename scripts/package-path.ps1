$ErrorActionPreference = 'Stop'

& "$PSScriptRoot/verify-version.ps1" -Path "target/x86_64-pc-windows-msvc/release/iris-path-probe.exe"

# 試作を通常版とは別の配布物にまとめる。
New-Item -ItemType Directory -Force dist-path | Out-Null
Copy-Item target/x86_64-pc-windows-msvc/release/iris-path-probe.exe dist-path/
Copy-Item PATH_PROBE.md dist-path/
$sysroot = (& rustc --print sysroot).Trim()
if ($LASTEXITCODE -ne 0) { throw 'rustc sysroot lookup failed' }
Copy-Item "$sysroot/share/doc/rust/COPYRIGHT-library.html" dist-path/RUST-LIBRARY.html

# 新規依存を含め、ロック済みソースからライセンス原文を同梱する。
$metadata = & cargo metadata --locked --features focus-probe --format-version 1
if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
$notices = [System.Collections.Generic.List[string]]::new()
$notices.Add('Iris read-only ancestor path prototype - third-party notices; Iris project license is unchanged.')
foreach ($package in (($metadata | ConvertFrom-Json).packages | Where-Object { $_.name -ne 'iris' } | Sort-Object name)) {
    $notices.Add("`n=== $($package.name) $($package.version) ($($package.license)) ===")
    $directory = Split-Path $package.manifest_path
    $licenses = @(Get-ChildItem $directory -File | Where-Object { $_.Name -match '^(LICENSE|COPYING)' } | Sort-Object Name)
    if ($licenses.Count -eq 0) { throw "Missing license source: $($package.name)" }
    foreach ($license in $licenses) {
        $notices.Add("--- $($license.Name) ---")
        $notices.Add((Get-Content $license.FullName -Raw))
    }
}
$notices | Set-Content dist-path/THIRD_PARTY_NOTICES.txt -Encoding utf8NoBOM

# 実行ファイル・手順・ライセンス原文のハッシュを添える。
$lines = foreach ($name in @('iris-path-probe.exe', 'PATH_PROBE.md', 'THIRD_PARTY_NOTICES.txt', 'RUST-LIBRARY.html')) {
    $hash = (Get-FileHash "dist-path/$name" -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $name"
}
$lines | Set-Content dist-path/SHA256SUMS.txt -Encoding utf8NoBOM
