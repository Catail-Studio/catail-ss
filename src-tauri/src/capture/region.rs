//! Region capture: freeze all monitors, let the user pick a rectangle on an overlay, crop it.

use std::borrow::Cow;
use std::sync::atomic::Ordering;

use anyhow::{anyhow, Result};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{AppHandle, Manager, Runtime, UriSchemeContext};

use crate::capture::MonitorShot;
use crate::i18n::t_with;
use crate::state::{AppState, CaptureSession};
use crate::{notify, overlay};

/// URI scheme the overlay loads frozen frames from: `capture://localhost/<session>-<index>.png`.
pub const PROTOCOL: &str = "capture";

/// Selection in physical pixels, relative to one monitor's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct SelectionRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// A selection in physical desktop pixels: the plane every monitor sits in, as Windows lays
/// them out (so it can start left of or above the main one). One selection may cover several.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct DesktopRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Cuts `rect` out of every monitor it covers and lays the pieces into one picture, where the
/// desktop has them. Anything over no monitor stays transparent. `None` when nothing is covered.
pub fn stitch(shots: &[MonitorShot], rect: DesktopRect) -> Option<RgbaImage> {
    if rect.width == 0 || rect.height == 0 {
        return None;
    }
    let mut out = RgbaImage::new(rect.width, rect.height);
    let (right, bottom) = (
        i64::from(rect.x) + i64::from(rect.width),
        i64::from(rect.y) + i64::from(rect.height),
    );
    let mut covered = false;
    for shot in shots {
        let (w, h) = shot.image.dimensions();
        let left = i64::from(rect.x).max(i64::from(shot.x));
        let top = i64::from(rect.y).max(i64::from(shot.y));
        let piece_right = right.min(i64::from(shot.x) + i64::from(w));
        let piece_bottom = bottom.min(i64::from(shot.y) + i64::from(h));
        if piece_right <= left || piece_bottom <= top {
            continue;
        }
        let piece = image::imageops::crop_imm(
            &shot.image,
            (left - i64::from(shot.x)) as u32,
            (top - i64::from(shot.y)) as u32,
            (piece_right - left) as u32,
            (piece_bottom - top) as u32,
        )
        .to_image();
        image::imageops::replace(
            &mut out,
            &piece,
            left - i64::from(rect.x),
            top - i64::from(rect.y),
        );
        covered = true;
    }
    covered.then_some(out)
}

/// The current session's picture of `rect`, across monitors.
pub fn stitch_selection(state: &AppState, rect: DesktopRect) -> Result<RgbaImage> {
    let session = state.session.lock().unwrap();
    let shots = &session
        .as_ref()
        .ok_or_else(|| anyhow!("no capture session"))?
        .shots;
    stitch(shots, rect).ok_or_else(|| anyhow!("empty selection"))
}

/// Hotkey entry point. Runs on a worker thread.
pub fn start<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    if state.overlay_active.swap(true, Ordering::SeqCst) {
        return; // overlay already open
    }
    if let Err(e) = open_overlays(app, &state) {
        state.overlay_active.store(false, Ordering::SeqCst);
        crate::badge::refresh(app);
        let msg = t_with("notify.capture_failed", &[("error", &format!("{e:#}"))]);
        notify::show(app, &crate::i18n::t("app.name"), &msg);
    }
}

fn open_overlays<R: Runtime>(app: &AppHandle<R>, state: &AppState) -> Result<()> {
    crate::panel::hide(app);
    crate::badge::hide_for_capture(app);
    let shots = crate::capture::capture_all_monitors()?;
    let bounds: Vec<_> = shots
        .iter()
        .map(|s| (s.x, s.y, s.image.width(), s.image.height()))
        .collect();
    let id = state.next_session_id();
    *state.session.lock().unwrap() = Some(CaptureSession { id, shots });
    overlay::present(app, &bounds)
}

/// Crops the selection out of monitor `index` of the current session.
pub fn crop_selection(state: &AppState, index: usize, rect: SelectionRect) -> Result<RgbaImage> {
    let session = state.session.lock().unwrap();
    let shot = session
        .as_ref()
        .and_then(|s| s.shots.get(index))
        .ok_or_else(|| anyhow!("no capture for monitor {index}"))?;
    crop(&shot.image, rect).ok_or_else(|| anyhow!("empty selection"))
}

/// Clamps `rect` to the image and returns it, or `None` if nothing is left.
pub fn clamp_rect(rect: SelectionRect, img_w: u32, img_h: u32) -> Option<SelectionRect> {
    let x = rect.x.min(img_w);
    let y = rect.y.min(img_h);
    let width = rect.width.min(img_w - x);
    let height = rect.height.min(img_h - y);
    (width > 0 && height > 0).then_some(SelectionRect {
        x,
        y,
        width,
        height,
    })
}

