//! === LIBRARY REGISTRY & PRECEDENCE RESOLVER ===
//!
//! PURPOSE:
//! Resolves `Subject`, `Style` (plus verified `1..=3` PNG exemplars), and
//! `Atmosphere` cards across the strict precedence chain:
//! `project` (`<workspace>/library/...`) > `user` (`$XDG_CONFIG_HOME/imgforge/...`
//! when non-empty & absolute, else `$HOME/.config/imgforge/...`) > `builtin`.
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Keeping filesystem discovery and symlink checks in `registry.rs` isolates
//!    pure domain validation (`subject.rs`, `style.rs`, `atmosphere.rs`) from I/O.
//! 2. If a higher-priority tier contains a card for `<slug>` that is malformed,
//!    mismatched in slug, or references a symlinked/escaping exemplar PNG, the
//!    resolver fails closed rather than silently falling back to a lower tier.
//! 3. Style directories and exemplar PNGs are verified via `symlink_metadata`,
//!    `O_NOFOLLOW`, canonical containment, and 8-byte PNG signature validation.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. Resolve `user` config root from `$XDG_CONFIG_HOME` / `$HOME`.
//! 2. Probe `project` -> `user` -> `builtin` for `<kind>/<slug>/card.yaml`.
//! 3. Validate card schema, slug consistency, and (for `Style`) exemplar PNGs.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atmosphere::Atmosphere;
use crate::backend::PNG_SIGNATURE;
use crate::config::ResolvedWorkspace;
use crate::error::CoreError;
use crate::lifecycle::{read_regular_file_nofollow, validate_slug};
use crate::presets::{
    builtin_atmosphere, builtin_atmospheres, builtin_style, builtin_style_exemplar_png,
    builtin_styles,
};
use crate::style::Style;
use crate::subject::Subject;

/// Canonical filename for library entity cards (`Subject`, `Style`, `Atmosphere`).
pub const CARD_FILENAME: &str = "card.yaml";

/// Resolution tier from which a library entity was loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibrarySource {
    /// Loaded from `<workspace>/library/...`.
    Project,
    /// Loaded from `$XDG_CONFIG_HOME/imgforge/...` or `$HOME/.config/imgforge/...`.
    User,
    /// Loaded from compiled-in `imgforge-core` defaults.
    Builtin,
}

impl LibrarySource {
    /// Canonical lowercase tier name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::User => "user",
            Self::Builtin => "builtin",
        }
    }
}

/// Resolved `Subject` paired with its precedence source and base directory.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedSubject {
    /// Validated `Subject` card.
    pub subject: Subject,
    /// Tier from which this `Subject` was resolved (`project` or `user`).
    pub source: LibrarySource,
    /// Directory containing `card.yaml` and optional `packs/`.
    pub dir: PathBuf,
}

/// Resolved `Style` paired with its precedence source and verified PNG exemplars.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedStyle {
    /// Validated `Style` card.
    pub style: Style,
    /// Tier from which this `Style` was resolved (`project`, `user`, or `builtin`).
    pub source: LibrarySource,
    /// Directory containing `card.yaml` on disk (`None` for compiled-in `Builtin`).
    pub dir: Option<PathBuf>,
    /// Verified PNG bytes for each exemplar in `style.exemplars`.
    pub exemplar_pngs: Vec<(PathBuf, Vec<u8>)>,
}

impl ResolvedStyle {
    /// Ensure all exemplar PNGs exist as regular files on disk inside `cache_dir` (for `Builtin`)
    /// or return their verified filesystem paths (for `Project`/`User`).
    pub fn materialize_exemplars(
        &self,
        builtin_cache_dir: &Path,
    ) -> Result<Vec<PathBuf>, CoreError> {
        if let Some(ref base_dir) = self.dir {
            let mut paths = Vec::with_capacity(self.style.exemplars.len());
            for rel in &self.style.exemplars {
                paths.push(base_dir.join(rel));
            }
            return Ok(paths);
        }

        let style_cache = builtin_cache_dir.join(&self.style.slug);
        fs::create_dir_all(&style_cache).map_err(|err| CoreError::Io {
            context: format!(
                "creating built-in style exemplar directory {}",
                style_cache.display()
            ),
            source: err,
        })?;

        let mut paths = Vec::with_capacity(self.exemplar_pngs.len());
        for (rel, bytes) in &self.exemplar_pngs {
            let file_name = rel
                .file_name()
                .ok_or_else(|| CoreError::StyleInvalid("missing exemplar filename".to_string()))?;
            let target = style_cache.join(file_name);
            fs::write(&target, bytes).map_err(|err| CoreError::Io {
                context: format!("writing built-in exemplar PNG {}", target.display()),
                source: err,
            })?;
            paths.push(target);
        }
        Ok(paths)
    }
}

