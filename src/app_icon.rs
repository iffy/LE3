//! Application window icon loaded from the bundled PNG asset.

use egui::IconData;
use std::sync::Arc;

#[cfg(any(not(target_os = "macos"), test))]
const APP_ICON_PNG: &[u8] = include_bytes!("assets/appicon.png");

/// Window icons only need a modest raster; keep the bundled 1024px asset for packaging.
#[cfg(any(not(target_os = "macos"), test))]
const WINDOW_ICON_SIZE: u32 = 128;

/// Icon for the eframe/winit viewport (taskbar / window chrome).
///
/// macOS uses the `.app` bundle `AppIcon.icns` instead. eframe substitutes its own
/// default PNG when `viewport.icon` is unset, which still crashes in
/// `NSImage::initWithData` (ImageIO SIGBUS). `IconData::default()` is the sentinel
/// eframe uses to skip runtime dock-icon installation entirely.
pub fn load_for_viewport() -> Arc<IconData> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(IconData::default())
    }
    #[cfg(not(target_os = "macos"))]
    {
        load_rgba_icon()
    }
}

#[cfg(any(not(target_os = "macos"), test))]
fn load_rgba_icon() -> Arc<IconData> {
    match image::load_from_memory(APP_ICON_PNG) {
        Ok(image) => {
            let rgba = image::imageops::resize(
                &image.to_rgba8(),
                WINDOW_ICON_SIZE,
                WINDOW_ICON_SIZE,
                image::imageops::FilterType::Lanczos3,
            );
            let (width, height) = rgba.dimensions();
            Arc::new(IconData {
                rgba: rgba.into_raw(),
                width,
                height,
            })
        }
        Err(_) => Arc::new(IconData::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_icon_png_decodes_to_square_image() {
        let png = include_bytes!("assets/appicon.png");
        let image = image::load_from_memory(png).expect("appicon.png should decode");
        assert!(image.width() >= 256);
        assert!(image.height() >= 256);
        assert_eq!(image.width(), image.height());
    }

    #[test]
    fn load_rgba_icon_produces_window_sized_icon_data() {
        let icon = load_rgba_icon();
        assert!(!icon.rgba.is_empty());
        assert_eq!(icon.width, WINDOW_ICON_SIZE);
        assert_eq!(icon.height, WINDOW_ICON_SIZE);
    }

    #[test]
    fn viewport_icon_policy_matches_platform() {
        let icon = load_for_viewport();
        #[cfg(target_os = "macos")]
        {
            assert_eq!(icon.width, 0);
            assert!(icon.rgba.is_empty());
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(icon.width, WINDOW_ICON_SIZE);
            assert!(!icon.rgba.is_empty());
        }
    }
}