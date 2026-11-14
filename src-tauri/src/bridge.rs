//! The Catail Studio bridge: how the studio's apps (Catail, Catail Convert) find each other on
//! this computer and hand work over. Entirely local and offline:
//!
//! - Each app keeps `%LOCALAPPDATA%\Catail Studio\bridge\<app>.json` up to date: its version,
//!   exe, cat and language, and for Catail the queue.
//! - While it runs, each app holds `<app>.lock` open with no sharing; the others can tell it is
//!   running because they can't open that file. A crash releases it, so it never lies.
//! - Work is handed over on the command line of the other app's exe (its single-instance
//!   plugin forwards the arguments to the running copy, or the launch starts it).
//!
//! The same file lives in both repositories; keep them in step.

// Shared by the apps; each one uses its own part of it.
#![allow(dead_code)]

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CATAIL: &str = "catail";
pub const CONVERT: &str = "convert";

/// Catail: add these files to the queue.
pub const ARG_ENQUEUE: &str = "--enqueue";
/// Convert: open the cat box with these files.
pub const ARG_BOX: &str = "--box";
/// Convert: open the studio window with these files.
pub const ARG_STUDIO: &str = "--studio";

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct QueueEntry {
    /// "image", "text" or "file".
    pub kind: String,
    pub path: Option<String>,
    pub text: Option<String>,
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct QueueState {
    pub count: usize,
    pub text_mode: bool,
    pub items: Vec<QueueEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Presence {
    pub app: String,
    pub version: String,
    pub exe: String,
    /// The chosen cat ("orange", "gray", …) and interface language ("tr", "en", …).
    pub cat: String,
    pub language: String,
    /// Catail only.
    pub queue: Option<QueueState>,
}

pub fn dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Catail Studio").join("bridge")
}

fn presence_path(app: &str) -> PathBuf {
    dir().join(format!("{app}.json"))
}

fn lock_path(app: &str) -> PathBuf {
    dir().join(format!("{app}.lock"))
}

/// Opens this app's lock file with no sharing. Keep the returned file for the app's lifetime.
pub fn hold_lock(app: &str) -> Option<File> {
    fs::create_dir_all(dir()).ok()?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    options.open(lock_path(app)).ok()
}

/// Whether the other app is running right now.
pub fn is_running(app: &str) -> bool {
    let path = lock_path(app);
    if !path.exists() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // ERROR_SHARING_VIOLATION: someone holds it, so the app is up.
        match OpenOptions::new().read(true).share_mode(0).open(&path) {
            Ok(_) => false,
            Err(e) => e.raw_os_error() == Some(32),
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Writes this app's presence; atomic, so a reader never sees half a file.
pub fn announce(presence: &Presence) {
    let Ok(json) = serde_json::to_vec_pretty(presence) else { return };
    if fs::create_dir_all(dir()).is_err() {
        return;
    }
    let path = presence_path(&presence.app);
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, json).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

pub fn read(app: &str) -> Option<Presence> {
    let bytes = fs::read(presence_path(app)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Where the installer puts each app (per-user NSIS install).
fn default_exe(app: &str) -> Option<PathBuf> {
    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    Some(match app {
        CATAIL => base.join("Catail").join("catail.exe"),
        CONVERT => base.join("Catail Convert").join("catail-convert.exe"),
        _ => return None,
    })
}

/// The other app's exe, if it is on this computer: the one it last announced, or the one its
/// installer would have put in place.
pub fn find_exe(app: &str) -> Option<PathBuf> {
    read(app)
        .map(|p| PathBuf::from(p.exe))
        .filter(|p| p.is_file())
        .or_else(|| default_exe(app).filter(|p| p.is_file()))
}

/// Starts the other app (or hands the arguments to its running copy).
pub fn launch(exe: &Path, args: &[String]) -> std::io::Result<()> {
    std::process::Command::new(exe).args(args).spawn().map(|_| ())
}

/// What one app knows about the other, for its window.
#[derive(Serialize, Clone, Debug, Default)]
pub struct Peer {
    pub installed: bool,
    pub running: bool,
    pub version: Option<String>,
    pub cat: Option<String>,
    pub language: Option<String>,
    pub queue: Option<QueueState>,
}

pub fn peer(app: &str) -> Peer {
    let installed = find_exe(app).is_some();
    let running = installed && is_running(app);
    let presence = read(app);
    Peer {
        installed,
        running,
        version: presence.as_ref().map(|p| p.version.clone()),
        cat: presence.as_ref().map(|p| p.cat.clone()).filter(|c| !c.is_empty()),
        language: presence.as_ref().map(|p| p.language.clone()).filter(|l| !l.is_empty()),
        // A queue that isn't running isn't there.
        queue: if running { presence.and_then(|p| p.queue) } else { None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_roundtrip_and_defaults() {
        let p = Presence {
            app: "x".into(),
            version: "1.0.0".into(),
            queue: Some(QueueState { count: 1, text_mode: false, items: vec![QueueEntry { kind: "text".into(), text: Some("a".into()), ..Default::default() }] }),
            ..Default::default()
        };
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Presence>(&json).unwrap(), p);
        // Older or newer files with missing fields still read.
        assert_eq!(serde_json::from_str::<Presence>("{\"app\":\"y\"}").unwrap().app, "y");
    }
}
