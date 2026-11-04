//! Bundled browser logos for Fleet cards. See assets/browser-logos/SOURCES.md.

use rune::Pixels;
use std::sync::OnceLock;

const PNGS: [&[u8]; 7] = [
    include_bytes!("../assets/browser-logos/brave.png"),
    include_bytes!("../assets/browser-logos/chrome.png"),
    include_bytes!("../assets/browser-logos/duckduckgo.png"),
    include_bytes!("../assets/browser-logos/edge.png"),
    include_bytes!("../assets/browser-logos/firefox.png"),
    include_bytes!("../assets/browser-logos/opera.png"),
    include_bytes!("../assets/browser-logos/safari.png"),
];

static PIXELS: OnceLock<[Option<Pixels>; 7]> = OnceLock::new();

fn decoded() -> &'static [Option<Pixels>; 7] {
    PIXELS.get_or_init(|| {
        std::array::from_fn(|index| {
            let image = image::load_from_memory_with_format(PNGS[index], image::ImageFormat::Png)
                .ok()?
                .to_rgba8();
            let image =
                image::imageops::resize(&image, 64, 64, image::imageops::FilterType::Lanczos3);
            let pixels = Pixels::new();
            pixels.set(64, 64, image.into_raw());
            Some(pixels)
        })
    })
}

pub(crate) fn for_name(name: &str) -> Option<&'static Pixels> {
    let index = match name.to_ascii_lowercase().as_str() {
        "brave" | "brave browser" => 0,
        "chrome" | "google chrome" => 1,
        "duckduckgo" | "duckduckgo browser" => 2,
        "edge" | "microsoft edge" => 3,
        "firefox" | "mozilla firefox" => 4,
        "opera" => 5,
        "safari" | "apple safari" => 6,
        _ => return None,
    };
    decoded()[index].as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_browsers_have_decoded_color_logos() {
        for name in [
            "Google Chrome",
            "Apple Safari",
            "Microsoft Edge",
            "Mozilla Firefox",
            "Opera",
            "Brave",
            "DuckDuckGo",
        ] {
            assert_eq!(for_name(name).map(Pixels::size), Some((64, 64)), "{name}");
        }
        assert!(for_name("Example Browser").is_none());
    }
}
