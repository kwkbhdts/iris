param([Parameter(Mandatory = $true)][string]$Path)
$ErrorActionPreference = 'Stop'

# 独立パーサーに加え、Windows 自身が読む VERSIONINFO も照合する。
python "$PSScriptRoot/verify-version.py" $Path
if ($LASTEXITCODE -ne 0) { throw 'VERSIONINFO resource verification failed' }
$info = (Get-Item -LiteralPath $Path).VersionInfo
$product = (Select-String -Path "$PSScriptRoot/../Cargo.toml" -Pattern '^version = "([^"]+)"').Matches[0].Groups[1].Value
$core = ($product -split '[-+]')[0]
$number = if ($env:GITHUB_ACTIONS -eq 'true') { $env:GITHUB_RUN_NUMBER } else { '0' }
if ($info.ProductVersion -ne $product -or $info.FileVersion -ne "$core.$number") {
    throw 'Windows VersionInfo strings do not match the build inputs'
}
if ($info.OriginalFilename -ne (Split-Path $Path -Leaf) -or [string]::IsNullOrEmpty($info.Comments)) {
    throw 'Windows VersionInfo identity is missing'
}
$origin = if ($env:GITHUB_ACTIONS -eq 'true') { 'github' } else { 'local' }
$run = if ($origin -eq 'github') { $env:GITHUB_RUN_ID } else { 'none' }
$attempt = if ($origin -eq 'github') { $env:GITHUB_RUN_ATTEMPT } else { 'none' }
$commit = (& git rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot verify source commit' }
$status = & git status --porcelain --untracked-files=normal
if ($LASTEXITCODE -ne 0) { throw 'Cannot verify source tree' }
$tree = if ($status) { 'dirty' } else { 'clean' }
$expected = "product=$product file=$core.$number origin=$origin run=$run attempt=$attempt commit=$commit tree=$tree"
$numericFile = @($info.FileMajorPart, $info.FileMinorPart, $info.FileBuildPart, $info.FilePrivatePart) -join '.'
$numericProduct = @($info.ProductMajorPart, $info.ProductMinorPart, $info.ProductBuildPart, $info.ProductPrivatePart) -join '.'
if ($info.Comments -cne $expected -or $numericFile -ne "$core.$number" -or $numericProduct -ne "$core.0") {
    throw 'Windows VersionInfo numeric version or CI identity does not match'
}
if ($info.ProductName -ne 'Iris' -or $info.InternalName -ne [IO.Path]::GetFileNameWithoutExtension($Path)) {
    throw 'Windows VersionInfo application kind does not match'
}
Write-Output "PASS Windows VersionInfo: $($info.OriginalFilename) $($info.FileVersion) $($info.Comments)"
