//! The EasySearch mark, drawn in code.
//!
//! One design serves three places — the window icon, the tray pixmap and the
//! mark in the About dialog and the empty state — and all three are evaluated
//! from the same geometry, so they cannot drift apart. The matching
//! `packaging/icons/hicolor/scalable/apps/io.github.easysearch.EasySearch.svg`
//! ships the same shapes for desktop launchers and software centres.
//!
//! Coordinates are written in that SVG's own 512-unit space (a rounded badge
//! with a magnifier over a lightning bolt) and normalised at raster time, so the
//! two files can be read side by side. Pixels are supersampled, so the mark is
//! smooth at any size and needs no image asset.

/// The rounded badge the mark sits on.
const BADGE: [u8; 3] = [0x23, 0x28, 0x42];
/// The magnifier — ring and handle — in the app's accent colour.
const RING: [u8; 3] = [0x7a, 0xa2, 0xf7];
/// The lightning bolt, standing for "instant".
const BOLT: [u8; 3] = [0xe0, 0xaf, 0x68];

/// The SVG's coordinate space, normalised away when sampling.
const U: f32 = 512.0;

/// Badge rectangle `(x, y, w, h)` and corner radius.
const BADGE_RECT: (f32, f32, f32, f32) = (16.0, 16.0, 480.0, 480.0);
const BADGE_RADIUS: f32 = 108.0;

/// Lens centre, outer and inner radius (the ring is the disc between them).
const LENS: (f32, f32) = (228.0, 220.0);
const LENS_OUTER: f32 = 128.0;
const LENS_INNER: f32 = 91.0;

/// Handle: a capsule along the 45° diagonal, `HANDLE_FROM`..`HANDLE_TO` from the
/// lens centre, `HANDLE_HALF` thick. Its inner end sits inside the ring, so the
/// round cap there is hidden and cannot be told from the SVG's flat one.
const HANDLE_FROM: f32 = 107.5;
const HANDLE_TO: f32 = 220.2;
const HANDLE_HALF: f32 = 28.7;

/// The bolt outline, relative to the lens centre.
const BOLT_SHAPE: [(f32, f32); 6] = [
    (0.0, -69.0),
    (-35.9, 11.0),
    (-7.0, 11.0),
    (-15.0, 69.0),
    (36.0, -14.0),
    (7.0, -14.0),
];

/// Samples per axis per output pixel (16 samples per pixel).
const SUPERSAMPLE: u32 = 4;

/// The mark as `size`×`size` RGBA8 with straight alpha, for
/// [`egui::IconData`](egui::IconData) or an [`egui::ColorImage`](egui::ColorImage).
pub fn rgba(size: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    let bolt = bolt_poly();
    let n = SUPERSAMPLE * SUPERSAMPLE;
    for py in 0..size {
        for px in 0..size {
            let (mut sr, mut sg, mut sb, mut covered) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..SUPERSAMPLE {
                for sx in 0..SUPERSAMPLE {
                    let x = (px as f32 + (sx as f32 + 0.5) / SUPERSAMPLE as f32) / size as f32 * U;
                    let y = (py as f32 + (sy as f32 + 0.5) / SUPERSAMPLE as f32) / size as f32 * U;
                    if let Some(c) = cover(x, y, &bolt) {
                        sr += c[0] as u32;
                        sg += c[1] as u32;
                        sb += c[2] as u32;
                        covered += 1;
                    }
                }
            }
            // Colour is averaged over the *covered* samples only, so an edge
            // pixel keeps the mark's colour and just becomes translucent
            // instead of fading towards black. Nothing covered draws nothing.
            let colour = [sr, sg, sb].map(|c| c.checked_div(covered).unwrap_or(0));
            out.extend_from_slice(&[
                colour[0] as u8,
                colour[1] as u8,
                colour[2] as u8,
                (covered * 255 / n) as u8,
            ]);
        }
    }
    out
}

/// The mark as `size`×`size` ARGB32 in network byte order, which is what a
/// StatusNotifierItem pixmap expects.
pub fn argb32(size: u32) -> Vec<u8> {
    let rgba = rgba(size);
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
    }
    out
}