/// Resolved `Atmosphere` paired with its precedence source.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedAtmosphere {
    /// Validated `Atmosphere` card.
    pub atmosphere: Atmosphere,
    /// Tier from which this `Atmosphere` was resolved (`project`, `user`, or `builtin`).
    pub source: LibrarySource,
    /// Directory containing `card.yaml` on disk (`None` for compiled-in `Builtin`).
    pub dir: Option<PathBuf>,
}

/// Resolve the user-level `imgforge` config directory:
/// - `$XDG_CONFIG_HOME/imgforge` ONLY when `$XDG_CONFIG_HOME` is non-empty and absolute;
/// - else `$HOME/.config/imgforge` when `$HOME` is non-empty and absolute;
/// - else `None`.
#[must_use]
pub fn resolve_user_config_dir(
    xdg_config_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Option<PathBuf> {
    if let Some(raw_xdg) = xdg_config_home
        && !raw_xdg.is_empty()
    {
        let xdg_path = Path::new(raw_xdg);
        if xdg_path.is_absolute() {
            return Some(xdg_path.join("imgforge"));
        }
    }

    if let Some(raw_home) = home
        && !raw_home.is_empty()
    {
        let home_path = Path::new(raw_home);
        if home_path.is_absolute() {
            return Some(home_path.join(".config").join("imgforge"));
        }
    }

    None
}

/// Resolve the user-level `imgforge` config directory from process environment variables.
#[must_use]
pub fn resolve_user_config_dir_from_env() -> Option<PathBuf> {
    let xdg = std::env::var_os("XDG_CONFIG_HOME");
    let home = std::env::var_os("HOME");
    resolve_user_config_dir(xdg.as_deref(), home.as_deref())
}

/// Precedence-aware library resolver (`project > user > builtin`).
#[derive(Debug, Clone)]
pub struct LibraryRegistry {
    project_library_dir: Option<PathBuf>,
    user_config_dir: Option<PathBuf>,
}

impl LibraryRegistry {
    /// Construct a `LibraryRegistry` with explicit project and user roots (deterministic for tests).
    #[must_use]
    pub fn new(project_library_dir: Option<PathBuf>, user_config_dir: Option<PathBuf>) -> Self {
        Self {
            project_library_dir,
            user_config_dir,
        }
    }

    /// Construct a `LibraryRegistry` bound to a resolved workspace and host user config dir.
    #[must_use]
    pub fn from_workspace(ws: &ResolvedWorkspace) -> Self {
        Self {
            project_library_dir: Some(ws.root.join(&ws.config.paths.library_dir)),
            user_config_dir: resolve_user_config_dir_from_env(),
        }
    }

    /// Return the configured project library directory, if any.
    #[must_use]
    pub fn project_library_dir(&self) -> Option<&Path> {
        self.project_library_dir.as_deref()
    }

    /// Return the configured user config directory, if any.
    #[must_use]
    pub fn user_config_dir(&self) -> Option<&Path> {
        self.user_config_dir.as_deref()
    }

    /// Resolve a `Subject` across `project > user` (fail-closed if missing or invalid).
    pub fn resolve_subject(&self, slug: &str) -> Result<ResolvedSubject, CoreError> {
        validate_slug(slug, "Subject").map_err(CoreError::SubjectInvalid)?;

        for (source, root_opt) in [
            (LibrarySource::Project, self.project_library_dir.as_deref()),
            (LibrarySource::User, self.user_config_dir.as_deref()),
        ] {
            let Some(root) = root_opt else { continue };
            let dir = root.join("subjects").join(slug);
            let card_path = dir.join(CARD_FILENAME);
            if let Some(raw) = read_optional_card_yaml(&dir, &card_path, "Subject")? {
                let subject = Subject::from_yaml_str(&raw)?;
                if subject.slug != slug {
                    return Err(CoreError::SubjectInvalid(format!(
                        "Subject card at {} declares slug '{}' (expected directory slug '{slug}')",
                        card_path.display(),
                        subject.slug
                    )));
                }
                return Ok(ResolvedSubject {
                    subject,
                    source,
                    dir,
                });
            }
        }

        Err(CoreError::RegistryResolutionFailed {
            kind: "Subject",
            slug: slug.to_string(),
            reason: "no card.yaml found in project or user library".to_string(),
        })
    }

    /// Resolve a `Style` across `project > user > builtin`, verifying `1..=3` PNG exemplars.
    pub fn resolve_style(&self, slug: &str) -> Result<ResolvedStyle, CoreError> {
        validate_slug(slug, "Style").map_err(CoreError::StyleInvalid)?;

        for (source, root_opt) in [
            (LibrarySource::Project, self.project_library_dir.as_deref()),
            (LibrarySource::User, self.user_config_dir.as_deref()),
        ] {
            let Some(root) = root_opt else { continue };
            let dir = root.join("styles").join(slug);
            let card_path = dir.join(CARD_FILENAME);
            if let Some(raw) = read_optional_card_yaml(&dir, &card_path, "Style")? {
                let style = Style::from_yaml_str(&raw)?;
                if style.slug != slug {
                    return Err(CoreError::StyleInvalid(format!(
                        "Style card at {} declares slug '{}' (expected directory slug '{slug}')",
                        card_path.display(),
                        style.slug
                    )));
                }
                let exemplar_pngs = load_and_verify_style_exemplars(&dir, &style)?;
                return Ok(ResolvedStyle {
                    style,
                    source,
                    dir: Some(dir),
                    exemplar_pngs,
                });
            }
        }

        if let Some(style) = builtin_style(slug) {
            let png = builtin_style_exemplar_png(slug)
                .ok_or_else(|| CoreError::StyleInvalid(format!("missing exemplar for '{slug}'")))?
                .to_vec();
            let rel = style.exemplars[0].clone();
            return Ok(ResolvedStyle {
                style,
                source: LibrarySource::Builtin,
                dir: None,
                exemplar_pngs: vec![(rel, png)],
            });
        }

        Err(CoreError::RegistryResolutionFailed {
            kind: "Style",
            slug: slug.to_string(),
            reason: "no card.yaml found in project, user, or built-in presets".to_string(),
        })
    }

    /// Resolve an `Atmosphere` across `project > user > builtin`.
    pub fn resolve_atmosphere(&self, slug: &str) -> Result<ResolvedAtmosphere, CoreError> {
        validate_slug(slug, "Atmosphere").map_err(CoreError::AtmosphereInvalid)?;

        for (source, root_opt) in [
            (LibrarySource::Project, self.project_library_dir.as_deref()),
            (LibrarySource::User, self.user_config_dir.as_deref()),
        ] {
            let Some(root) = root_opt else { continue };
            let dir = root.join("atmospheres").join(slug);
            let card_path = dir.join(CARD_FILENAME);
            if let Some(raw) = read_optional_card_yaml(&dir, &card_path, "Atmosphere")? {
                let atmosphere = Atmosphere::from_yaml_str(&raw)?;
                if atmosphere.slug != slug {
                    return Err(CoreError::AtmosphereInvalid(format!(
                        "Atmosphere card at {} declares slug '{}' (expected directory slug '{slug}')",
                        card_path.display(),
                        atmosphere.slug
                    )));
                }
                return Ok(ResolvedAtmosphere {
                    atmosphere,
                    source,
                    dir: Some(dir),
                });
            }
        }

        if let Some(atmosphere) = builtin_atmosphere(slug) {
            return Ok(ResolvedAtmosphere {
                atmosphere,
                source: LibrarySource::Builtin,
                dir: None,
            });
        }

        Err(CoreError::RegistryResolutionFailed {
            kind: "Atmosphere",
            slug: slug.to_string(),
            reason: "no card.yaml found in project, user, or built-in presets".to_string(),
        })
    }

    /// List all effective `Subject` entries resolved across `project > user` tiers.
    pub fn list_subjects(&self) -> Result<Vec<ResolvedSubject>, CoreError> {
        let mut by_slug: BTreeMap<String, ResolvedSubject> = BTreeMap::new();
        for root_opt in [
            self.user_config_dir.as_deref(),
            self.project_library_dir.as_deref(),
        ] {
            let Some(root) = root_opt else { continue };
            for slug in collect_entity_slugs(&root.join("subjects"), "Subject")? {
                let resolved = self.resolve_subject(&slug)?;
                by_slug.insert(slug, resolved);
            }
        }
        Ok(by_slug.into_values().collect())
    }

    /// List all effective `Style` entries resolved across `project > user > builtin` tiers.
    pub fn list_styles(&self) -> Result<Vec<ResolvedStyle>, CoreError> {
        let mut slugs: BTreeMap<String, ()> = BTreeMap::new();
        for style in builtin_styles() {
            slugs.insert(style.slug, ());
        }
        for root_opt in [
            self.user_config_dir.as_deref(),
            self.project_library_dir.as_deref(),
        ] {
            let Some(root) = root_opt else { continue };
            for slug in collect_entity_slugs(&root.join("styles"), "Style")? {
                slugs.insert(slug, ());
            }
        }

        let mut out = Vec::with_capacity(slugs.len());
        for slug in slugs.keys() {
            out.push(self.resolve_style(slug)?);
        }
        Ok(out)
    }

    /// List all effective `Atmosphere` entries resolved across `project > user > builtin` tiers.
    pub fn list_atmospheres(&self) -> Result<Vec<ResolvedAtmosphere>, CoreError> {
        let mut slugs: BTreeMap<String, ()> = BTreeMap::new();
        for atmo in builtin_atmospheres() {
            slugs.insert(atmo.slug, ());
        }
        for root_opt in [
            self.user_config_dir.as_deref(),
            self.project_library_dir.as_deref(),
        ] {
            let Some(root) = root_opt else { continue };
            for slug in collect_entity_slugs(&root.join("atmospheres"), "Atmosphere")? {
                slugs.insert(slug, ());
            }
        }

        let mut out = Vec::with_capacity(slugs.len());
        for slug in slugs.keys() {
            out.push(self.resolve_atmosphere(slug)?);
        }
        Ok(out)
    }
}

fn collect_entity_slugs(parent: &Path, kind: &'static str) -> Result<Vec<String>, CoreError> {
    match fs::symlink_metadata(parent) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(CoreError::Io {
                    context: format!("inspecting {kind} directory {}", parent.display()),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("{kind} container directory must not be a symlink"),
                    ),
                });
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(CoreError::Io {
                context: format!("inspecting {kind} directory {}", parent.display()),
                source: err,
            });
        }
    }

    let entries = fs::read_dir(parent).map_err(|err| CoreError::Io {
        context: format!("listing {kind} directory {}", parent.display()),
        source: err,
    })?;

    let mut slugs = Vec::new();
    for entry_res in entries {
        let entry = entry_res.map_err(|err| CoreError::Io {
            context: format!("reading entry in {}", parent.display()),
            source: err,
        })?;
        let path = entry.path();
        if path.join(CARD_FILENAME).exists()
            && let Some(name) = entry.file_name().to_str()
        {
            slugs.push(name.to_string());
        }
    }
    slugs.sort();
    Ok(slugs)
}

