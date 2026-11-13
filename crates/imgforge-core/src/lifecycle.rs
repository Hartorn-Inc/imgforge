//! === ASSET LIFECYCLE & SEED INPUT CONTRACTS ===
//!
//! PURPOSE:
//! Defines the strict five-state `Lifecycle` progression (`draft -> candidate ->
//! validated -> canonical -> superseded`), the `SeedInput` bootstrap image role,
//! and shared deterministic hashing/slug/timestamp helpers used across `Subject`,
//! `Style`, `Atmosphere`, `Run`, and `manifest.lock.json`.
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Mixing bootstrap reference inputs (`Seed`) into `Lifecycle` creates ambiguous
//!    state machines where unvalidated draft images masquerade as locked assets.
//!    `SeedInput` is therefore strictly modeled as an input parameter, never a
//!    `Lifecycle` variant (`GLOSSARY.md`).
//! 2. `canonical` artifacts must remain immutable and content-addressed so
//!    downstream `ReferencePack` sheets and `SceneSpec` runs cannot silently mutate
//!    provenance; only explicit promotion of a newer version transitions a prior
//!    `canonical` entry to `superseded`.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. New assets start in `Lifecycle::Draft`.
//! 2. Transitions follow `Draft -> Candidate -> Validated -> Canonical -> Superseded`.
//! 3. Any backward, skipped, or post-`Superseded` transition fails closed with
//!    `CoreError::LifecycleTransitionInvalid`.

use std::fs::{self, OpenOptions};
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::backend::PNG_SIGNATURE;
use crate::error::CoreError;

/// Strict five-state asset lifecycle progression (`GLOSSARY.md`).
///
/// `Seed` is strictly an input role (`SeedInput`), never a `Lifecycle` state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    /// Initial unreviewed state for newly authored assets or runs.
    #[default]
    Draft,
    /// Asset or run promoted for automated/VLM inspection.
    Candidate,
    /// Asset or run that passed structural, color QA, and `VisionCritic` checks.
    Validated,
    /// Immutable approved asset frozen in `library/manifest.lock.json`.
    Canonical,
    /// Former `canonical` asset replaced by a newer promoted version.
    Superseded,
}

impl Lifecycle {
    /// Canonical lowercase identifier matching `GLOSSARY.md` and `frontend/src/types/domain.ts`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Candidate => "candidate",
            Self::Validated => "validated",
            Self::Canonical => "canonical",
            Self::Superseded => "superseded",
        }
    }

    /// Returns `true` only for valid strict forward state transitions:
    /// `draft -> candidate -> validated -> canonical -> superseded`.
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Draft, Self::Candidate)
                | (Self::Candidate, Self::Validated)
                | (Self::Validated, Self::Canonical)
                | (Self::Canonical, Self::Superseded)
        )
    }

    /// Attempt to transition from `self` to `next`, failing closed on invalid transitions.
    pub fn transition_to(self, next: Self) -> Result<Self, CoreError> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            let reason = match (self, next) {
                (a, b) if a == b => "state is already active".to_string(),
                (Self::Canonical, _) => {
                    "canonical artifacts are immutable and may only transition to 'superseded'"
                        .to_string()
                }
                (Self::Superseded, _) => {
                    "superseded artifacts are terminal and cannot transition further".to_string()
                }
                _ => "must progress strictly along draft -> candidate -> validated -> canonical -> superseded".to_string(),
            };
            Err(CoreError::LifecycleTransitionInvalid {
                from: self.as_str().to_string(),
                to: next.as_str().to_string(),
                reason,
            })
        }
    }

    /// Fail-closed immutability guard rejecting direct mutation of `canonical` artifacts.
    pub fn ensure_mutable(self, artifact: &str) -> Result<(), CoreError> {
        if self == Self::Canonical {
            Err(CoreError::CanonicalImmutable {
                artifact: artifact.to_string(),
                reason: "promote a newer version to supersede the canonical artifact instead of mutating it in place".to_string(),
            })
        } else if self == Self::Superseded {
            Err(CoreError::CanonicalImmutable {
                artifact: artifact.to_string(),
                reason: "superseded historical artifacts are read-only".to_string(),
            })
        } else {
            Ok(())
        }
    }
}

