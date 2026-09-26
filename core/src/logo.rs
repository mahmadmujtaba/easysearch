//! The EasySearch logo, compiled into the binary.
//!
//! The brand assets live in `icons/` at the root of the repository (4000×4000
//! exports of the wordmark). `assets/logo.png` is a 512×512 copy of the
//! **transparent** artwork, embedded here so that the window icon, the tray
//! pixmap and the in-app mark never depend on the icon theme being installed —
//! which matters for the single-file binary, and for X11, where the window icon
//! comes from the process rather than from the desktop entry. Being transparent,
//! it reads correctly on both the light and the dark theme.
//!
//! It lives in `core` because both frontends draw it: the GUI (`rgba`, for
//! `egui::IconData` and the About dialog) and the tray item (`argb32`).
//!
//! The same artwork is installed as
//! `packaging/icons/hicolor/scalable/apps/io.github.easysearch.EasySearch.svg`
//! for launchers and software centres.

use std::sync::OnceLock;

/// The artwork is square and transparent; 512 px is its native size and enough
/// for the window icon and for the dialog and welcome-screen renders.
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
///
/// The colour channels are premultiplied by alpha (the convention Qt/KDE and
/// most ARGB32 pixmap consumers use); with the transparent artwork a
/// straight-alpha buffer would show bright fringes around the mark.
pub fn argb32(size: u32) -> Vec<u8> {
    let rgba = rgba(size);
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        let a = u32::from(px[3]);
        out.push(px[3]);
        out.push((u32::from(px[0]) * a / 255) as u8);
        out.push((u32::from(px[1]) * a / 255) as u8);
        out.push((u32::from(px[2]) * a / 255) as u8);
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
        }
    }

    #[test]
    fn the_logo_is_transparent_not_a_coloured_tile() {
        // The asset is the transparent export, so it must carry a real alpha
        // channel: a fully opaque image here means the wrong file got embedded
        // (the coloured tile has a cream background and would look pasted on
        // the dark theme).
        let data = rgba(128);
        let alpha: Vec<u8> = data.iter().skip(3).step_by(4).copied().collect();
        assert!(alpha.contains(&0), "expected fully transparent pixels");
        assert!(alpha.contains(&255), "expected opaque pixels in the mark");
    }

    #[test]
    fn the_installed_icon_is_the_transparent_export() {
        // A launcher and the window decoration draw the icon from the packaging
        // tree, so it must be the *transparent* export: the cream tile reads as
        // a pasted-on white square on a dark panel. This is the guard that the
        // two copies cannot drift apart.
        const INSTALLED: &str = include_str!(
            "../../packaging/icons/hicolor/scalable/apps/io.github.easysearch.EasySearch.svg"
        );
        const TRANSPARENT: &str = include_str!("../../icons/transparent-logo.svg");
        let body = INSTALLED
            .split_once("-->\n")
            .map(|(_, rest)| rest)
            .expect("the installed icon carries the generated header comment");
        assert_eq!(
            body, TRANSPARENT,
            "the packaging icon must be icons/transparent-logo.svg (transparent), not the \
             cream `colored-logo.svg`"
        );
    }

    #[test]
    fn argb32_premultiplies_in_network_byte_order() {
        let rgba = rgba(32);
        let argb = argb32(32);
        assert_eq!(argb.len(), rgba.len());
        for (i, px) in rgba.chunks_exact(4).enumerate() {
            let a = u32::from(px[3]);
            let out = &argb[i * 4..i * 4 + 4];
            assert_eq!(u32::from(out[0]), a, "alpha first");
            for c in 0..3 {
                assert_eq!(
                    u32::from(out[1 + c]),
                    u32::from(px[c]) * a / 255,
                    "channel {c} is premultiplied"
                );
            }
        }
    }
}