/// The bolt outline in absolute coordinates.
fn bolt_poly() -> [(f32, f32); 6] {
    std::array::from_fn(|i| (LENS.0 + BOLT_SHAPE[i].0, LENS.1 + BOLT_SHAPE[i].1))
}

/// Which colour covers the point, if any. Painted front to back: the bolt sits
/// inside the lens, so it wins over the ring it is enclosed by.
fn cover(x: f32, y: f32, bolt: &[(f32, f32); 6]) -> Option<[u8; 3]> {
    if point_in_poly(x, y, bolt) {
        return Some(BOLT);
    }
    let (dx, dy) = (x - LENS.0, y - LENS.1);
    let r2 = dx * dx + dy * dy;
    // The hole is punched through everything, the handle's inner end included
    // (it reaches into the lens), so the magnifier only counts outside it.
    if r2 >= LENS_INNER * LENS_INNER {
        let band = (LENS_INNER * LENS_INNER..=LENS_OUTER * LENS_OUTER).contains(&r2);
        if band || inside_handle(x, y) {
            return Some(RING);
        }
    }
    if inside_badge(x, y) {
        return Some(BADGE);
    }
    None
}

/// The rounded badge.
fn inside_badge(x: f32, y: f32) -> bool {
    let (bx, by, bw, bh) = BADGE_RECT;
    if x < bx || y < by || x > bx + bw || y > by + bh {
        return false;
    }
    inside_rounded_rect(x, y, bx, by, bw, bh, BADGE_RADIUS)
}

/// The handle capsule: every point within [`HANDLE_HALF`] of the diagonal
/// segment (a round cap at each end).
fn inside_handle(x: f32, y: f32) -> bool {
    let d = std::f32::consts::FRAC_1_SQRT_2;
    let (sx, sy) = (LENS.0 + HANDLE_FROM * d, LENS.1 + HANDLE_FROM * d);
    // The segment runs from that point to `HANDLE_TO` out from the lens centre.
    let len = (HANDLE_TO - HANDLE_FROM) * d;
    let (vx, vy) = (len, len);
    let (wx, wy) = (x - sx, y - sy);
    let t = ((wx * vx + wy * vy) / (vx * vx + vy * vy)).clamp(0.0, 1.0);
    let (dx, dy) = (wx - t * vx, wy - t * vy);
    dx * dx + dy * dy <= HANDLE_HALF * HANDLE_HALF
}

/// The far/left/top corners of the badge, squared off to `r`.
fn inside_rounded_rect(x: f32, y: f32, rx: f32, ry: f32, w: f32, h: f32, r: f32) -> bool {
    let cx = x.clamp(rx + r, rx + w - r);
    let cy = y.clamp(ry + r, ry + h - r);
    let (dx, dy) = (x - cx, y - cy);
    dx * dx + dy * dy <= r * r
}

