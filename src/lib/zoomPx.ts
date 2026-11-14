// Pages drawn bigger or smaller with CSS `zoom` ("Rozet boyutu") measure themselves with
// getBoundingClientRect. Current WebView2 already gives those rects in window px; older engines
// gave them unzoomed. This tells the two apart once per zoom, by measuring something of a known
// size, so what a page reports in window px is right on either.

let cached: { zoom: number; factor: number } | null = null;

/** The page's CSS zoom (1 when none is set). */
export function pageZoom(): number {
  return Number(document.documentElement.style.zoom) || 1;
}

/** What a getBoundingClientRect length is multiplied by to be window px. */
export function rectToWindowPx(): number {
  const zoom = pageZoom();
  if (zoom === 1) return 1;
  if (cached?.zoom === zoom) return cached.factor;
  const probe = document.createElement("div");
  probe.style.cssText = "position:fixed;left:0;top:0;width:100px;height:1px;visibility:hidden;pointer-events:none";
  document.body.append(probe);
  const measured = probe.getBoundingClientRect().width;
  probe.remove();
  // Zoomed rects: 100 × zoom wide, nothing to do. Unzoomed: 100 wide, so times the zoom.
  const factor = measured > 0 ? (100 * zoom) / measured : 1;
  cached = { zoom, factor: Math.abs(factor - 1) < 0.01 ? 1 : factor };
  return cached.factor;
}
