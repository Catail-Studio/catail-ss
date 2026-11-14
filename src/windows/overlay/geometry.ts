// Pure selection math for the overlay. CSS pixels unless noted.
import type { DesktopRect, ImageRect } from "../../lib/ipc";

export interface Point {
  x: number;
  y: number;
}

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Size {
  width: number;
  height: number;
}

export const clamp = (value: number, min: number, max: number) =>
  Math.min(Math.max(value, min), max);

/** Rectangle spanned by two corners, in any drag direction. */
export function rectFromPoints(a: Point, b: Point): Rect {
  return {
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    width: Math.abs(a.x - b.x),
    height: Math.abs(a.y - b.y),
  };
}

export const clampPoint = (p: Point, view: Size): Point => ({
  x: clamp(p.x, 0, view.width),
  y: clamp(p.y, 0, view.height),
});

export const containsPoint = (r: Rect, p: Point) =>
  p.x >= r.x && p.x <= r.x + r.width && p.y >= r.y && p.y <= r.y + r.height;

/** Moves `r` so its top-left lands on `origin`, kept fully inside the view. */
export const moveRect = (r: Rect, origin: Point, view: Size): Rect => ({
  ...r,
  x: clamp(origin.x, 0, view.width - r.width),
  y: clamp(origin.y, 0, view.height - r.height),
});

/**
 * Converts a CSS-pixel selection to physical image pixels. `scale` is image width over
 * view width (the monitor's DPI factor). Edges are rounded independently so adjacent
 * selections tile without gaps, then clamped to the image.
 */
export function toImageRect(r: Rect, scale: number, image: Size): ImageRect {
  const left = clamp(Math.round(r.x * scale), 0, image.width);
  const top = clamp(Math.round(r.y * scale), 0, image.height);
  const right = clamp(Math.round((r.x + r.width) * scale), 0, image.width);
  const bottom = clamp(Math.round((r.y + r.height) * scale), 0, image.height);
  return { x: left, y: top, width: right - left, height: bottom - top };
}

/** A selection edge or corner that can be dragged to resize: n, s, e, w or a pair. */
export type Handle = "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";

/** How close (CSS px, either side of the border) the pointer must be to grab an edge. */
export const HANDLE_REACH = 7;

/** The edge or corner of `r` under `p`, if any. Corners win over edges. */
export function handleAt(r: Rect, p: Point, reach = HANDLE_REACH): Handle | null {
  const inX = p.x >= r.x - reach && p.x <= r.x + r.width + reach;
  const inY = p.y >= r.y - reach && p.y <= r.y + r.height + reach;
  if (!inX || !inY) return null;
  const near = (a: number, b: number) => Math.abs(a - b) <= reach;
  // On a tiny selection the edges overlap; pick the nearer one.
  const left = near(p.x, r.x) && Math.abs(p.x - r.x) <= Math.abs(p.x - (r.x + r.width));
  const right = !left && near(p.x, r.x + r.width);
  const top = near(p.y, r.y) && Math.abs(p.y - r.y) <= Math.abs(p.y - (r.y + r.height));
  const bottom = !top && near(p.y, r.y + r.height);
  const v = top ? "n" : bottom ? "s" : "";
  const h = left ? "w" : right ? "e" : "";
  return (v + h || null) as Handle | null;
}

/**
 * Drags `handle` of `start` to `p`; the opposite edges stay put. Dragging past the
 * opposite edge flips the rectangle instead of giving it a negative size.
 */
export function resizeRect(start: Rect, handle: Handle, p: Point): Rect {
  let left = start.x;
  let top = start.y;
  let right = start.x + start.width;
  let bottom = start.y + start.height;
  if (handle.includes("w")) left = p.x;
  if (handle.includes("e")) right = p.x;
  if (handle.includes("n")) top = p.y;
  if (handle.includes("s")) bottom = p.y;
  return rectFromPoints({ x: left, y: top }, { x: right, y: bottom });
}

/** CSS cursor class suffix for a handle; the corner paw serves all four corners. */
export const handleCursor = (handle: Handle) =>
  handle === "n" || handle === "s" ? "ns" : handle === "e" || handle === "w" ? "ew" : "corner";

const GAP = 8;

/**
 * Places the toolbar under the selection's bottom-right corner; above it when there's no
 * room below; inside the selection's bottom edge when there's room neither below nor above.
 */
export function toolbarPosition(sel: Rect, bar: Size, view: Size): Point {
  const x = clamp(sel.x + sel.width - bar.width, GAP, view.width - bar.width - GAP);
  const below = sel.y + sel.height + GAP;
  if (below + bar.height <= view.height) return { x, y: below };
  const above = sel.y - GAP - bar.height;
  if (above >= 0) return { x, y: above };
  return { x, y: Math.max(sel.y + sel.height - bar.height - GAP, GAP) };
}