/// Ray-casting point-in-polygon test.
fn point_in_poly(x: f32, y: f32, poly: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The colour at the centre of a pixel of a `64`px mark.
    fn px(size: u32, x: u32, y: u32) -> [u8; 4] {
        let data = rgba(size);
        let i = ((y * size + x) * 4) as usize;
        [data[i], data[i + 1], data[i + 2], data[i + 3]]
    }

    #[test]
    fn layers_are_where_they_should_be() {
        // Straight down from the middle of the lens: bolt, then ring, then badge.
        let lens = LENS;
        let mid = (LENS_INNER + LENS_OUTER) / 2.0;
        assert_eq!(
            cover(lens.0, lens.1, &bolt_poly()),
            Some(BOLT),
            "lens centre"
        );
        assert_eq!(
            cover(lens.0, lens.1 - mid, &bolt_poly()),
            Some(RING),
            "ring band"
        );
        assert_eq!(cover(lens.0, 40.0, &bolt_poly()), Some(BADGE), "badge");
        assert_eq!(cover(2.0, 2.0, &bolt_poly()), None, "outside the badge");

        // The handle: its far end and a point along it.
        let d = std::f32::consts::FRAC_1_SQRT_2;
        let far = (LENS.0 + HANDLE_TO * d, LENS.1 + HANDLE_TO * d);
        assert_eq!(cover(far.0, far.1, &bolt_poly()), Some(RING), "handle end");
        let near = (LENS.0 + HANDLE_FROM * d, LENS.1 + HANDLE_FROM * d);
        assert_eq!(
            cover(near.0, near.1, &bolt_poly()),
            Some(RING),
            "handle start"
        );
        // Well to the side of the handle.
        assert_eq!(
            cover(near.0 + 90.0, near.1 - 90.0, &bolt_poly()),
            Some(BADGE)
        );
    }

    #[test]
    fn the_handle_stops_at_its_cap() {
        // Past the far cap the handle must be over and the badge show again.
        // Regression: the segment's direction was once measured from the lens
        // centre instead of from its own start, which drew the handle a third
        // longer, into the badge's bottom-right corner.
        let d = std::f32::consts::FRAC_1_SQRT_2;
        let past = HANDLE_TO + HANDLE_HALF + 8.0;
        let at = (LENS.0 + past * d, LENS.1 + past * d);
        assert_eq!(cover(at.0, at.1, &bolt_poly()), Some(BADGE), "past the cap");
    }

    #[test]
    fn the_lens_hole_is_punched_through_the_handle() {
        // The handle's inner cap reaches into the lens, and the hole is punched
        // through it: that sliver shows the badge, not the ring.
        let d = std::f32::consts::FRAC_1_SQRT_2;
        let (sx, sy) = (LENS.0 + HANDLE_FROM * d, LENS.1 + HANDLE_FROM * d);
        let at = (sx - 0.8 * HANDLE_HALF * d, sy - 0.8 * HANDLE_HALF * d);
        let r = ((at.0 - LENS.0).powi(2) + (at.1 - LENS.1).powi(2)).sqrt();
        assert!(r < LENS_INNER, "probe must be inside the hole (r={r})");
        assert!(inside_handle(at.0, at.1), "and inside the handle");
        assert_eq!(cover(at.0, at.1, &bolt_poly()), Some(BADGE));
    }

    #[test]
    fn pixels_are_opaque_inside_and_transparent_outside() {
        let size = 64;
        // The lens centre is solid.
        assert_eq!(px(size, 28, 27), [0xe0, 0xaf, 0x68, 255], "bolt centre");
        // A corner outside the rounded badge is fully transparent.
        assert_eq!(px(size, 1, 1)[3], 0, "corner is transparent");
        // The middle of the badge edge is solid and badge-coloured.
        assert_eq!(px(size, 32, 3), [0x23, 0x28, 0x42, 255], "top edge");
    }

    #[test]
    fn edges_are_antialiased_and_buffers_are_sized() {
        let size = 64;
        let data = rgba(size);
        assert_eq!(data.len(), (size * size * 4) as usize);
        // Somewhere along the badge's rounded corner there must be a partial
        // alpha — proof that the mark is supersampled rather than jagged.
        let alphas: Vec<u8> = data.iter().skip(3).step_by(4).copied().collect();
        assert!(
            alphas.iter().any(|&a| a > 0 && a < 255),
            "expected antialiased edge pixels"
        );

        // ARGB32 keeps the same pixel count and moves alpha to the front.
        let argb = argb32(size);
        assert_eq!(argb.len(), data.len());
        let i = ((27 * size + 28) * 4) as usize;
        assert_eq!(&argb[i..i + 4], &[255, 0xe0, 0xaf, 0x68]);
    }

    #[test]
    fn every_size_rasterises() {
        for size in [16_u32, 22, 32, 64, 128, 256] {
            let data = rgba(size);
            assert_eq!(data.len(), (size * size * 4) as usize, "size {size}");
            // Something is always drawn.
            assert!(
                data.iter().skip(3).step_by(4).any(|&a| a > 0),
                "size {size}"
            );
        }
    }
}
