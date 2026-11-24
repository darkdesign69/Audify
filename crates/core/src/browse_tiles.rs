//! Checks on `crates/core/browse_tiles.json`, the Browse tile colours both apps draw (the Mac
//! imports the file, the iPhone mirrors its hexes). Nothing at runtime reads it from Rust.

const JSON: &str = include_str!("../browse_tiles.json");

fn hexes() -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(JSON).expect("browse_tiles.json parses");
    let fixed = v["fixed"].as_object().expect("fixed").values();
    let palette = v["palette"].as_array().expect("palette").iter();
    fixed.chain(palette).map(|c| c.as_str().expect("hex string").to_lowercase()).collect()
}

/// WCAG contrast of white text on `#rrggbb`.
fn white_contrast(hex: &str) -> f64 {
    let ch = |i: usize| {
        let c = u8::from_str_radix(&hex[i..i + 2], 16).unwrap() as f64 / 255.0;
        if c <= 0.039_28 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    let l = 0.2126 * ch(1) + 0.7152 * ch(3) + 0.0722 * ch(5);
    1.05 / (l + 0.05)
}

#[test]
fn tiles_are_ours_and_readable() {
    // Spotify's own Browse colours, which the old palette copied.
    const SPOTIFY: [&str; 20] = [
        "#e13300", "#006450", "#8400e7", "#1e3264", "#e8115b", "#27856a", "#477d95", "#ba5d07",
        "#509bf5", "#8d67ab", "#af2896", "#148a08", "#503750", "#e91429", "#0d73ec", "#777777",
        "#dc148c", "#bc5900", "#e1118c", "#b0461e",
    ];
    let all = hexes();
    assert!(all.len() >= 17);
    for h in &all {
        assert!(h.len() == 7 && h.starts_with('#'), "{h} is #rrggbb");
        assert!(!SPOTIFY.contains(&h.as_str()), "{h} is Spotify's");
        assert!(white_contrast(h) >= 4.5, "white on {h} is {:.2}:1", white_contrast(h));
    }
}

#[test]
fn category_tiles_use_the_palette() {
    let all = hexes();
    let core = crate::smart::podhome::CATEGORIES.iter().chain(crate::audiobooks::ia::GENRES.iter());
    for (_, name, color) in core {
        assert!(all.contains(&color.to_string()), "{name}: {color} is not in browse_tiles.json");
    }
}
