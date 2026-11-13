//! === BUILT-IN STYLE & ATMOSPHERE PRESETS ===
//!
//! PURPOSE:
//! Compiles the 5 built-in `Style` presets (with deterministic embedded minimal
//! PNG exemplars) and 4 built-in `Atmosphere` presets directly into `imgforge-core`
//! so every workspace has validated defaults out of the box.
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Built-in presets provide the lowest-precedence fallback tier in
//!    `project > user > builtin` resolution without requiring network access or
//!    external asset downloads.
//! 2. All preset text is strictly pure English, domain-agnostic, and free of
//!    active negation phrasing so `PromptCompiler` never rejects built-in presets.
//! 3. Each built-in `Style` embeds a deterministic valid PNG exemplar matching the
//!    `1..=3` exemplar contract enforced by `Style::validate`.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. `builtin_style(slug)` / `builtin_styles()` return validated `Style` cards.
//! 2. `builtin_style_exemplar_png(slug)` returns cached deterministic PNG bytes.
//! 3. `builtin_atmosphere(slug)` / `builtin_atmospheres()` return validated
//!    `Atmosphere` cards.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::OnceLock;

use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};

use crate::atmosphere::{Atmosphere, ExposureFloor};
use crate::lifecycle::Lifecycle;
use crate::style::Style;

/// Canonical slugs of the 5 built-in `Style` presets.
pub const BUILTIN_STYLE_SLUGS: &[&str] = &[
    "franco-belgian-line",
    "dark-fantasy-realistic",
    "manga-ink",
    "parchment-handout",
    "topdown-battlemap",
];

/// Canonical slugs of the 4 built-in `Atmosphere` presets.
pub const BUILTIN_ATMOSPHERE_SLUGS: &[&str] = &[
    "neutral-studio",
    "golden-dawn",
    "overcast-dusk",
    "torchlit-crypt",
];

struct BuiltinExemplarCache {
    pngs: BTreeMap<&'static str, Vec<u8>>,
}

static EXEMPLAR_CACHE: OnceLock<BuiltinExemplarCache> = OnceLock::new();

fn build_exemplar_png(base: [u8; 3], accent: [u8; 3], line: [u8; 3]) -> Vec<u8> {
    const SIZE: u32 = 64;
    let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(SIZE, SIZE);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let is_border = !(3..SIZE - 3).contains(&x) || !(3..SIZE - 3).contains(&y);
            let is_diagonal = (x as i32 - y as i32).abs() <= 1 && (10..54).contains(&x);
            let is_accent_box = (16..32).contains(&x) && (16..32).contains(&y);
            let rgb = if is_border || is_diagonal {
                line
            } else if is_accent_box {
                accent
            } else {
                base
            };
            img.put_pixel(x, y, Rgba([rgb[0], rgb[1], rgb[2], 255]));
        }
    }
    let mut out = Vec::new();
    DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .expect("encoding deterministic built-in exemplar PNG must succeed");
    out
}

fn exemplar_cache() -> &'static BuiltinExemplarCache {
    EXEMPLAR_CACHE.get_or_init(|| {
        let mut pngs = BTreeMap::new();
        pngs.insert(
            "franco-belgian-line",
            build_exemplar_png([236, 229, 212], [68, 122, 168], [28, 30, 36]),
        );
        pngs.insert(
            "dark-fantasy-realistic",
            build_exemplar_png([48, 44, 52], [176, 118, 62], [20, 18, 22]),
        );
        pngs.insert(
            "manga-ink",
            build_exemplar_png([246, 246, 246], [164, 164, 164], [14, 14, 14]),
        );
        pngs.insert(
            "parchment-handout",
            build_exemplar_png([228, 210, 174], [148, 108, 72], [72, 48, 30]),
        );
        pngs.insert(
            "topdown-battlemap",
            build_exemplar_png([122, 134, 110], [168, 150, 118], [46, 52, 44]),
        );
        BuiltinExemplarCache { pngs }
    })
}

/// Return the deterministic embedded PNG exemplar bytes for a built-in `Style` slug.
#[must_use]
pub fn builtin_style_exemplar_png(slug: &str) -> Option<&'static [u8]> {
    exemplar_cache().pngs.get(slug).map(Vec::as_slice)
}