/// Bootstrap input image (`Seed`) supplied to generate a `ReferencePack` (`GLOSSARY.md`).
///
/// `SeedInput` is strictly an input role and is never stored as a `Lifecycle` state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedInput {
    /// Filesystem path to the bootstrap PNG image.
    pub path: PathBuf,
    /// Lowercase SHA-256 hex digest of the validated seed PNG bytes.
    pub sha256: String,
    /// Optional descriptive label (for example `"parchment-portrait"` or `"explore-v003"`).
    #[serde(default)]
    pub label: Option<String>,
}

impl SeedInput {
    /// Load and validate a `SeedInput` from a regular non-symlink PNG file on disk.
    pub fn from_png_path(path: &Path, label: Option<String>) -> Result<Self, CoreError> {
        let bytes = read_regular_png_bytes_nofollow(path)?;
        if bytes.len() < PNG_SIGNATURE.len() || bytes[..PNG_SIGNATURE.len()] != PNG_SIGNATURE {
            return Err(CoreError::InvalidPng {
                path: path.to_path_buf(),
                reason: "SeedInput file does not start with valid 8-byte PNG signature".to_string(),
            });
        }
        image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).map_err(|err| {
            CoreError::InvalidPng {
                path: path.to_path_buf(),
                reason: format!("SeedInput PNG decode failed: {err}"),
            }
        })?;

        let sha256 = sha256_hex(&bytes);
        let cleaned_label = label
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        Ok(Self {
            path: path.to_path_buf(),
            sha256,
            label: cleaned_label,
        })
    }
}

/// Compute lowercase hexadecimal SHA-256 digest of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Format the current UTC timestamp in RFC 3339 format.
#[must_use]
pub fn now_utc_iso() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Validate a safe lowercase kebab-case/alphanumeric entity slug (preventing path traversal).
pub(crate) fn validate_slug(slug: &str, entity_kind: &str) -> Result<(), String> {
    if slug.is_empty() {
        return Err(format!("{entity_kind} slug must not be empty"));
    }
    if slug != slug.trim() {
        return Err(format!(
            "{entity_kind} slug '{slug}' must not have leading or trailing whitespace"
        ));
    }
    if slug.starts_with('-') || slug.ends_with('-') || slug.contains("--") {
        return Err(format!(
            "{entity_kind} slug '{slug}' must be lowercase kebab-case without leading, trailing, or consecutive hyphens"
        ));
    }
    if !slug
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err(format!(
            "{entity_kind} slug '{slug}' must contain only lowercase ASCII letters, digits, '-' or '_'"
        ));
    }
    Ok(())
}

/// Validate a `#RRGGBB` color string and return its normalized uppercase `#RRGGBB` form.
pub(crate) fn normalize_hex_color(raw: &str, field_name: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() != 7 || bytes[0] != b'#' || !bytes[1..].iter().all(u8::is_ascii_hexdigit) {
        return Err(format!(
            "{field_name} color '{raw}' must be a 6-digit hex color (#RRGGBB)"
        ));
    }
    Ok(trimmed.to_ascii_uppercase())
}

/// Read a regular non-symlink file using `O_NOFOLLOW` on Unix to prevent symlink races.
pub(crate) fn read_regular_file_nofollow(
    path: &Path,
    context_label: &str,
) -> Result<Vec<u8>, CoreError> {
    let lstat_meta = fs::symlink_metadata(path).map_err(|err| CoreError::Io {
        context: format!("inspecting {context_label} at {}", path.display()),
        source: err,
    })?;
    if lstat_meta.file_type().is_symlink() || !lstat_meta.is_file() {
        return Err(CoreError::Io {
            context: format!("reading {context_label} at {}", path.display()),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing symlink or non-regular {context_label}"),
            ),
        });
    }

    #[cfg(unix)]
    {
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|err| CoreError::Io {
                context: format!("opening {context_label} at {}", path.display()),
                source: err,
            })?;
        let meta = file.metadata().map_err(|err| CoreError::Io {
            context: format!("inspecting opened {context_label} at {}", path.display()),
            source: err,
        })?;
        if !meta.is_file() {
            return Err(CoreError::Io {
                context: format!("reading {context_label} at {}", path.display()),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{context_label} is not a regular file"),
                ),
            });
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|err| CoreError::Io {
            context: format!("reading {context_label} at {}", path.display()),
            source: err,
        })?;
        Ok(bytes)
    }

    #[cfg(not(unix))]
    {
        fs::read(path).map_err(|err| CoreError::Io {
            context: format!("reading {context_label} at {}", path.display()),
            source: err,
        })
    }
}

