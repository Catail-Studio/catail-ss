//! Catail's side of the Catail Studio bridge (bridge.rs): keeps Catail Convert told about the
//! queue, and hands captures and queue items over to it for converting. Nothing here goes
//! online; when Convert isn't installed, none of it shows.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tauri::{AppHandle, Manager, Runtime};
use uuid::Uuid;

use crate::bridge::{self, Presence, QueueEntry, QueueState};
use crate::i18n::t;
use crate::queue::item::QueueItemKind;
use crate::state::AppState;

/// Captures and queued text handed to Convert wait here; files older than a day are removed.
fn handoff_dir() -> PathBuf {
    std::env::temp_dir().join("catail-to-convert")
}

fn kind_name(kind: QueueItemKind) -> &'static str {
    match kind {
        QueueItemKind::Image => "image",
        QueueItemKind::Text => "text",
        QueueItemKind::File => "file",
    }
}

/// Tells the other apps what Catail looks like now: cat, language, queue.
pub fn announce<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    let cat = {
        let settings = state.settings.lock().unwrap();
        serde_json::to_value(settings.cat)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    };
    let queue = {
        let queue = state.queue.lock().unwrap();
        let items: Vec<QueueEntry> = queue
            .items()
            .iter()
            .map(|item| QueueEntry {
                kind: kind_name(item.kind).into(),
                path: item.path_to_paste().map(|p| p.display().to_string()),
                text: item.text.clone(),
                name: item.file_name(),
            })
            .collect();
        QueueState {
            count: items.len(),
            text_mode: state.text_mode.load(std::sync::atomic::Ordering::SeqCst),
            items,
        }
    };
    bridge::announce(&Presence {
        app: bridge::CATAIL.into(),
        version: app.package_info().version.to_string(),
        exe: std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
        cat,
        language: crate::i18n::current(),
        queue: Some(queue),
    });
}

/// Whether Catail Convert is on this computer.
pub fn convert_installed() -> bool {
    bridge::find_exe(bridge::CONVERT).is_some()
}

fn launch_convert(flag: &str, paths: &[PathBuf]) -> Result<(), String> {
    let exe = bridge::find_exe(bridge::CONVERT).ok_or_else(|| t("convert.missing"))?;
    let mut args = vec![flag.to_string()];
    args.extend(paths.iter().map(|p| p.display().to_string()));
    bridge::launch(&exe, &args).map_err(|e| format!("{}: {e}", t("convert.missing")))
}

/// A file name that is free in the hand-off folder.
fn free_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut path = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}).{ext}"));
        n += 1;
    }
    path
}

fn prepare_handoff_dir() -> Result<PathBuf, String> {
    let dir = handoff_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // Yesterday's hand-overs have been converted (or forgotten) by now.
    if let Ok(entries) = fs::read_dir(&dir) {
        let day = Duration::from_secs(24 * 60 * 60);
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| SystemTime::now().duration_since(m).ok())
                .is_some_and(|age| age > day);
            if old {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    Ok(dir)
}

/// The capture toolbar's "Dönüştür": the selection opens in Convert's cat box, with its format
/// bubbles ready.
pub fn send_capture(image: &image::RgbaImage) -> Result<(), String> {
    let dir = prepare_handoff_dir()?;
    let stem = format!(
        "{} {}",
        t("convert.capture_name"),
        chrono::Local::now().format("%Y-%m-%d %H.%M.%S")
    );
    let path = free_path(&dir, &stem, "png");
    image.save(&path).map_err(|e| e.to_string())?;
    launch_convert(bridge::ARG_BOX, &[path])
}

/// Queued text as a file Convert can read: JSON stays JSON, the rest is plain text.
fn text_file(dir: &Path, text: &str, n: usize) -> Result<PathBuf, String> {
    let trimmed = text.trim();
    let ext = if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && serde_json::from_str::<serde_json::Value>(trimmed).is_ok()
    {
        "json"
    } else {
        "txt"
    };
    let path = free_path(dir, &format!("{} {n}", t("convert.text_name")), ext);
    fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path)
}

/// The panel's "Dönüştür": these queue items (all of them when `ids` is `None`) open in
/// Convert's studio window, in queue order.
pub fn send_queue<R: Runtime>(app: &AppHandle<R>, ids: Option<&[Uuid]>) -> Result<(), String> {
    let dir = prepare_handoff_dir()?;
    let items: Vec<_> = {
        let state = app.state::<AppState>();
        let queue = state.queue.lock().unwrap();
        queue
            .items()
            .iter()
            .filter(|item| ids.is_none_or(|ids| ids.contains(&item.id)))
            .cloned()
            .collect()
    };
    let mut paths = Vec::new();
    for (n, item) in items.iter().enumerate() {
        match (&item.text, item.path_to_paste()) {
            (_, Some(path)) => paths.push(path.to_path_buf()),
            (Some(text), None) => paths.push(text_file(&dir, text, n + 1)?),
            (None, None) => {}
        }
    }
    if paths.is_empty() {
        return Ok(());
    }
    launch_convert(bridge::ARG_STUDIO, &paths)
}

/// Tray: open Convert's studio window.
pub fn open_convert() -> Result<(), String> {
    launch_convert(bridge::ARG_STUDIO, &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_names_do_not_clash() {
        let dir = std::env::temp_dir().join(format!("catail-studio-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let a = free_path(&dir, "x", "png");
        fs::write(&a, b"1").unwrap();
        let b = free_path(&dir, "x", "png");
        assert_ne!(a, b);
        assert!(b.ends_with("x (2).png"));
        let json = text_file(&dir, "{\"a\":1}", 1).unwrap();
        assert_eq!(json.extension().unwrap(), "json");
        let txt = text_file(&dir, "merhaba", 2).unwrap();
        assert_eq!(txt.extension().unwrap(), "txt");
        fs::remove_dir_all(&dir).unwrap();
    }
}
