# 一键出两个发布物：
#   dist\NiuMa-Workstation-<ver>-setup.exe      安装版（Inno Setup）
#   dist\NiuMa-Workstation-<ver>-portable.zip   绿色版（解压即用，数据存 exe 旁的 data\）
# 若存在安装版，还会生成 dist\latest.json：上传到 GitHub Release 后供应用内“检查更新”使用。
param(
    [switch]$SkipBuild,
    [string]$Notes = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$match = Select-String -Path "crates\app\Cargo.toml" -Pattern '^version\s*=\s*"(.+)"' | Select-Object -First 1
if (-not $match) { throw "读不到 crates\app\Cargo.toml 里的版本号" }
$version = $match.Matches[0].Groups[1].Value

$dist = Join-Path $root "dist"
$portableDir = Join-Path $dist "NiuMa-Workstation-$version-portable"
$portableZip = Join-Path $dist "NiuMa-Workstation-$version-portable.zip"
$setupExe = Join-Path $dist "NiuMa-Workstation-$version-setup.exe"

if (-not $SkipBuild) {
    # 静态链接 CRT：发布物不依赖 VC++ 运行库，干净机器可直接运行
    $env:RUSTFLAGS = "-C target-feature=+crt-static"
    cargo build -p niu_ma_workstation --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build --release 失败" }
}

$exe = "target\release\NiuMa-Workstation.exe"
if (-not (Test-Path $exe)) { throw "缺少 $exe，请先去掉 -SkipBuild" }

New-Item -ItemType Directory -Path $dist -Force | Out-Null

# 绿色版：exe + portable.flag（标记存在时数据写到 exe 旁的 data\）
if (Test-Path $portableDir) { Remove-Item $portableDir -Recurse -Force }
New-Item -ItemType Directory -Path $portableDir -Force | Out-Null
Copy-Item $exe $portableDir
New-Item -ItemType File -Path (Join-Path $portableDir "portable.flag") -Force | Out-Null
if (Test-Path $portableZip) { Remove-Item $portableZip -Force }
Compress-Archive -Path (Join-Path $portableDir "*") -DestinationPath $portableZip

# 安装版：优先找用户目录下（winget 默认）的 Inno Setup
$iscc = @(
    (Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe"),
    "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
    "C:\Program Files\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1

if ($iscc) {
    & $iscc "/DAppVersion=$version" "packaging\NiuMa-Workstation.iss"
    if ($LASTEXITCODE -ne 0) { throw "ISCC 打包安装版失败" }
} else {
    Write-Warning "未找到 Inno Setup（ISCC.exe），已跳过安装版，仅生成绿色版"
}

# 更新清单：安装版存在时生成，随 GitHub Release 一起上传
if (Test-Path $setupExe) {
    $hash = (Get-FileHash $setupExe -Algorithm SHA256).Hash.ToLower()
    $size = (Get-Item $setupExe).Length
    $url = "https://github.com/tq57/NiuMa-Workstation/releases/latest/download/NiuMa-Workstation-$version-setup.exe"
    $manifest = [ordered]@{
        version   = $version
        notes     = $Notes
        installer = [ordered]@{
            url    = $url
            sha256 = $hash
            size   = $size
        }
    }
    $manifest | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $dist "latest.json") -Encoding UTF8
}

Get-ChildItem $dist -Filter "NiuMa-Workstation-$version-*" |
    Select-Object Name, @{ n = "MB"; e = { [math]::Round($_.Length / 1MB, 1) } }
