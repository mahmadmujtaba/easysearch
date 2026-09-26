//! The EasySearch logo, compiled into the binary.
//!
//! The brand assets live in `icons/` at the root of the repository (4000×4000
//! exports of the wordmark). `assets/logo.png` is a 512×512 copy of the coloured
//! tile, embedded here so that the window icon, the tray pixmap and the in-app
//! mark never depend on the icon theme being installed — which matters for the
//! single-file binary, and for X11, where the window icon comes from the process
//! rather than from the desktop entry.
//!
//! The same artwork is installed as
//! `packaging/icons/hicolor/scalable/apps/io.github.easysearch.EasySearch.svg`
//! for launchers and software centres.

use std::sync::OnceLock;

/// The tile is opaque and square; 512 px is its native size and enough for the
/// window icon and for the dialog and welcome-screen renders.
const LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");

/// The decoded logo. Decoding once keeps the tray refresh and the UI off the
/// PNG decoder; a corrupt asset is a build-time mistake, caught by the tests.
fn logo() -> &'static image::DynamicImage {
    static LOGO: OnceLock<image::DynamicImage> = OnceLock::new();
    LOGO.get_or_init(|| image::load_from_memory(LOGO_PNG).expect("assets/logo.png is a valid PNG"))
}

/// The logo at `size`×`size`, RGBA8 with straight alpha — ready for
/// [`egui::IconData`](egui::IconData) or an [`egui::ColorImage`](egui::ColorImage).
pub fn rgba(size: u32) -> Vec<u8> {
    let src = logo();
    let scaled = if src.width() == size && src.height() == size {
        None
    } else {
        Some(src.resize_exact(size, size, image::imageops::FilterType::Lanczos3))
    };
    scaled.as_ref().unwrap_or(src).to_rgba8().into_raw()
}

/// The same image as ARGB32 in network byte order, which is what a
/// StatusNotifierItem pixmap expects.
pub fn argb32(size: u32) -> Vec<u8> {
    let rgba = rgba(size);
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_logo_decodes_and_is_square() {
        let src = logo();
        assert_eq!(src.width(), src.height(), "the mark is a square tile");
        assert!(src.width() >= 256, "enough resolution to stay sharp");
    }

    #[test]
    fn buffers_are_the_right_size() {
        for size in [16_u32, 22, 64, 128, 256, 512] {
            let data = rgba(size);
            assert_eq!(data.len(), (size * size * 4) as usize, "size {size}");
            // The exported tile is opaque, and resampling must not invent
            // transparency at the edges.
            assert!(
                data.iter().skip(3).step_by(4).all(|&a| a == 255),
                "size {size} lost opacity"
            );
        }
    }

    #[test]
    fn argb32_moves_alpha_to_the_front() {
        let rgba = rgba(32);
        let argb = argb32(32);
        assert_eq!(argb.len(), rgba.len());
        assert_eq!(&argb[0..1], &rgba[3..4], "alpha first");
        assert_eq!(&argb[1..4], &rgba[0..3], "then the colour");
    }
}