pub fn crop(img: &RgbaImage, rect: SelectionRect) -> Option<RgbaImage> {
    let r = clamp_rect(rect, img.width(), img.height())?;
    Some(image::imageops::crop_imm(img, r.x, r.y, r.width, r.height).to_image())
}

/// Parses `/<session>-<index>.png`.
pub fn parse_frame_path(path: &str) -> Option<(u64, usize)> {
    let name = path.trim_start_matches('/').strip_suffix(".png")?;
    let (session, index) = name.split_once('-')?;
    Some((session.parse().ok()?, index.parse().ok()?))
}

/// Serves frozen frames to the overlay webviews.
pub fn serve<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
) -> Response<Cow<'static, [u8]>> {
    let state = ctx.app_handle().state::<AppState>();
    let png = parse_frame_path(request.uri().path()).and_then(|(id, index)| {
        let session = state.session.lock().unwrap();
        let session = session.as_ref().filter(|s| s.id == id)?;
        session.shots.get(index).map(|shot| shot.png.clone())
    });

    let builder = Response::builder()
        .header(header::CACHE_CONTROL, "no-store")
        // The overlay draws the frame into a canvas to export annotations; without CORS the
        // canvas would be tainted and refuse to export.
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
    match png {
        Some(png) => builder
            .header(header::CONTENT_TYPE, "image/png")
            .body(Cow::Owned(png)),
        None => builder
            .status(StatusCode::NOT_FOUND)
            .body(Cow::Borrowed(&[][..])),
    }
    .expect("static response parts are valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(x: i32, y: i32, w: u32, h: u32, value: u8) -> MonitorShot {
        MonitorShot {
            x,
            y,
            image: RgbaImage::from_pixel(w, h, image::Rgba([value, value, value, 255])),
            png: Vec::new(),
        }
    }

    #[test]
    fn stitches_a_selection_across_two_monitors() {
        // The main monitor at 0,0 and another, taller one to its left, a little higher.
        let shots = [shot(0, 0, 100, 50, 10), shot(-80, -20, 80, 90, 200)];
        let rect = DesktopRect {
            x: -30,
            y: 10,
            width: 60,
            height: 20,
        };
        let out = stitch(&shots, rect).unwrap();
        assert_eq!(out.dimensions(), (60, 20));
        assert_eq!(out.get_pixel(0, 0)[0], 200); // left monitor
        assert_eq!(out.get_pixel(29, 19)[0], 200);
        assert_eq!(out.get_pixel(30, 0)[0], 10); // main monitor
        assert_eq!(out.get_pixel(59, 19)[0], 10);
    }

    #[test]
    fn leaves_gaps_between_monitors_transparent() {
        let shots = [shot(0, 0, 10, 10, 50)];
        let out = stitch(
            &shots,
            DesktopRect {
                x: 5,
                y: 5,
                width: 10,
                height: 10,
            },
        )
        .unwrap();
        assert_eq!(out.get_pixel(0, 0)[3], 255);
        assert_eq!(out.get_pixel(9, 9)[3], 0);
        assert!(stitch(
            &shots,
            DesktopRect {
                x: 50,
                y: 50,
                width: 5,
                height: 5
            }
        )
        .is_none());
        assert!(stitch(
            &shots,
            DesktopRect {
                x: 0,
                y: 0,
                width: 0,
                height: 5
            }
        )
        .is_none());
    }
    use image::Rgba;

    fn rect(x: u32, y: u32, width: u32, height: u32) -> SelectionRect {
        SelectionRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn clamp_keeps_inner_rect() {
        assert_eq!(
            clamp_rect(rect(10, 20, 30, 40), 100, 100),
            Some(rect(10, 20, 30, 40))
        );
    }

    #[test]
    fn clamp_trims_overflow() {
        assert_eq!(
            clamp_rect(rect(90, 95, 30, 40), 100, 100),
            Some(rect(90, 95, 10, 5))
        );
    }

    #[test]
    fn clamp_rejects_empty_and_outside() {
        assert_eq!(clamp_rect(rect(10, 10, 0, 5), 100, 100), None);
        assert_eq!(clamp_rect(rect(100, 10, 5, 5), 100, 100), None);
    }

    #[test]
    fn crop_returns_exact_pixels() {
        let mut img = RgbaImage::new(4, 4);
        img.put_pixel(2, 1, Rgba([1, 2, 3, 255]));
        let out = crop(&img, rect(2, 1, 2, 3)).unwrap();
        assert_eq!(out.dimensions(), (2, 3));
        assert_eq!(out.get_pixel(0, 0), &Rgba([1, 2, 3, 255]));
    }

    #[test]
    fn frame_paths() {
        assert_eq!(parse_frame_path("/12-0.png"), Some((12, 0)));
        assert_eq!(parse_frame_path("3-2.png"), Some((3, 2)));
        assert_eq!(parse_frame_path("/3-2"), None);
        assert_eq!(parse_frame_path("/x-2.png"), None);
        assert_eq!(parse_frame_path("/favicon.ico"), None);
    }
}