fn read_regular_png_bytes_nofollow(path: &Path) -> Result<Vec<u8>, CoreError> {
    let lstat_meta = fs::symlink_metadata(path).map_err(|err| CoreError::Io {
        context: format!("inspecting PNG at {}", path.display()),
        source: err,
    })?;
    if lstat_meta.file_type().is_symlink() || !lstat_meta.is_file() {
        return Err(CoreError::InvalidPng {
            path: path.to_path_buf(),
            reason: "refusing symlink or non-regular PNG file".to_string(),
        });
    }
    read_regular_file_nofollow(path, "PNG file")
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

    #[rstest]
    #[case::draft_to_candidate(Lifecycle::Draft, Lifecycle::Candidate, true)]
    #[case::candidate_to_validated(Lifecycle::Candidate, Lifecycle::Validated, true)]
    #[case::validated_to_canonical(Lifecycle::Validated, Lifecycle::Canonical, true)]
    #[case::canonical_to_superseded(Lifecycle::Canonical, Lifecycle::Superseded, true)]
    #[case::skip_draft_to_validated(Lifecycle::Draft, Lifecycle::Validated, false)]
    #[case::skip_draft_to_canonical(Lifecycle::Draft, Lifecycle::Canonical, false)]
    #[case::skip_candidate_to_canonical(Lifecycle::Candidate, Lifecycle::Canonical, false)]
    #[case::backward_validated_to_candidate(Lifecycle::Validated, Lifecycle::Candidate, false)]
    #[case::backward_canonical_to_validated(Lifecycle::Canonical, Lifecycle::Validated, false)]
    #[case::backward_canonical_to_draft(Lifecycle::Canonical, Lifecycle::Draft, false)]
    #[case::terminal_superseded_to_canonical(Lifecycle::Superseded, Lifecycle::Canonical, false)]
    #[case::same_state_noop_rejected(Lifecycle::Canonical, Lifecycle::Canonical, false)]
    fn enforces_strict_five_state_lifecycle_transitions(
        #[case] from: Lifecycle,
        #[case] to: Lifecycle,
        #[case] expect_ok: bool,
    ) {
        assert_eq!(from.can_transition_to(to), expect_ok);
        let res = from.transition_to(to);
        if expect_ok {
            assert_eq!(res.expect("valid transition"), to);
        } else {
            assert!(matches!(
                res.expect_err("invalid transition must fail closed"),
                CoreError::LifecycleTransitionInvalid { .. }
            ));
        }
    }

    #[rstest]
    #[case::draft_mutable(Lifecycle::Draft, true)]
    #[case::candidate_mutable(Lifecycle::Candidate, true)]
    #[case::validated_mutable(Lifecycle::Validated, true)]
    #[case::canonical_immutable(Lifecycle::Canonical, false)]
    #[case::superseded_immutable(Lifecycle::Superseded, false)]
    fn enforces_canonical_immutability_guard(#[case] state: Lifecycle, #[case] expect_ok: bool) {
        let res = state.ensure_mutable("run:courier-ren/v001");
        assert_eq!(res.is_ok(), expect_ok);
    }

    #[test]
    fn seed_input_computes_sha256_and_rejects_symlinks() {
        let dir = TempDir::new().expect("tempdir");
        let seed_path = dir.path().join("seed.png");
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(32, 32, Rgba([100, 120, 140, 255]));
        let mut buf = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        fs::write(&seed_path, &buf).unwrap();

        let seed =
            SeedInput::from_png_path(&seed_path, Some("parchment-portrait".to_string())).unwrap();
        assert_eq!(seed.sha256, sha256_hex(&buf));
        assert_eq!(seed.label.as_deref(), Some("parchment-portrait"));

        #[cfg(unix)]
        {
            let link = dir.path().join("seed_link.png");
            symlink(&seed_path, &link).unwrap();
            let err = SeedInput::from_png_path(&link, None).expect_err("symlink seed rejected");
            assert!(matches!(err, CoreError::InvalidPng { .. }));
        }
    }

    #[rstest]
    #[case::valid_kebab("courier-ren", true)]
    #[case::valid_digits("gate-02", true)]
    #[case::uppercase_rejected("Courier-Ren", false)]
    #[case::traversal_rejected("../evil", false)]
    #[case::slash_rejected("a/b", false)]
    #[case::empty_rejected("", false)]
    #[case::double_hyphen_rejected("a--b", false)]
    fn validates_slug_format(#[case] slug: &str, #[case] expect_ok: bool) {
        assert_eq!(validate_slug(slug, "Subject").is_ok(), expect_ok);
    }
}
