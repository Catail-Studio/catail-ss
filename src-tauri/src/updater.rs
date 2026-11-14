//! Updates from GitHub Releases (`latest.json`, signed with the key in tauri.conf.json).
//! This is the only network access the app makes on its own, and it only downloads.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, Url};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::i18n::{t, t_with};
use crate::notify;

/// The repository the releases live in (the same one tauri.conf.json's endpoint points at).
const REPO: &str = "umutugrasan/catail";
/// How the download is going, sent to every window while an update comes in.
pub const EVENT_PROGRESS: &str = "update:progress";
/// Progress is sent at most this often; the download itself isn't slowed.
const PROGRESS_EVERY: Duration = Duration::from_millis(80);
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

/// The update found by the last check, ready to install.
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<Update>>);

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
}

pub async fn check<R: Runtime>(app: &AppHandle<R>) -> Result<Option<UpdateInfo>> {
    let update = app
        .updater()
        .context("updater unavailable")?
        .check()
        .await
        .context("update check failed")?;
    let info = update.as_ref().map(|u| UpdateInfo {
        version: u.version.clone(),
        notes: u.body.clone(),
    });
    *app.state::<PendingUpdate>().0.lock().unwrap() = update;
    Ok(info)
}

/// Downloads the pending update (telling the windows how far along it is), checks its signature
/// and installs it quietly; the installer starts the new version once it's done. If anything
/// fails on the way, the update stays pending, so it can be tried again.
pub async fn install<R: Runtime>(app: &AppHandle<R>) -> Result<()> {
    let pending = app
        .state::<PendingUpdate>()
        .0
        .lock()
        .unwrap()
        .clone()
        .context("no update to install")?;
    let mut update = pending;
    update.download_url = reachable_download_url(&update.download_url).await;

    let mut downloaded = 0_u64;
    let mut last_sent = Instant::now() - PROGRESS_EVERY;
    let bytes = update
        .download(
            |chunk, total| {
                downloaded += chunk as u64;
                if last_sent.elapsed() >= PROGRESS_EVERY || total == Some(downloaded) {
                    last_sent = Instant::now();
                    let _ = app.emit(EVENT_PROGRESS, Progress { downloaded, total });
                }
            },
            || {},
        )
        .await
        .context("update download failed")?;
    // Verifies the signature, starts the installer and leaves; nothing after this runs on success.
    update.install(bytes).context("update install failed")?;
    app.restart()
}

/// The address latest.json gives for the installer, or, if nothing is there (a release published
/// under a different tag than the one latest.json was made with), the same file found by name
/// among the published releases. The file's signature is checked either way.
async fn reachable_download_url(url: &Url) -> Url {
    match find_download_url(url).await {
        Ok(found) => found,
        Err(_) => url.clone(),
    }
}

async fn find_download_url(url: &Url) -> Result<Url> {
    // The updater does the same: reqwest is built without a default TLS crypto provider.
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let client = reqwest::Client::builder()
        .timeout(LOOKUP_TIMEOUT)
        .user_agent(concat!("catail/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("cannot create HTTP client")?;
    if client
        .head(url.clone())
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
    {
        return Ok(url.clone());
    }
    let name = url
        .path_segments()
        .and_then(|mut parts| parts.next_back())
        .filter(|name| !name.is_empty())
        .context("no file name in the download address")?
        .to_owned();
    let releases: serde_json::Value = client
        .get(format!(
            "https://api.github.com/repos/{REPO}/releases?per_page=30"
        ))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("cannot list releases")?
        .error_for_status()
        .context("cannot list releases")?
        .json()
        .await
        .context("cannot read releases")?;
    asset_url(&releases, &name).context("installer not found among the releases")
}

/// The download address of the published (not draft) release asset called `name`.
fn asset_url(releases: &serde_json::Value, name: &str) -> Option<Url> {
    releases
        .as_array()?
        .iter()
        .filter(|release| !release["draft"].as_bool().unwrap_or(false))
        .flat_map(|release| release["assets"].as_array().into_iter().flatten())
        .find(|asset| asset["name"].as_str() == Some(name))
        .and_then(|asset| asset["browser_download_url"].as_str())
        .and_then(|url| Url::parse(url).ok())
}

/// Startup check. Failures (offline, no release yet) are silent; a found update is announced.
pub fn check_in_background<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Ok(Some(info)) = check(&app).await {
            let body = t_with(
                "notify.update_available_body",
                &[("version", &info.version)],
            );
            notify::show(&app, &t("notify.update_available_title"), &body);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_the_installer_by_name_under_any_tag() {
        let releases = json!([
            { "draft": true, "assets": [{ "name": "Catail_0.9.5_x64-setup.exe", "browser_download_url": "https://github.com/x/y/releases/download/draft/Catail_0.9.5_x64-setup.exe" }] },
            { "draft": false, "assets": [
                { "name": "latest.json", "browser_download_url": "https://github.com/x/y/releases/download/v9.0.5/latest.json" },
                { "name": "Catail_0.9.5_x64-setup.exe", "browser_download_url": "https://github.com/x/y/releases/download/v9.0.5/Catail_0.9.5_x64-setup.exe" }
            ] }
        ]);
        let found = asset_url(&releases, "Catail_0.9.5_x64-setup.exe").unwrap();
        assert_eq!(
            found.as_str(),
            "https://github.com/x/y/releases/download/v9.0.5/Catail_0.9.5_x64-setup.exe"
        );
        assert!(asset_url(&releases, "Catail_1.0.0_x64-setup.exe").is_none());
        assert!(asset_url(&json!({ "message": "rate limited" }), "x").is_none());
    }
}