/**
 * Places the vertical drawing palette to the right of the selection, or to its left when
 * there's no room, or inside its right edge when neither side fits.
 */
export function sidebarPosition(sel: Rect, bar: Size, view: Size): Point {
  const y = clamp(sel.y, GAP, view.height - bar.height - GAP);
  const right = sel.x + sel.width + GAP;
  if (right + bar.width <= view.width) return { x: right, y };
  const left = sel.x - GAP - bar.width;
  if (left >= 0) return { x: left, y };
  return { x: Math.max(sel.x + sel.width - bar.width - GAP, GAP), y };
}

/** Places the size label just above the selection, or inside it at the top edge. */
export function sizeLabelPosition(sel: Rect, labelHeight: number): Point {
  const above = sel.y - labelHeight - 4;
  return { x: sel.x, y: above >= 0 ? above : sel.y + 4 };
}

/**
 * Places a QR code's link chip beside the code, like a phone camera: to its right, vertically
 * centred; to its left when the right side has no room; below (or above) it when neither
 * side has. Always kept on screen.
 */
export function chipPosition(code: Rect, chip: Size, view: Size): Point {
  const gap = 10;
  const centredY = clamp(code.y + code.height / 2 - chip.height / 2, GAP, view.height - chip.height - GAP);
  const right = code.x + code.width + gap;
  if (right + chip.width <= view.width - GAP) return { x: right, y: centredY };
  const left = code.x - gap - chip.width;
  if (left >= GAP) return { x: left, y: centredY };
  const x = clamp(code.x + code.width / 2 - chip.width / 2, GAP, view.width - chip.width - GAP);
  const below = code.y + code.height + gap;
  if (below + chip.height <= view.height - GAP) return { x, y: below };
  return { x, y: Math.max(GAP, code.y - gap - chip.height) };
}

/*
 * Several monitors. Each overlay works in its own CSS px, with (0, 0) at its monitor's corner
 * and `scale` physical px to each; the desktop plane is physical px. A selection may run past
 * its overlay onto the monitors beside it, and only where there is one.
 */

/** A desktop rectangle in an overlay's px. */
export const fromDesktop = (d: DesktopRect, scale: number, origin: Point): Rect => ({
  x: (d.x - origin.x) / scale,
  y: (d.y - origin.y) / scale,
  width: d.width / scale,
  height: d.height / scale,
});

/** An overlay's selection on the desktop plane; edges rounded on their own, like toImageRect. */
export function toDesktop(r: Rect, scale: number, origin: Point): DesktopRect {
  const left = Math.round(origin.x + r.x * scale);
  const top = Math.round(origin.y + r.y * scale);
  const right = Math.round(origin.x + (r.x + r.width) * scale);
  const bottom = Math.round(origin.y + (r.y + r.height) * scale);
  return { x: left, y: top, width: Math.max(right - left, 0), height: Math.max(bottom - top, 0) };
}

/** Whether `d` lies wholly on the monitor `m` (both desktop px). */
export const withinMonitor = (d: DesktopRect, m: DesktopRect) =>
  d.x >= m.x && d.y >= m.y && d.x + d.width <= m.x + m.width && d.y + d.height <= m.y + m.height;

/**
 * `p` kept on a monitor: where it is if it's on one, otherwise the nearest point on any of
 * them. So a selection can be stretched onto a monitor beside this one, and stops at an edge
 * with no monitor past it.
 */
export function clampToAreas(p: Point, areas: Rect[]): Point {
  let best: Point | null = null;
  let bestDistance = Infinity;
  for (const a of areas) {
    const q = { x: clamp(p.x, a.x, a.x + a.width), y: clamp(p.y, a.y, a.y + a.height) };
    const distance = Math.hypot(q.x - p.x, q.y - p.y);
    if (distance === 0) return p;
    if (distance < bestDistance) {
      best = q;
      bestDistance = distance;
    }
  }
  return best ?? p;
}

/** The smallest rectangle round all of `areas`. */
export function boundsOf(areas: Rect[]): Rect {
  const left = Math.min(...areas.map((a) => a.x));
  const top = Math.min(...areas.map((a) => a.y));
  const right = Math.max(...areas.map((a) => a.x + a.width));
  const bottom = Math.max(...areas.map((a) => a.y + a.height));
  return { x: left, y: top, width: right - left, height: bottom - top };
}

/** Moves `r` so its top-left lands on `origin`, kept inside `bounds`. */
export const moveRectWithin = (r: Rect, origin: Point, bounds: Rect): Rect => ({
  ...r,
  x: clamp(origin.x, bounds.x, bounds.x + bounds.width - r.width),
  y: clamp(origin.y, bounds.y, bounds.y + bounds.height - r.height),
});
