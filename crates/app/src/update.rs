//! 检查更新与安装包下载：只做安装版的半自动升级。
//!
//! 清单地址固定为 GitHub Release 上的 `latest.json`；绿色版只提示手动下载，
//! 安装版由用户点击「安装并重启」后退出应用并静默运行安装包。

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// 当前应用版本，与 Cargo.toml 保持一致。
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

const MANIFEST_URL: &str =
    "https://github.com/tq57/NiuMa-Workstation/releases/latest/download/latest.json";

/// 绿色版手动下载时打开的发布页。
pub const RELEASES_URL: &str = "https://github.com/tq57/NiuMa-Workstation/releases/latest";

const PORTABLE_FLAG: &str = "portable.flag";

/// GitHub Release 中附带的更新清单。
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub installer: UpdateAsset,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateAsset {
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
}

/// 已确认可用的新版本。
#[derive(Debug, Clone)]
pub struct AvailableUpdate {
    pub version: String,
    pub notes: String,
    pub installer: UpdateAsset,
}

/// 更新流程的界面状态。
#[derive(Debug, Clone)]
pub enum UpdateStatus {
    Checking,
    UpToDate,
    Available(AvailableUpdate),
    Downloading { version: String },
    Ready { version: String, installer: PathBuf },
    Failed(String),
}

/// 拉取远端清单并判断是否有新版本。网络失败会返回错误，由调用方决定是否提示。
pub fn check_latest() -> Result<Option<AvailableUpdate>> {
    let manifest = fetch_manifest()?;
    evaluate(CURRENT_VERSION, &manifest)
}

fn fetch_manifest() -> Result<UpdateManifest> {
    let response = agent()
        .get(MANIFEST_URL)
        .call()
        .context("请求更新清单失败")?;
    serde_json::from_reader(response.into_reader()).context("解析更新清单失败")
}

fn evaluate(current: &str, manifest: &UpdateManifest) -> Result<Option<AvailableUpdate>> {
    let current = parse_version(current)?;
    let latest = parse_version(&manifest.version)?;
    if latest <= current {
        return Ok(None);
    }
    Ok(Some(AvailableUpdate {
        version: latest.to_string(),
        notes: manifest.notes.clone(),
        installer: manifest.installer.clone(),
    }))
}

fn parse_version(text: &str) -> Result<Version> {
    Version::parse(text.trim().trim_start_matches('v'))
        .with_context(|| format!("无法识别版本号：{text}"))
}

/// 下载并校验安装包，成功返回本地路径。目标文件已存在且哈希一致时直接复用。
pub fn download_installer(update: &AvailableUpdate) -> Result<PathBuf> {
    let target = installer_path(&update.version);
    if target.is_file() && sha256_file(&target)? == update.installer.sha256.to_lowercase() {
        return Ok(target);
    }

    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("创建更新目录失败: {}", dir.display()))?;
    }
    let part = target.with_extension("exe.part");
    let _ = std::fs::remove_file(&part);
    let response = agent()
        .get(&update.installer.url)
        .call()
        .context("下载安装包失败")?;
    let mut reader = response.into_reader();
    let mut file = File::create(&part).with_context(|| format!("创建文件失败: {}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut written = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).context("读取安装包失败")?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).context("写入安装包失败")?;
        hasher.update(&buffer[..read]);
        written += read as u64;
    }
    file.flush().context("写入安装包失败")?;
    drop(file);

    if update.installer.size > 0 && written != update.installer.size {
        let _ = std::fs::remove_file(&part);
        bail!(
            "安装包大小不一致：期望 {} 字节，实际 {} 字节",
            update.installer.size,
            written
        );
    }
    let actual = hex(&hasher.finalize());
    if !actual.eq_ignore_ascii_case(&update.installer.sha256) {
        let _ = std::fs::remove_file(&part);
        bail!("安装包校验失败：SHA-256 不匹配");
    }

    let _ = std::fs::remove_file(&target);
    std::fs::rename(&part, &target).context("保存安装包失败")?;
    Ok(target)
}

/// 安装包缓存位置：`%LOCALAPPDATA%\NiuMa-Workstation\updates`。
pub fn installer_path(version: &str) -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("NiuMa-Workstation")
        .join("updates")
        .join(format!("NiuMa-Workstation-{version}-setup.exe"))
}

/// 当前是否绿色版：exe 同目录存在 portable.flag。
pub fn is_portable() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .map(|dir| dir.join(PORTABLE_FLAG).is_file())
        .unwrap_or(false)
}