/// Look up a single built-in `Style` preset by slug.
#[must_use]
pub fn builtin_style(slug: &str) -> Option<Style> {
    let style = match slug {
        "franco-belgian-line" => Style {
            schema_version: 1,
            slug: "franco-belgian-line".to_string(),
            display_name: "Franco-Belgian Clear Line".to_string(),
            medium: "pen-and-ink line illustration with flat matte gouache fills".to_string(),
            linework: "uniform closed black ink contours with steady crisp pen weight".to_string(),
            shading_and_texture: "clean two-tone cel shadows and smooth unmodulated flat color planes".to_string(),
            palette_profile: "balanced mid-saturation hues with distinct local color separation".to_string(),
            framing_and_border: "clean full-bleed panel framing with crisp subject silhouettes".to_string(),
            style_guardrails: vec![
                "maintain uniform contour ink thickness across foreground and background".to_string(),
                "preserve flat cel-shaded planes with clear local silhouette separation".to_string(),
            ],
            exemplars: vec![PathBuf::from("exemplars/exemplar_1.png")],
            lifecycle: Lifecycle::Canonical,
        },
        "dark-fantasy-realistic" => Style {
            schema_version: 1,
            slug: "dark-fantasy-realistic".to_string(),
            display_name: "Dark Fantasy Realistic".to_string(),
            medium: "traditional oil and opaque gouache brushwork on textured canvas".to_string(),
            linework: "painterly edge-defined forms shaped by value contrast and subtle rim strokes".to_string(),
            shading_and_texture: "rich chiaroscuro modeling with weathered leather, stone, and brushed metal grain".to_string(),
            palette_profile: "earthy muted midtones anchored by deep charcoal shadows and warm specular accents".to_string(),
            framing_and_border: "cinematic full-frame composition with atmospheric depth separation".to_string(),
            style_guardrails: vec![
                "build form through painterly tonal transitions and tactile material grain".to_string(),
                "preserve grounded realistic anatomy and authentic metallic reflections".to_string(),
            ],
            exemplars: vec![PathBuf::from("exemplars/exemplar_1.png")],
            lifecycle: Lifecycle::Canonical,
        },
        "manga-ink" => Style {
            schema_version: 1,
            slug: "manga-ink".to_string(),
            display_name: "Manga Ink & Screentone".to_string(),
            medium: "monochrome G-pen India ink with fine mechanical halftone screentones".to_string(),
            linework: "expressive tapered ink strokes with bold foreground contour hierarchy".to_string(),
            shading_and_texture: "solid black ink spotting paired with crisp dot-pattern screentone gradients".to_string(),
            palette_profile: "high-contrast neutral paper white, charcoal grays, and deep India ink black".to_string(),
            framing_and_border: "sharp full-frame graphic layout with decisive figure-ground contrast".to_string(),
            style_guardrails: vec![
                "render shadows through clean ink spotting and uniform mechanical halftone dots".to_string(),
                "keep line art crisp and monochrome across characters and environments".to_string(),
            ],
            exemplars: vec![PathBuf::from("exemplars/exemplar_1.png")],
            lifecycle: Lifecycle::Canonical,
        },
        "parchment-handout" => Style {
            schema_version: 1,
            slug: "parchment-handout".to_string(),
            display_name: "Parchment Handout".to_string(),
            medium: "sepia quill ink and diluted walnut wash on aged vellum parchment".to_string(),
            linework: "fine archival dip-pen contours with delicate parallel hatching and stipple".to_string(),
            shading_and_texture: "subtle cross-hatched tonal modeling over warm fibrous paper grain".to_string(),
            palette_profile: "warm sepia brown inks, burnt umber accents, and natural cream parchment tones".to_string(),
            framing_and_border: "editorial archival plate composition with generous paper breathing room".to_string(),
            style_guardrails: vec![
                "preserve hand-drawn dip-pen hatching and warm archival parchment substrate".to_string(),
                "keep washes translucent so fine ink contour hatching stays legible".to_string(),
            ],
            exemplars: vec![PathBuf::from("exemplars/exemplar_1.png")],
            lifecycle: Lifecycle::Canonical,
        },
        "topdown-battlemap" => Style {
            schema_version: 1,
            slug: "topdown-battlemap".to_string(),
            display_name: "Top-Down Cartographic Battlemap".to_string(),
            medium: "orthographic top-down painted cartography with crisp architectural detail".to_string(),
            linework: "clean overhead plan-view ink edges separating walkable floor tiles from obstacles".to_string(),
            shading_and_texture: "directional drop shadows indicating wall height over textured flagstone, timber, and foliage".to_string(),
            palette_profile: "readable low-contrast ground surfaces paired with distinct interactive prop hues".to_string(),
            framing_and_border: "strictly vertical 90-degree orthographic overhead projection aligned to square grid axes".to_string(),
            style_guardrails: vec![
                "maintain strict 90-degree overhead orthographic projection across all structures".to_string(),
                "keep floor surfaces readable with consistent directional elevation shadows".to_string(),
            ],
            exemplars: vec![PathBuf::from("exemplars/exemplar_1.png")],
            lifecycle: Lifecycle::Canonical,
        },
        _ => return None,
    };
    Some(style)
}

/// Return all 5 built-in `Style` presets in deterministic order.
#[must_use]
pub fn builtin_styles() -> Vec<Style> {
    BUILTIN_STYLE_SLUGS
        .iter()
        .filter_map(|slug| builtin_style(slug))
        .collect()
}

