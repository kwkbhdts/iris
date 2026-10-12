$ErrorActionPreference = 'Stop'

& "$PSScriptRoot/verify-version.ps1" -Path "target/x86_64-pc-windows-msvc/release/iris-auto-send-probe.exe"

# 試作を通常版とは別の配布物にまとめる。
New-Item -ItemType Directory -Force dist-auto-send | Out-Null
Copy-Item target/x86_64-pc-windows-msvc/release/iris-auto-send-probe.exe dist-auto-send/
Copy-Item AUTO_SEND_PROBE.md dist-auto-send/
$sysroot = (& rustc --print sysroot).Trim()
if ($LASTEXITCODE -ne 0) { throw 'rustc sysroot lookup failed' }
Copy-Item "$sysroot/share/doc/rust/COPYRIGHT-library.html" dist-auto-send/RUST-LIBRARY.html

# 新規依存を含め、ロック済みソースからライセンス原文を同梱する。
$metadata = & cargo metadata --locked --features focus-probe --format-version 1
if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
$notices = [System.Collections.Generic.List[string]]::new()
$notices.Add('Iris one-press auto-send prototype - third-party notices; Iris project license is unchanged.')
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
$notices | Set-Content dist-auto-send/THIRD_PARTY_NOTICES.txt -Encoding utf8NoBOM

# 実行ファイル・手順・ライセンス原文のハッシュを添える。
$lines = foreach ($name in @('iris-auto-send-probe.exe', 'AUTO_SEND_PROBE.md', 'THIRD_PARTY_NOTICES.txt', 'RUST-LIBRARY.html')) {
    $hash = (Get-FileHash "dist-auto-send/$name" -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $name"
}
$lines | Set-Content dist-auto-send/SHA256SUMS.txt -Encoding utf8NoBOM
