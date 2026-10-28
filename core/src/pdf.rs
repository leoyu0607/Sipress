//! HTML → PDF：用本機的 Chrome / Edge（headless 模式）列印成 PDF
//!
//! 不內建 PDF 排版，版面與瀏覽器「列印 → 另存為 PDF」相同。找不到瀏覽器時回傳錯誤，
//! 呼叫端可改為開啟 HTML 讓使用者自己列印。

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 轉檔逾時
const TIMEOUT: Duration = Duration::from_secs(60);

/// 找本機的 Chrome / Edge / Chromium 執行檔
pub fn find_browser() -> Option<PathBuf> {
    candidates().into_iter().find(|p| p.is_file())
}

fn candidates() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        for app in [
            "Google Chrome.app/Contents/MacOS/Google Chrome",
            "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "Chromium.app/Contents/MacOS/Chromium",
        ] {
            v.push(Path::new("/Applications").join(app));
            if let Some(home) = std::env::var_os("HOME") {
                v.push(Path::new(&home).join("Applications").join(app));
            }
        }
    } else if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"] {
            if let Some(base) = std::env::var_os(var) {
                let base = PathBuf::from(base);
                v.push(base.join(r"Microsoft\Edge\Application\msedge.exe"));
                v.push(base.join(r"Google\Chrome\Application\chrome.exe"));
            }
        }
    } else {
        let names = [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "microsoft-edge",
            "microsoft-edge-stable",
        ];
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                for n in names {
                    v.push(dir.join(n));
                }
            }
        }
    }
    v
}

/// 把 HTML 檔列印成 PDF
pub fn html_to_pdf(html: &Path, pdf: &Path) -> Result<()> {
    let browser = find_browser().context(
        "找不到 Chrome 或 Edge，無法轉成 PDF；請用瀏覽器開啟 HTML 報告，再「列印 → 另存為 PDF」",
    )?;
    let html = std::fs::canonicalize(html).with_context(|| format!("找不到 {}", html.display()))?;
    if let Some(dir) = pdf.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(pdf);
    // 每次用新的使用者資料目錄，避免與使用者正在開的瀏覽器共用設定檔而失敗
    let profile = std::env::temp_dir().join(format!("sipress-pdf-{}", std::process::id()));
    // 不接 stdout / stderr：瀏覽器的背景子行程會繼承管線，讀到 EOF 前會一直等
    let mut child = Command::new(&browser)
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-first-run")
        .arg("--no-pdf-header-footer")
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--print-to-pdf={}", pdf.display()))
        .arg(file_url(&html))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("無法執行 {}", browser.display()))?;
    let deadline = Instant::now() + TIMEOUT;
    let exited = loop {
        if child.try_wait()?.is_some() {
            break true;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break false;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let _ = std::fs::remove_dir_all(&profile);
    if !pdf.is_file() {
        bail!(
            "{} {}沒有產生 PDF",
            browser.display(),
            if exited { "" } else { "逾時，" }
        );
    }
    Ok(())
}

/// 絕對路徑 → file:// URL（空白、中文等字元百分比編碼）
fn file_url(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    // Windows 的 canonicalize 會加上 \\?\ 前綴
    let s = s.trim_start_matches("//?/");
    let mut url = String::from(if s.starts_with('/') {
        "file://"
    } else {
        "file:///"
    });
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => {
                url.push(b as char)
            }
            _ => url.push_str(&format!("%{:02X}", b)),
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_file_urls() {
        assert_eq!(
            file_url(Path::new("/tmp/a b/報告.html")),
            "file:///tmp/a%20b/%E5%A0%B1%E5%91%8A.html"
        );
        assert_eq!(
            file_url(Path::new(r"\\?\C:\Users\me\r.html")),
            "file:///C:/Users/me/r.html"
        );
    }
}