/// Look up a single built-in `Atmosphere` preset by slug.
#[must_use]
pub fn builtin_atmosphere(slug: &str) -> Option<Atmosphere> {
    let atmo = match slug {
        "neutral-studio" => Atmosphere {
            schema_version: 1,
            slug: "neutral-studio".to_string(),
            display_name: "Neutral Studio".to_string(),
            key_light_direction: "soft diffused overhead studio key at 45 degrees front-left"
                .to_string(),
            secondary_light_direction: "even neutral fill bounce from broad front-right reflector"
                .to_string(),
            color_temperature_k: 5500,
            max_ambient_tint_blend: 0.05,
            exposure_floor: ExposureFloor {
                min_mean_l_star: 35.0,
                min_p10_l_star: 12.0,
            },
            weather_and_particles: "clear studio air".to_string(),
        },
        "golden-dawn" => Atmosphere {
            schema_version: 1,
            slug: "golden-dawn".to_string(),
            display_name: "Golden Dawn".to_string(),
            key_light_direction: "low warm eastern sunrise key at 15 degrees horizon elevation"
                .to_string(),
            secondary_light_direction: "cool pale-blue sky fill from upper opposite dome"
                .to_string(),
            color_temperature_k: 3400,
            max_ambient_tint_blend: 0.25,
            exposure_floor: ExposureFloor {
                min_mean_l_star: 32.0,
                min_p10_l_star: 10.0,
            },
            weather_and_particles: "light drifting morning mist catching warm rim rays".to_string(),
        },
        "overcast-dusk" => Atmosphere {
            schema_version: 1,
            slug: "overcast-dusk".to_string(),
            display_name: "Overcast Dusk".to_string(),
            key_light_direction: "broad diffused slate-blue zenith sky canopy light".to_string(),
            secondary_light_direction: "muted cool horizon bounce along damp ground planes"
                .to_string(),
            color_temperature_k: 6800,
            max_ambient_tint_blend: 0.30,
            exposure_floor: ExposureFloor {
                min_mean_l_star: 26.0,
                min_p10_l_star: 8.0,
            },
            weather_and_particles: "fine coastal mist and damp air haze".to_string(),
        },
        "torchlit-crypt" => Atmosphere {
            schema_version: 1,
            slug: "torchlit-crypt".to_string(),
            display_name: "Torchlit Crypt".to_string(),
            key_light_direction: "warm flickering wall-sconce flame key from side-elevated angle"
                .to_string(),
            secondary_light_direction: "subtle cool stone ambient bounce filling shadowed recesses"
                .to_string(),
            color_temperature_k: 2400,
            max_ambient_tint_blend: 0.35,
            exposure_floor: ExposureFloor {
                min_mean_l_star: 22.0,
                min_p10_l_star: 6.0,
            },
            weather_and_particles: "faint drifting embers and suspended stone dust motes"
                .to_string(),
        },
        _ => return None,
    };
    Some(atmo)
}

/// Return all 4 built-in `Atmosphere` presets in deterministic order.
#[must_use]
pub fn builtin_atmospheres() -> Vec<Atmosphere> {
    BUILTIN_ATMOSPHERE_SLUGS
        .iter()
        .filter_map(|slug| builtin_atmosphere(slug))
        .collect()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::backend::PNG_SIGNATURE;

    #[rstest]
    #[case::franco_belgian_line("franco-belgian-line")]
    #[case::dark_fantasy_realistic("dark-fantasy-realistic")]
    #[case::manga_ink("manga-ink")]
    #[case::parchment_handout("parchment-handout")]
    #[case::topdown_battlemap("topdown-battlemap")]
    fn all_builtin_styles_validate_and_embed_valid_png_exemplars(#[case] slug: &str) {
        let style = builtin_style(slug).expect("builtin style must exist");
        style
            .validate()
            .expect("builtin style must pass validation");
        assert_eq!(style.slug, slug);
        assert_eq!(style.exemplars.len(), 1);

        let png = builtin_style_exemplar_png(slug).expect("embedded PNG exemplar must exist");
        assert!(png.starts_with(&PNG_SIGNATURE));
        let decoded = image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .expect("embedded exemplar must decode cleanly");
        assert!(decoded.width() >= 32 && decoded.height() >= 32);
    }

    #[rstest]
    #[case::neutral_studio("neutral-studio")]
    #[case::golden_dawn("golden-dawn")]
    #[case::overcast_dusk("overcast-dusk")]
    #[case::torchlit_crypt("torchlit-crypt")]
    fn all_builtin_atmospheres_validate(#[case] slug: &str) {
        let atmo = builtin_atmosphere(slug).expect("builtin atmosphere must exist");
        atmo.validate()
            .expect("builtin atmosphere must pass validation");
        assert_eq!(atmo.slug, slug);
    }
}