/// 启动安装包。用户确认后才调用，随后应用自行退出。
pub fn launch_installer(path: &Path) -> Result<()> {
    let mut command = Command::new(path);
    command.args([
        "/SILENT",
        "/SUPPRESSMSGBOXES",
        "/NORESTART",
        "/CLOSEAPPLICATIONS",
        "/AUTOUPDATE=1",
    ]);
    if let Some(dir) = path.parent() {
        command.current_dir(dir);
    }
    command.spawn().context("启动安装包失败")?;
    Ok(())
}

/// 用默认浏览器打开发布页（绿色版手动更新）。
pub fn open_releases_page() -> Result<()> {
    let operation: Vec<u16> = "open\0".encode_utf16().collect();
    let url: Vec<u16> = RELEASES_URL.encode_utf16().chain(Some(0)).collect();
    let result = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            url.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        )
    };
    if result as isize <= 32 {
        bail!("打开浏览器失败");
    }
    Ok(())
}

fn agent() -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .user_agent(concat!("NiuMa-Workstation/", env!("CARGO_PKG_VERSION")));
    if let Some(proxy) = system_proxy() {
        if let Ok(proxy) = ureq::Proxy::new(&proxy) {
            builder = builder.proxy(proxy);
        }
    }
    builder.build()
}

/// 优先读环境变量，其次读 Windows 系统代理（Clash / 公司代理等）。
fn system_proxy() -> Option<String> {
    for key in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                return Some(normalize_proxy(value.trim()));
            }
        }
    }

    let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings")
        .ok()?;
    let enabled: u32 = key.get_value("ProxyEnable").unwrap_or(0);
    if enabled == 0 {
        return None;
    }
    let server: String = key.get_value("ProxyServer").ok()?;
    let server = if server.contains('=') {
        server
            .split(';')
            .find_map(|part| {
                part.strip_prefix("https=")
                    .or_else(|| part.strip_prefix("http="))
            })?
    } else {
        server.as_str()
    };
    Some(normalize_proxy(server))
}

fn normalize_proxy(proxy: &str) -> String {
    if proxy.contains("://") {
        proxy.to_string()
    } else {
        format!("http://{proxy}")
    }
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("打开文件失败: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{
        UpdateAsset, UpdateManifest, evaluate, hex, normalize_proxy, parse_version, sha256_file,
    };

    fn manifest(version: &str, sha256: &str) -> UpdateManifest {
        UpdateManifest {
            version: version.into(),
            notes: "修复若干问题".into(),
            installer: UpdateAsset {
                url: "https://example.com/app.exe".into(),
                sha256: sha256.into(),
                size: 10,
            },
        }
    }

    #[test]
    fn detects_newer_version() {
        let update = evaluate("0.1.0", &manifest("0.2.0", "abc")).unwrap();
        assert!(update.is_some());
        assert_eq!(update.unwrap().version, "0.2.0");

        assert!(evaluate("0.2.0", &manifest("0.2.0", "abc")).unwrap().is_none());
        assert!(evaluate("0.3.0", &manifest("0.2.0", "abc")).unwrap().is_none());
        assert!(evaluate("0.1.9", &manifest("0.1.10", "abc")).unwrap().is_some());
    }

    #[test]
    fn accepts_v_prefix() {
        assert_eq!(parse_version("v1.2.3").unwrap().to_string(), "1.2.3");
    }

    #[test]
    fn rejects_invalid_version() {
        assert!(evaluate("0.1.0", &manifest("latest", "abc")).is_err());
    }

    #[test]
    fn parses_release_manifest() {
        let json = r#"{
            "version": "v0.2.1",
            "notes": "修复项目弹框",
            "pub_date": "2026-10-25T10:00:00+08:00",
            "installer": {
                "url": "https://example.com/NiuMa-Workstation-0.2.1-setup.exe",
                "sha256": "ABCDEF",
                "size": 12345
            }
        }"#;
        let manifest: UpdateManifest = serde_json::from_str(json).unwrap();
        let update = evaluate("0.2.0", &manifest).unwrap().unwrap();
        assert_eq!(update.version, "0.2.1");
        assert_eq!(update.notes, "修复项目弹框");
        assert_eq!(update.installer.size, 12345);
    }

    #[test]
    fn normalizes_proxy() {
        assert_eq!(normalize_proxy("127.0.0.1:7897"), "http://127.0.0.1:7897");
        assert_eq!(
            normalize_proxy("http://127.0.0.1:7897"),
            "http://127.0.0.1:7897"
        );
    }

    #[test]
    fn hex_formats_lowercase() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff]), "000fff");
    }

    #[test]
    fn hashes_file() {
        let path = std::env::temp_dir().join(format!(
            "niu_ma_workstation_hash_{}.txt",
            std::process::id()
        ));
        std::fs::write(&path, b"hello").unwrap();
        let digest = sha256_file(&path).unwrap();
        assert_eq!(
            digest,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        let _ = std::fs::remove_file(&path);
    }
}