fn read_optional_card_yaml(
    entity_dir: &Path,
    card_path: &Path,
    kind: &'static str,
) -> Result<Option<String>, CoreError> {
    match fs::symlink_metadata(entity_dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(CoreError::Io {
                    context: format!("inspecting {kind} directory {}", entity_dir.display()),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("refusing symlink or non-directory {kind} directory"),
                    ),
                });
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(CoreError::Io {
                context: format!("inspecting {kind} directory {}", entity_dir.display()),
                source: err,
            });
        }
    }

    match fs::symlink_metadata(card_path) {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(CoreError::Io {
                context: format!("inspecting {kind} card {}", card_path.display()),
                source: err,
            });
        }
    }

    let bytes = read_regular_file_nofollow(card_path, kind)?;
    let raw = String::from_utf8(bytes).map_err(|err| CoreError::Io {
        context: format!("decoding UTF-8 {kind} card {}", card_path.display()),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, err),
    })?;
    Ok(Some(raw))
}

fn load_and_verify_style_exemplars(
    style_dir: &Path,
    style: &Style,
) -> Result<Vec<(PathBuf, Vec<u8>)>, CoreError> {
    let canonical_style_dir = fs::canonicalize(style_dir).map_err(|err| CoreError::Io {
        context: format!("canonicalizing Style directory {}", style_dir.display()),
        source: err,
    })?;

    let mut loaded = Vec::with_capacity(style.exemplars.len());
    for rel in &style.exemplars {
        // Walk every intermediate component inside style_dir to block symlinked subdirs.
        let mut cursor = style_dir.to_path_buf();
        let components: Vec<_> = rel.components().collect();
        for (idx, comp) in components.iter().enumerate() {
            cursor.push(comp.as_os_str());
            let meta = fs::symlink_metadata(&cursor).map_err(|err| {
                CoreError::StyleInvalid(format!(
                    "missing or unreadable Style exemplar component {}: {err}",
                    cursor.display()
                ))
            })?;
            if meta.file_type().is_symlink() {
                return Err(CoreError::StyleInvalid(format!(
                    "refusing symlink in Style exemplar path: {}",
                    cursor.display()
                )));
            }
            let is_last = idx + 1 == components.len();
            if is_last && !meta.is_file() {
                return Err(CoreError::StyleInvalid(format!(
                    "Style exemplar '{}' is not a regular file",
                    cursor.display()
                )));
            } else if !is_last && !meta.is_dir() {
                return Err(CoreError::StyleInvalid(format!(
                    "Style exemplar parent '{}' is not a directory",
                    cursor.display()
                )));
            }
        }

        let canonical_file = fs::canonicalize(&cursor).map_err(|err| CoreError::Io {
            context: format!("canonicalizing Style exemplar {}", cursor.display()),
            source: err,
        })?;
        if !canonical_file.starts_with(&canonical_style_dir) {
            return Err(CoreError::StyleInvalid(format!(
                "Style exemplar '{}' escapes style directory",
                rel.display()
            )));
        }

        let bytes = read_regular_file_nofollow(&cursor, "Style exemplar PNG")?;
        if bytes.len() < PNG_SIGNATURE.len() || bytes[..PNG_SIGNATURE.len()] != PNG_SIGNATURE {
            return Err(CoreError::InvalidPng {
                path: cursor,
                reason: "Style exemplar is missing valid 8-byte PNG signature".to_string(),
            });
        }
        image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).map_err(|err| {
            CoreError::InvalidPng {
                path: cursor,
                reason: format!("Style exemplar PNG decode failed: {err}"),
            }
        })?;

        loaded.push((rel.clone(), bytes));
    }

    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;

    fn write_tiny_png(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(16, 16, Rgba([20, 40, 60, 255]));
        let mut buf = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        fs::write(path, buf).unwrap();
    }

    fn write_style_dir(root: &Path, slug: &str, display_name: &str) {
        let dir = root.join("styles").join(slug);
        fs::create_dir_all(dir.join("exemplars")).unwrap();
        write_tiny_png(&dir.join("exemplars/ex1.png"));
        let yaml = format!(
            r#"
schema_version: 1
slug: {slug}
display_name: {display_name}
medium: digital ink
linework: crisp contours
shading_and_texture: flat shading
palette_profile: balanced hues
framing_and_border: clean margin
style_guardrails:
  - keep crisp lines
exemplars:
  - exemplars/ex1.png
"#
        );
        fs::write(dir.join("card.yaml"), yaml).unwrap();
    }

    fn write_atmosphere_dir(root: &Path, slug: &str, display_name: &str) {
        let dir = root.join("atmospheres").join(slug);
        fs::create_dir_all(&dir).unwrap();
        let yaml = format!(
            r#"
schema_version: 1
slug: {slug}
display_name: {display_name}
key_light_direction: overhead studio key
secondary_light_direction: soft fill
color_temperature_k: 5200
max_ambient_tint_blend: 0.1
exposure_floor:
  min_mean_l_star: 30.0
  min_p10_l_star: 10.0
weather_and_particles: clear air
"#
        );
        fs::write(dir.join("card.yaml"), yaml).unwrap();
    }

    #[rstest]
    #[case::absolute_xdg(Some("/tmp/xdg-cfg"), Some("/home/u"), Some("/tmp/xdg-cfg/imgforge"))]
    #[case::empty_xdg_falls_back(Some(""), Some("/home/u"), Some("/home/u/.config/imgforge"))]
    #[case::relative_xdg_falls_back(
        Some("rel/xdg"),
        Some("/home/u"),
        Some("/home/u/.config/imgforge")
    )]
    #[case::unset_xdg_uses_home(None, Some("/home/u"), Some("/home/u/.config/imgforge"))]
    #[case::both_relative_returns_none(Some("rel"), Some("rel"), None)]
    #[case::both_unset_returns_none(None, None, None)]
    fn resolves_user_config_dir_safely(
        #[case] xdg: Option<&str>,
        #[case] home: Option<&str>,
        #[case] expected: Option<&str>,
    ) {
        let actual = resolve_user_config_dir(xdg.map(OsStr::new), home.map(OsStr::new));
        assert_eq!(actual, expected.map(PathBuf::from));
    }

    #[test]
    fn enforces_project_over_user_over_builtin_precedence() {
        let root = TempDir::new().expect("tempdir");
        let project_lib = root.path().join("project/library");
        let user_cfg = root.path().join("user/imgforge");

        // 1. Built-in only
        let reg_builtin = LibraryRegistry::new(Some(project_lib.clone()), Some(user_cfg.clone()));
        let s_builtin = reg_builtin.resolve_style("franco-belgian-line").unwrap();
        assert_eq!(s_builtin.source, LibrarySource::Builtin);

        let a_builtin = reg_builtin.resolve_atmosphere("golden-dawn").unwrap();
        assert_eq!(a_builtin.source, LibrarySource::Builtin);

        // 2. User overrides Built-in
        write_style_dir(&user_cfg, "franco-belgian-line", "User Franco-Belgian");
        write_atmosphere_dir(&user_cfg, "golden-dawn", "User Golden Dawn");
        let s_user = reg_builtin.resolve_style("franco-belgian-line").unwrap();
        assert_eq!(s_user.source, LibrarySource::User);
        assert_eq!(s_user.style.display_name, "User Franco-Belgian");

        let a_user = reg_builtin.resolve_atmosphere("golden-dawn").unwrap();
        assert_eq!(a_user.source, LibrarySource::User);
        assert_eq!(a_user.atmosphere.display_name, "User Golden Dawn");

        // 3. Project overrides User and Built-in
        write_style_dir(
            &project_lib,
            "franco-belgian-line",
            "Project Franco-Belgian",
        );
        write_atmosphere_dir(&project_lib, "golden-dawn", "Project Golden Dawn");
        let s_proj = reg_builtin.resolve_style("franco-belgian-line").unwrap();
        assert_eq!(s_proj.source, LibrarySource::Project);
        assert_eq!(s_proj.style.display_name, "Project Franco-Belgian");

        let a_proj = reg_builtin.resolve_atmosphere("golden-dawn").unwrap();
        assert_eq!(a_proj.source, LibrarySource::Project);
        assert_eq!(a_proj.atmosphere.display_name, "Project Golden Dawn");
    }

    #[test]
    fn fails_closed_when_project_override_is_malformed_without_falling_back() {
        let root = TempDir::new().expect("tempdir");
        let project_lib = root.path().join("project/library");
        let bad_dir = project_lib.join("styles/franco-belgian-line");
        fs::create_dir_all(&bad_dir).unwrap();
        fs::write(bad_dir.join("card.yaml"), "schema_version: 99\n").unwrap();

        let reg = LibraryRegistry::new(Some(project_lib), None);
        let err = reg
            .resolve_style("franco-belgian-line")
            .expect_err("malformed project card must not silently fall back to builtin");
        assert!(matches!(err, CoreError::StyleInvalid(_)));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_style_exemplar_fail_closed() {
        let root = TempDir::new().expect("tempdir");
        let project_lib = root.path().join("project/library");
        let outside_png = root.path().join("outside.png");
        write_tiny_png(&outside_png);

        let style_dir = project_lib.join("styles/custom-ink");
        fs::create_dir_all(style_dir.join("exemplars")).unwrap();
        symlink(&outside_png, style_dir.join("exemplars/ex1.png")).unwrap();
        fs::write(
            style_dir.join("card.yaml"),
            r#"
schema_version: 1
slug: custom-ink
display_name: Custom Ink
medium: ink
linework: crisp
shading_and_texture: flat
palette_profile: muted
framing_and_border: full
style_guardrails:
  - keep ink sharp
exemplars:
  - exemplars/ex1.png
"#,
        )
        .unwrap();

        let reg = LibraryRegistry::new(Some(project_lib), None);
        let err = reg
            .resolve_style("custom-ink")
            .expect_err("symlink exemplar must be rejected fail-closed");
        assert!(matches!(err, CoreError::StyleInvalid(_)));
    }
}
