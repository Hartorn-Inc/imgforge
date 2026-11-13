//! === RUN VERSIONING & MANIFEST.LOCK.JSON STORE ===
//!
//! PURPOSE:
//! Persists versioned `Run` records (`runs/<slug>/vNNN.png` + `vNNN.meta.json`)
//! and manages `library/manifest.lock.json` with SHA-256 checksums, atomic writes,
//! `canonical` immutability enforcement, and superseding promotions.
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Non-atomic writes can corrupt `vNNN.meta.json` or `manifest.lock.json` on crash;
//!    all metadata and lockfile writes therefore stream to a sibling temporary file
//!    and atomically rename into place.
//! 2. Once a `Run` or `ReferencePack` reaches `Lifecycle::Canonical`, mutating its
//!    PNG or metadata in place would invalidate downstream provenance. All update
//!    methods therefore invoke `Lifecycle::ensure_mutable`, and promoting a newer
//!    version cleanly transitions the prior `canonical` artifact to `superseded`.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. `RunStore::record_run` computes `vNNN` and `image_sha256`, writes `vNNN.png`
//!    and `vNNN.meta.json` starting in `Lifecycle::Draft`.
//! 2. `RunStore::transition_run_lifecycle` advances `draft -> candidate -> validated`.
//! 3. `ManifestLockStore::promote_run_canonical` (or `promote_canonical`) transitions
//!    `validated -> canonical`, marks any previous `canonical` version `superseded`,
//!    and atomically writes `library/manifest.lock.json`.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Write as _};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::backend::PNG_SIGNATURE;
use crate::critic::CriticReport;
use crate::error::CoreError;
use crate::lifecycle::{Lifecycle, SeedInput, now_utc_iso, sha256_hex, validate_slug};
use crate::reference_pack::ReferencePackManifest;
use crate::scene_spec::Visibility;

/// Supported `RunMetadata` and `ManifestLock` schema version.
pub const SUPPORTED_STORE_SCHEMA_VERSION: u32 = 1;

/// Canonical filename for the workspace canonical lockfile under `library/`.
pub const MANIFEST_LOCK_FILENAME: &str = "manifest.lock.json";

/// Maximum file size permitted by `read_regular_file_nofollow` (`128 MiB`).
pub const MAX_REGULAR_FILE_BYTES: u64 = 128 * 1024 * 1024;

/// Maximum `.cube` 3D LUT file size permitted during `Export` (`16 MiB`).
pub const MAX_CUBE_LUT_BYTES: u64 = 16 * 1024 * 1024;

/// Maximum permitted raster dimension per axis (`16_384 px`).
pub const MAX_IMAGE_DIM: u32 = 16_384;

/// Maximum permitted decoded raster pixel count (`67_108_864` pixels = `8192 x 8192`).
pub const MAX_IMAGE_TOTAL_PIXELS: u64 = 67_108_864;

/// Maximum memory allocation allowed inside `image::Limits` during PNG decoding (`512 MiB`).
pub const MAX_IMAGE_DECODE_ALLOC_BYTES: u64 = 512 * 1024 * 1024;

/// Backend execution snapshot captured inside `vNNN.meta.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunBackendMetadata {
    /// Backend identifier (for example `"codex-native"`).
    pub name: String,
    /// Final target width in pixels.
    pub width: u32,
    /// Final target height in pixels.
    pub height: u32,
    /// Raw width returned by backend prior to host resampling.
    pub raw_width: u32,
    /// Raw height returned by backend prior to host resampling.
    pub raw_height: u32,
    /// Optional Codex `thread_id` captured from JSONL output.
    #[serde(default)]
    pub thread_id: Option<String>,
}

/// Automated structural, color QA, and `VisionCritic` results attached to a `Run`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunQaReports {
    /// Human-readable structural validation check notes.
    #[serde(default)]
    pub structural_checks: Vec<String>,
    /// Optional read-only `VisionCritic` evaluation report.
    #[serde(default)]
    pub critic_report: Option<CriticReport>,
    /// Optional maximum OKLab/`DeltaE_2000` drift reported by color QA.
    #[serde(default)]
    pub color_qa_max_delta_e2000: Option<f64>,
}

/// Versioned `Run` metadata persisted alongside `vNNN.png` at `runs/<slug>/vNNN.meta.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunMetadata {
    /// Schema version; must equal `1`.
    pub schema_version: u32,
    /// Unique identifier (`"<slug>/vNNN"`).
    pub run_id: String,
    /// Slug of the `SceneSpec`, `Subject`, or generation stream.
    pub slug: String,
    /// Monotonic version number (`1`, `2`, ...).
    pub version: u32,
    /// Formatted version label (`"v001"`, `"v002"`, ...).
    pub version_label: String,
    /// Five-state `Lifecycle` (`draft -> candidate -> validated -> canonical -> superseded`).
    pub lifecycle: Lifecycle,
    /// Access classification (`public`, `restricted`, or `internal`).
    pub visibility: Visibility,
    /// Relative PNG filename (`"vNNN.png"`).
    pub image_file: String,
    /// Lowercase SHA-256 hex digest of `vNNN.png`.
    pub image_sha256: String,
    /// Sorted map of input artifact labels/paths to SHA-256 hex digests.
    pub input_hashes: BTreeMap<String, String>,
    /// Exact positive-only compiled prompt passed to `ImageBackend`.
    pub compiled_prompt: String,
    /// Backend execution metadata.
    pub backend: RunBackendMetadata,
    /// Optional bootstrap `SeedInput` (strictly input role, never a lifecycle state).
    #[serde(default)]
    pub seed: Option<SeedInput>,
    /// QA and `VisionCritic` reports collected for this `Run`.
    #[serde(default)]
    pub qa_reports: RunQaReports,
    /// RFC 3339 UTC creation timestamp.
    pub created_at_iso: String,
}

/// Input parameters for recording a new versioned `Run`.
#[derive(Debug, Clone)]
pub struct RecordRunParams {
    /// Target run stream slug.
    pub slug: String,
    /// `Visibility` classification of the run.
    pub visibility: Visibility,
    /// Validated PNG bytes to store as `vNNN.png`.
    pub png_bytes: Vec<u8>,
    /// Input SHA-256 hashes (cards, references, scene spec).
    pub input_hashes: BTreeMap<String, String>,
    /// Compiled positive-only prompt.
    pub compiled_prompt: String,
    /// Backend execution metadata.
    pub backend: RunBackendMetadata,
    /// Optional `SeedInput` bootstrap image.
    pub seed: Option<SeedInput>,
    /// Optional initial QA reports.
    pub qa_reports: RunQaReports,
}

/// Recorded `Run` output including filesystem paths and parsed metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedRun {
    /// Persisted `RunMetadata`.
    pub metadata: RunMetadata,
    /// Path to `runs/<slug>/vNNN.png`.
    pub image_path: PathBuf,
    /// Path to `runs/<slug>/vNNN.meta.json`.
    pub meta_path: PathBuf,
}

/// Filesystem store managing `runs/<slug>/vNNN.png` + `vNNN.meta.json`.
#[derive(Debug, Clone)]
pub struct RunStore {
    runs_dir: PathBuf,
}

impl RunStore {
    /// Construct a `RunStore` rooted at `<workspace>/runs`.
    #[must_use]
    pub fn new(runs_dir: PathBuf) -> Self {
        Self { runs_dir }
    }

    /// Return the root `runs` directory.
    #[must_use]
    pub fn runs_dir(&self) -> &Path {
        &self.runs_dir
    }

    /// Format a version number into canonical `vNNN` label (`v001`, `v002`, ...).
    #[must_use]
    pub fn format_version_label(version: u32) -> String {
        format!("v{version:03}")
    }

    /// Compute the next deterministic version number (`1` when no runs exist yet for `slug`).
    pub fn next_version(&self, slug: &str) -> Result<u32, CoreError> {
        validate_slug(slug, "Run").map_err(|msg| CoreError::Io {
            context: "validating Run slug".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, msg),
        })?;
        let Some(slug_dir) = verify_optional_contained_dir_no_symlinks(
            &self.runs_dir,
            Path::new(slug),
            "Run directory",
        )?
        else {
            return Ok(1);
        };

        let mut max_ver = 0u32;
        let entries = fs::read_dir(&slug_dir).map_err(|err| CoreError::Io {
            context: format!("listing Run directory {}", slug_dir.display()),
            source: err,
        })?;
        for entry_res in entries {
            let entry = entry_res.map_err(|err| CoreError::Io {
                context: format!("reading entry in {}", slug_dir.display()),
                source: err,
            })?;
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            if let Some(num) = parse_version_from_filename(name_str) {
                max_ver = max_ver.max(num);
            }
        }
        Ok(max_ver + 1)
    }

    /// Record a new `Run` as `vNNN.png` + `vNNN.meta.json` starting in `Lifecycle::Draft`.
    pub fn record_run(&self, params: RecordRunParams) -> Result<RecordedRun, CoreError> {
        validate_slug(&params.slug, "Run").map_err(|msg| CoreError::Io {
            context: "validating Run slug".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, msg),
        })?;
        if params.png_bytes.len() < PNG_SIGNATURE.len()
            || params.png_bytes[..PNG_SIGNATURE.len()] != PNG_SIGNATURE
        {
            return Err(CoreError::InvalidPng {
                path: self.runs_dir.join(&params.slug),
                reason: "Run image bytes do not start with valid 8-byte PNG signature".to_string(),
            });
        }

        let slug_rel = Path::new(&params.slug);
        verify_existing_dir_prefix_no_symlinks(&self.runs_dir, slug_rel, "Run directory")?;
        let slug_dir = self.runs_dir.join(&params.slug);
        fs::create_dir_all(&slug_dir).map_err(|err| CoreError::Io {
            context: format!("creating Run directory {}", slug_dir.display()),
            source: err,
        })?;
        verify_contained_path_no_symlinks(&self.runs_dir, slug_rel, true, "Run directory")?;

        let version = self.next_version(&params.slug)?;
        let version_label = Self::format_version_label(version);
        let image_file = format!("{version_label}.png");
        let meta_file = format!("{version_label}.meta.json");
        let image_path = slug_dir.join(&image_file);
        let meta_path = slug_dir.join(&meta_file);

        let image_sha256 = sha256_hex(&params.png_bytes);
        let metadata = RunMetadata {
            schema_version: SUPPORTED_STORE_SCHEMA_VERSION,
            run_id: format!("{}/{version_label}", params.slug),
            slug: params.slug,
            version,
            version_label,
            lifecycle: Lifecycle::Draft,
            visibility: params.visibility,
            image_file,
            image_sha256,
            input_hashes: params.input_hashes,
            compiled_prompt: params.compiled_prompt,
            backend: params.backend,
            seed: params.seed,
            qa_reports: params.qa_reports,
            created_at_iso: now_utc_iso(),
        };

        write_atomic(&image_path, &params.png_bytes, "Run PNG")?;
        let meta_json = serde_json::to_vec_pretty(&metadata).map_err(|err| CoreError::Io {
            context: "serializing RunMetadata".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, err),
        })?;
        write_atomic(&meta_path, &meta_json, "Run metadata")?;

        Ok(RecordedRun {
            metadata,
            image_path,
            meta_path,
        })
    }

    /// Load `RunMetadata` for `(slug, version)` fail-closed.
    pub fn load_run(&self, slug: &str, version: u32) -> Result<RunMetadata, CoreError> {
        validate_slug(slug, "Run").map_err(|msg| CoreError::Io {
            context: "validating Run slug".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, msg),
        })?;
        let rel_meta =
            Path::new(slug).join(format!("{}.meta.json", Self::format_version_label(version)));
        let meta_path =
            verify_contained_path_no_symlinks(&self.runs_dir, &rel_meta, false, "Run metadata")?;
        let bytes = read_regular_file_nofollow(&meta_path, "Run metadata")?;
        let meta: RunMetadata = serde_json::from_slice(&bytes).map_err(|err| CoreError::Io {
            context: format!("parsing Run metadata {}", meta_path.display()),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, err),
        })?;
        if meta.schema_version != SUPPORTED_STORE_SCHEMA_VERSION {
            return Err(CoreError::Io {
                context: format!("validating Run metadata {}", meta_path.display()),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "unsupported RunMetadata schema_version {}",
                        meta.schema_version
                    ),
                ),
            });
        }
        Ok(meta)
    }

    /// List all recorded versions for `slug` in ascending version order.
    pub fn list_runs(&self, slug: &str) -> Result<Vec<RunMetadata>, CoreError> {
        validate_slug(slug, "Run").map_err(|msg| CoreError::Io {
            context: "validating Run slug".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, msg),
        })?;
        let Some(slug_dir) = verify_optional_contained_dir_no_symlinks(
            &self.runs_dir,
            Path::new(slug),
            "Run directory",
        )?
        else {
            return Ok(Vec::new());
        };
        let mut versions = Vec::new();
        for entry_res in fs::read_dir(&slug_dir).map_err(|err| CoreError::Io {
            context: format!("listing Run directory {}", slug_dir.display()),
            source: err,
        })? {
            let entry = entry_res.map_err(|err| CoreError::Io {
                context: format!("reading Run directory {}", slug_dir.display()),
                source: err,
            })?;
            let name = entry.file_name();
            if let Some(name_str) = name.to_str()
                && name_str.ends_with(".meta.json")
                && let Some(ver) = parse_version_from_filename(name_str)
            {
                versions.push(ver);
            }
        }
        versions.sort_unstable();
        let mut out = Vec::with_capacity(versions.len());
        for ver in versions {
            out.push(self.load_run(slug, ver)?);
        }
        Ok(out)
    }

    /// Advance a `Run` strictly along `draft -> candidate -> validated -> canonical -> superseded`.
    pub fn transition_run_lifecycle(
        &self,
        slug: &str,
        version: u32,
        next: Lifecycle,
    ) -> Result<RunMetadata, CoreError> {
        let mut meta = self.load_run(slug, version)?;
        meta.lifecycle = meta.lifecycle.transition_to(next)?;
        self.persist_metadata(&meta)?;
        Ok(meta)
    }

    /// Attach or update QA reports on a mutable (`draft`/`candidate`/`validated`) `Run`.
    /// Fails closed if the `Run` is `canonical` or `superseded`.
    pub fn update_run_qa(
        &self,
        slug: &str,
        version: u32,
        qa_reports: RunQaReports,
    ) -> Result<RunMetadata, CoreError> {
        let mut meta = self.load_run(slug, version)?;
        meta.lifecycle.ensure_mutable(&meta.run_id)?;
        meta.qa_reports = qa_reports;
        self.persist_metadata(&meta)?;
        Ok(meta)
    }

    /// Return the expected path to `runs/<slug>/vNNN.png`.
    #[must_use]
    pub fn run_image_path(&self, slug: &str, version: u32) -> PathBuf {
        self.runs_dir
            .join(slug)
            .join(format!("{}.png", Self::format_version_label(version)))
    }

    /// Return the expected path to `runs/<slug>/vNNN.meta.json`.
    #[must_use]
    pub fn run_meta_path(&self, slug: &str, version: u32) -> PathBuf {
        self.runs_dir
            .join(slug)
            .join(format!("{}.meta.json", Self::format_version_label(version)))
    }

    /// Inspect whether `input_path` resolves inside `self.runs_dir` with a sibling
    /// `vNNN.meta.json` and, if so, return that `RunMetadata.visibility` fail-closed.
    pub fn inspect_run_image_visibility(
        &self,
        input_path: &Path,
    ) -> Result<Option<Visibility>, CoreError> {
        let canon_runs = match fs::canonicalize(&self.runs_dir) {
            Ok(p) => p,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(CoreError::Io {
                    context: format!("canonicalizing runs_dir {}", self.runs_dir.display()),
                    source: err,
                });
            }
        };
        let canon_input = match fs::canonicalize(input_path) {
            Ok(p) => p,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(CoreError::Io {
                    context: format!("canonicalizing input path {}", input_path.display()),
                    source: err,
                });
            }
        };
        if !canon_input.starts_with(&canon_runs) {
            return Ok(None);
        }
        let Some(file_name) = canon_input.file_name().and_then(|s| s.to_str()) else {
            return Ok(None);
        };
        if !file_name.to_ascii_lowercase().ends_with(".png")
            || parse_version_from_filename(file_name).is_none()
        {
            return Ok(None);
        }
        let Some(stem) = canon_input.file_stem().and_then(|s| s.to_str()) else {
            return Ok(None);
        };
        let meta_path = canon_input.with_file_name(format!("{stem}.meta.json"));
        match fs::symlink_metadata(&meta_path) {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(CoreError::Io {
                    context: format!("inspecting sibling Run metadata {}", meta_path.display()),
                    source: err,
                });
            }
        }
        let bytes = read_regular_file_nofollow(&meta_path, "Run metadata")?;
        let meta: RunMetadata = serde_json::from_slice(&bytes).map_err(|err| CoreError::Io {
            context: format!("parsing Run metadata {}", meta_path.display()),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, err),
        })?;
        if meta.schema_version != SUPPORTED_STORE_SCHEMA_VERSION {
            return Err(CoreError::Io {
                context: format!("validating Run metadata {}", meta_path.display()),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "unsupported RunMetadata schema_version {}",
                        meta.schema_version
                    ),
                ),
            });
        }
        Ok(Some(meta.visibility))
    }

    fn persist_metadata(&self, meta: &RunMetadata) -> Result<(), CoreError> {
        let meta_path = self.run_meta_path(&meta.slug, meta.version);
        let bytes = serde_json::to_vec_pretty(meta).map_err(|err| CoreError::Io {
            context: "serializing RunMetadata".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, err),
        })?;
        write_atomic(&meta_path, &bytes, "Run metadata")
    }
}

fn parse_version_from_filename(name: &str) -> Option<u32> {
    let rest = name.strip_prefix('v')?;
    let digits = if let Some(stem) = rest.strip_suffix(".meta.json") {
        stem
    } else {
        rest.strip_suffix(".png")?
    };
    if digits.len() >= 3 && digits.bytes().all(|b| b.is_ascii_digit()) {
        digits.parse::<u32>().ok()
    } else {
        None
    }
}

/// Canonical lock entry classification recorded in `library/manifest.lock.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestEntryKind {
    /// Style-neutral `Subject` card.
    Subject,
    /// First-class `Style` card and exemplars.
    Style,
    /// Environmental `Atmosphere` card.
    Atmosphere,
    /// Style-scoped `ReferencePack` sheets and slice manifest.
    ReferencePack,
    /// Versioned generation `Run`.
    Run,
}

/// Single entry inside `library/manifest.lock.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestEntry {
    /// Kind of locked asset.
    pub entry_kind: ManifestEntryKind,
    /// Canonical lock key (for example `"pack:courier-ren/franco-belgian-line"` or `"run:harbor"`).
    pub key: String,
    /// Monotonic version number.
    pub version: u32,
    /// Human-readable version label (`"v001"`, `"v002"`, ...).
    pub version_label: String,
    /// Lifecycle state (`Canonical` for active lock entries; `Superseded` in history).
    pub lifecycle: Lifecycle,
    /// Access classification (`public`, `restricted`, or `internal`).
    pub visibility: Visibility,
    /// Sorted map of workspace-relative artifact paths to lowercase SHA-256 hex digests.
    pub artifact_hashes: BTreeMap<String, String>,
    /// RFC 3339 UTC timestamp when promoted.
    pub promoted_at_iso: String,
}

/// Atomic lockfile (`library/manifest.lock.json`) freezing all `canonical` assets and `ReferencePack`s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestLock {
    /// Schema version; must equal `1`.
    pub schema_version: u32,
    /// Active `canonical` entries keyed by canonical identifier.
    pub canonical: BTreeMap<String, ManifestEntry>,
    /// Historical prior `canonical` entries transitioned to `superseded`.
    #[serde(default)]
    pub superseded: Vec<ManifestEntry>,
}

impl Default for ManifestLock {
    fn default() -> Self {
        Self {
            schema_version: SUPPORTED_STORE_SCHEMA_VERSION,
            canonical: BTreeMap::new(),
            superseded: Vec::new(),
        }
    }
}

/// Persistence manager for `library/manifest.lock.json`.
#[derive(Debug, Clone)]
pub struct ManifestLockStore {
    lock_path: PathBuf,
}

impl ManifestLockStore {
    /// Construct a `ManifestLockStore` for `<workspace>/library/manifest.lock.json`.
    #[must_use]
    pub fn new(library_dir: &Path) -> Self {
        Self {
            lock_path: library_dir.join(MANIFEST_LOCK_FILENAME),
        }
    }

    /// Return the path to `manifest.lock.json`.
    #[must_use]
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// Load `manifest.lock.json` if present, or return an empty initialized `ManifestLock`.
    pub fn load_or_init(&self) -> Result<ManifestLock, CoreError> {
        match fs::symlink_metadata(&self.lock_path) {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ManifestLock::default());
            }
            Err(err) => {
                return Err(CoreError::Io {
                    context: format!("inspecting {}", self.lock_path.display()),
                    source: err,
                });
            }
        }
        let bytes = read_regular_file_nofollow(&self.lock_path, "manifest.lock.json")?;
        let lock: ManifestLock = serde_json::from_slice(&bytes)
            .map_err(|err| CoreError::ManifestLockInvalid(err.to_string()))?;
        if lock.schema_version != SUPPORTED_STORE_SCHEMA_VERSION {
            return Err(CoreError::ManifestLockInvalid(format!(
                "unsupported schema_version {} (expected {})",
                lock.schema_version, SUPPORTED_STORE_SCHEMA_VERSION
            )));
        }
        for (key, entry) in &lock.canonical {
            if entry.lifecycle != Lifecycle::Canonical {
                return Err(CoreError::ManifestLockInvalid(format!(
                    "canonical entry '{key}' has non-canonical lifecycle '{}'",
                    entry.lifecycle.as_str()
                )));
            }
        }
        for entry in &lock.superseded {
            if entry.lifecycle != Lifecycle::Superseded {
                return Err(CoreError::ManifestLockInvalid(format!(
                    "superseded entry '{}' has lifecycle '{}' (expected 'superseded')",
                    entry.key,
                    entry.lifecycle.as_str()
                )));
            }
        }
        Ok(lock)
    }

    fn prepare_promoted_lock(
        &self,
        entry: ManifestEntry,
    ) -> Result<(ManifestLock, Vec<u8>), CoreError> {
        if entry.lifecycle != Lifecycle::Canonical {
            return Err(CoreError::ManifestLockInvalid(format!(
                "promote_canonical requires lifecycle 'canonical' (got '{}')",
                entry.lifecycle.as_str()
            )));
        }
        if entry.artifact_hashes.is_empty() {
            return Err(CoreError::ManifestLockInvalid(format!(
                "canonical entry '{}' must record at least 1 SHA-256 artifact hash",
                entry.key
            )));
        }

        let mut lock = self.load_or_init()?;
        if let Some(mut prior) = lock.canonical.remove(&entry.key) {
            if entry.version <= prior.version {
                return Err(CoreError::ManifestLockInvalid(format!(
                    "new canonical version {} for '{}' must exceed prior canonical version {}",
                    entry.version, entry.key, prior.version
                )));
            }
            prior.lifecycle = prior.lifecycle.transition_to(Lifecycle::Superseded)?;
            lock.superseded.push(prior);
        }
        lock.canonical.insert(entry.key.clone(), entry);
        let lock_bytes = serde_json::to_vec_pretty(&lock)
            .map_err(|err| CoreError::ManifestLockInvalid(err.to_string()))?;
        Ok((lock, lock_bytes))
    }

    /// Atomically promote a `canonical` entry into `manifest.lock.json`, marking any prior
    /// `canonical` entry for the same `key` as `superseded`.
    pub fn promote_canonical(&self, entry: ManifestEntry) -> Result<ManifestLock, CoreError> {
        let (lock, lock_bytes) = self.prepare_promoted_lock(entry)?;
        write_atomic(&self.lock_path, &lock_bytes, "manifest.lock.json")?;
        Ok(lock)
    }

    /// Promote a `Validated` `Run` to `Canonical`, mark any prior `Canonical` version of the
    /// same `Run` stream `Superseded` on disk and in `manifest.lock.json`, and lock SHA-256 hashes.
    ///
    /// Performs full preflight validation, symlink/containment checks, file reads, SHA-256
    /// hashing, and JSON serialization in memory before mutating any file on disk.
    pub fn promote_run_canonical(
        &self,
        run_store: &RunStore,
        slug: &str,
        version: u32,
        workspace_root: &Path,
    ) -> Result<(RunMetadata, ManifestLock), CoreError> {
        // 1. Preflight: validate Run lifecycle and prior canonical version ordering.
        let current = run_store.load_run(slug, version)?;
        let next_lifecycle = current.lifecycle.transition_to(Lifecycle::Canonical)?;

        let runs_dir = run_store.runs_dir();
        if let Ok(rel_runs_dir) = runs_dir.strip_prefix(workspace_root)
            && !rel_runs_dir.as_os_str().is_empty()
        {
            verify_contained_path_no_symlinks(
                workspace_root,
                rel_runs_dir,
                true,
                "runs directory",
            )?;
        }

        let version_label = RunStore::format_version_label(version);
        let rel_image = Path::new(slug).join(format!("{version_label}.png"));
        let rel_meta = Path::new(slug).join(format!("{version_label}.meta.json"));
        let image_path = verify_contained_path_no_symlinks(runs_dir, &rel_image, false, "Run PNG")?;
        let meta_path =
            verify_contained_path_no_symlinks(runs_dir, &rel_meta, false, "Run metadata")?;

        let lock = self.load_or_init()?;
        let lock_key = format!("run:{slug}");
        let prior_superseded = if let Some(prior_entry) = lock.canonical.get(&lock_key) {
            if version <= prior_entry.version {
                return Err(CoreError::ManifestLockInvalid(format!(
                    "new canonical version {version} for '{lock_key}' must exceed prior canonical version {}",
                    prior_entry.version
                )));
            }
            let mut prior_meta = run_store.load_run(slug, prior_entry.version)?;
            prior_meta.lifecycle = prior_meta.lifecycle.transition_to(Lifecycle::Superseded)?;
            let prior_bytes =
                serde_json::to_vec_pretty(&prior_meta).map_err(|err| CoreError::Io {
                    context: "serializing superseded RunMetadata".to_string(),
                    source: std::io::Error::new(std::io::ErrorKind::InvalidData, err),
                })?;
            Some((
                run_store.run_meta_path(slug, prior_entry.version),
                prior_bytes,
            ))
        } else {
            None
        };

        // 2. Preflight: read PNG, verify metadata & paths, serialize promoted metadata, and hash.
        let img_bytes = read_regular_file_nofollow(&image_path, "Run PNG")?;
        let _existing_meta_bytes = read_regular_file_nofollow(&meta_path, "Run metadata")?;

        let rel_img_key = relative_workspace_key(workspace_root, &image_path)?;
        let rel_meta_key = relative_workspace_key(workspace_root, &meta_path)?;

        let mut promoted_meta = current;
        promoted_meta.lifecycle = next_lifecycle;
        let promoted_meta_bytes =
            serde_json::to_vec_pretty(&promoted_meta).map_err(|err| CoreError::Io {
                context: "serializing promoted RunMetadata".to_string(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, err),
            })?;

        let mut artifact_hashes = BTreeMap::new();
        artifact_hashes.insert(rel_img_key, sha256_hex(&img_bytes));
        artifact_hashes.insert(rel_meta_key, sha256_hex(&promoted_meta_bytes));

        let (updated_lock, lock_bytes) = self.prepare_promoted_lock(ManifestEntry {
            entry_kind: ManifestEntryKind::Run,
            key: lock_key,
            version: promoted_meta.version,
            version_label: promoted_meta.version_label.clone(),
            lifecycle: Lifecycle::Canonical,
            visibility: promoted_meta.visibility,
            artifact_hashes,
            promoted_at_iso: now_utc_iso(),
        })?;

        // 3. Commit atomically only after every preflight check and serialization succeeded.
        write_atomic(&meta_path, &promoted_meta_bytes, "Run metadata")?;
        write_atomic(&self.lock_path, &lock_bytes, "manifest.lock.json")?;
        if let Some((prior_meta_path, prior_bytes)) = prior_superseded {
            write_atomic(&prior_meta_path, &prior_bytes, "Run metadata")?;
        }

        Ok((promoted_meta, updated_lock))
    }

    /// Promote a `Validated` style-scoped `ReferencePack` (`pack.json`) to `Canonical`
    /// and record SHA-256 checksums for all sheets, slices, and manifests in `manifest.lock.json`.
    ///
    /// Performs full preflight validation, relative-path & symlink confinement checks inside
    /// `pack_dir`, file reads, SHA-256 hashing, and JSON serialization before mutating `pack.json`
    /// or `manifest.lock.json`.
    pub fn promote_reference_pack_canonical(
        &self,
        pack_dir: &Path,
        workspace_root: &Path,
    ) -> Result<(ReferencePackManifest, ManifestLock), CoreError> {
        // 1. Preflight: verify pack_dir is a non-symlink directory confined inside workspace_root.
        let canonical_ws = fs::canonicalize(workspace_root).map_err(|err| CoreError::Io {
            context: format!("canonicalizing workspace {}", workspace_root.display()),
            source: err,
        })?;
        if let Ok(rel_pack_dir) = pack_dir.strip_prefix(workspace_root)
            && !rel_pack_dir.as_os_str().is_empty()
        {
            verify_contained_path_no_symlinks(
                workspace_root,
                rel_pack_dir,
                true,
                "ReferencePack directory",
            )?;
        } else {
            let meta = fs::symlink_metadata(pack_dir).map_err(|err| CoreError::Io {
                context: format!("inspecting ReferencePack directory {}", pack_dir.display()),
                source: err,
            })?;
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(CoreError::ManifestLockInvalid(format!(
                    "ReferencePack directory '{}' must be a non-symlink directory",
                    pack_dir.display()
                )));
            }
            let canonical_pack = fs::canonicalize(pack_dir).map_err(|err| CoreError::Io {
                context: format!(
                    "canonicalizing ReferencePack directory {}",
                    pack_dir.display()
                ),
                source: err,
            })?;
            if !canonical_pack.starts_with(&canonical_ws) {
                return Err(CoreError::ManifestLockInvalid(format!(
                    "ReferencePack directory '{}' escapes workspace root",
                    pack_dir.display()
                )));
            }
        }

        let pack_path = verify_contained_path_no_symlinks(
            pack_dir,
            Path::new("pack.json"),
            false,
            "pack.json",
        )?;
        let raw = read_regular_file_nofollow(&pack_path, "pack.json")?;
        let mut manifest: ReferencePackManifest = serde_json::from_slice(&raw)
            .map_err(|err| CoreError::ManifestLockInvalid(err.to_string()))?;

        // 2. Preflight: validate Validated -> Canonical lifecycle and prior canonical version.
        manifest.lifecycle = manifest.lifecycle.transition_to(Lifecycle::Canonical)?;

        let lock = self.load_or_init()?;
        let lock_key = format!("pack:{}/{}", manifest.subject_slug, manifest.style_slug);
        if let Some(prior_entry) = lock.canonical.get(&lock_key)
            && manifest.version <= prior_entry.version
        {
            return Err(CoreError::ManifestLockInvalid(format!(
                "new canonical version {} for '{lock_key}' must exceed prior canonical version {}",
                manifest.version, prior_entry.version
            )));
        }

        // 3. Preflight: validate every manifest.artifact_hashes key is strictly confined inside pack_dir,
        //    symlink-free, and readable before mutating pack.json.
        let updated_pack_bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(|err| CoreError::ManifestLockInvalid(err.to_string()))?;

        let mut artifact_hashes = BTreeMap::new();
        artifact_hashes.insert(
            relative_workspace_key(workspace_root, &pack_path)?,
            sha256_hex(&updated_pack_bytes),
        );
        for rel_name in manifest.artifact_hashes.keys() {
            let rel_path = validate_strict_relative_path(rel_name, "ReferencePack artifact")
                .map_err(CoreError::ManifestLockInvalid)?;
            let full = verify_contained_path_no_symlinks(pack_dir, &rel_path, false, rel_name)
                .map_err(|err| CoreError::ManifestLockInvalid(err.to_string()))?;
            let bytes = read_regular_file_nofollow(&full, rel_name)?;
            artifact_hashes.insert(
                relative_workspace_key(workspace_root, &full)?,
                sha256_hex(&bytes),
            );
        }

        let version_label = RunStore::format_version_label(manifest.version);
        let (updated_lock, lock_bytes) = self.prepare_promoted_lock(ManifestEntry {
            entry_kind: ManifestEntryKind::ReferencePack,
            key: lock_key,
            version: manifest.version,
            version_label,
            lifecycle: Lifecycle::Canonical,
            visibility: manifest.visibility,
            artifact_hashes,
            promoted_at_iso: now_utc_iso(),
        })?;

        // 4. Commit pack.json and manifest.lock.json atomically.
        write_atomic(&pack_path, &updated_pack_bytes, "pack.json")?;
        write_atomic(&self.lock_path, &lock_bytes, "manifest.lock.json")?;

        Ok((manifest, updated_lock))
    }

    /// Verify that every artifact in `manifest.lock.json`'s `canonical` table exists on disk
    /// as a regular non-symlink file inside `workspace_root` with matching SHA-256 checksum.
    pub fn verify_canonical_checksums(&self, workspace_root: &Path) -> Result<(), CoreError> {
        let lock = self.load_or_init()?;

        for (key, entry) in &lock.canonical {
            for (rel_str, expected_hash) in &entry.artifact_hashes {
                let rel_path =
                    validate_strict_relative_path(rel_str, &format!("canonical entry '{key}'"))
                        .map_err(CoreError::ManifestLockInvalid)?;
                let full_path = verify_contained_path_no_symlinks(
                    workspace_root,
                    &rel_path,
                    false,
                    &format!("canonical artifact '{rel_str}' ({key})"),
                )
                .map_err(|err| CoreError::ManifestLockInvalid(err.to_string()))?;
                let bytes = read_regular_file_nofollow(&full_path, "canonical artifact")?;
                let actual_hash = sha256_hex(&bytes);
                if actual_hash != *expected_hash {
                    return Err(CoreError::ManifestLockInvalid(format!(
                        "checksum mismatch for canonical artifact '{rel_str}' ({key}): expected {expected_hash}, got {actual_hash}"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Compute a safe workspace-relative slash-separated path for `manifest.lock.json` keys.
pub(crate) fn relative_workspace_key(
    workspace_root: &Path,
    target: &Path,
) -> Result<String, CoreError> {
    let canonical_ws = fs::canonicalize(workspace_root).map_err(|err| CoreError::Io {
        context: format!("canonicalizing workspace {}", workspace_root.display()),
        source: err,
    })?;
    let canonical_target = fs::canonicalize(target).map_err(|err| CoreError::Io {
        context: format!("canonicalizing artifact {}", target.display()),
        source: err,
    })?;
    let rel = canonical_target.strip_prefix(&canonical_ws).map_err(|_| {
        CoreError::ManifestLockInvalid(format!(
            "artifact {} is outside workspace {}",
            target.display(),
            workspace_root.display()
        ))
    })?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

/// Write `bytes` to `dest` atomically via a sibling temporary file + fsync + rename.
pub fn write_atomic(dest: &Path, bytes: &[u8], label: &str) -> Result<(), CoreError> {
    let raw_parent = dest.parent().ok_or_else(|| CoreError::Io {
        context: format!(
            "resolving parent directory for {label} at {}",
            dest.display()
        ),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing parent directory"),
    })?;
    let parent = if raw_parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        raw_parent
    };
    fs::create_dir_all(parent).map_err(|err| CoreError::Io {
        context: format!("creating directory {} for {label}", parent.display()),
        source: err,
    })?;

    let mut tmp = NamedTempFile::new_in(parent).map_err(|err| CoreError::Io {
        context: format!("creating temp file for {label} in {}", parent.display()),
        source: err,
    })?;
    tmp.write_all(bytes).map_err(|err| CoreError::Io {
        context: format!("writing temp {label} at {}", tmp.path().display()),
        source: err,
    })?;
    tmp.as_file().sync_all().map_err(|err| CoreError::Io {
        context: format!("syncing temp {label} at {}", tmp.path().display()),
        source: err,
    })?;
    let _file: File = tmp.persist(dest).map_err(|err| CoreError::Io {
        context: format!("persisting {label} to {}", dest.display()),
        source: err.error,
    })?;
    Ok(())
}

/// Validate that `raw` is a strict relative path with only normal segments (no `/`, `\`, `..`, `.`,
/// NUL, or control characters).
pub(crate) fn validate_strict_relative_path(raw: &str, label: &str) -> Result<PathBuf, String> {
    if raw.is_empty()
        || raw != raw.trim()
        || raw.contains('\\')
        || raw.contains('\0')
        || raw.chars().any(char::is_control)
    {
        return Err(format!("{label} has invalid relative path '{raw}'"));
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(format!("{label} path '{raw}' must be relative"));
    }
    let mut count = 0usize;
    for comp in path.components() {
        match comp {
            Component::Normal(seg) if !seg.is_empty() => count += 1,
            _ => {
                return Err(format!(
                    "{label} path '{raw}' must contain only normal relative path segments"
                ));
            }
        }
    }
    if count == 0 {
        return Err(format!("{label} path '{raw}' must not be empty"));
    }
    Ok(path.to_path_buf())
}

/// Walk every path component from `root` down to `root/rel` via `fs::symlink_metadata` to ensure
/// zero symlinks in `root`, intermediate directories, or leaf target, and verify canonical containment.
pub(crate) fn verify_contained_path_no_symlinks(
    root: &Path,
    rel: &Path,
    target_is_dir: bool,
    label: &str,
) -> Result<PathBuf, CoreError> {
    let root_meta = fs::symlink_metadata(root).map_err(|err| CoreError::Io {
        context: format!("inspecting root directory {} for {label}", root.display()),
        source: err,
    })?;
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        return Err(CoreError::Io {
            context: format!("inspecting root directory {} for {label}", root.display()),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("root directory for {label} must not be a symlink"),
            ),
        });
    }
    let canonical_root = fs::canonicalize(root).map_err(|err| CoreError::Io {
        context: format!(
            "canonicalizing root directory {} for {label}",
            root.display()
        ),
        source: err,
    })?;

    let components: Vec<_> = rel.components().collect();
    if components.is_empty() || !components.iter().all(|c| matches!(c, Component::Normal(_))) {
        return Err(CoreError::Io {
            context: format!("validating relative path for {label}"),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{label} path '{}' must be strictly relative", rel.display()),
            ),
        });
    }

    let mut cursor = root.to_path_buf();
    for (idx, comp) in components.iter().enumerate() {
        cursor.push(comp.as_os_str());
        let meta = fs::symlink_metadata(&cursor).map_err(|err| CoreError::Io {
            context: format!("inspecting {label} at {}", cursor.display()),
            source: err,
        })?;
        if meta.file_type().is_symlink() {
            return Err(CoreError::Io {
                context: format!("inspecting {label} at {}", cursor.display()),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("refusing symlink in {label} path: {}", cursor.display()),
                ),
            });
        }
        let is_last = idx + 1 == components.len();
        if !is_last || target_is_dir {
            if !meta.is_dir() {
                return Err(CoreError::Io {
                    context: format!("inspecting {label} at {}", cursor.display()),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("{label} segment {} is not a directory", cursor.display()),
                    ),
                });
            }
        } else if !meta.is_file() {
            return Err(CoreError::Io {
                context: format!("inspecting {label} at {}", cursor.display()),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{label} {} is not a regular file", cursor.display()),
                ),
            });
        }
    }

    let canonical_target = fs::canonicalize(&cursor).map_err(|err| CoreError::Io {
        context: format!("canonicalizing {label} at {}", cursor.display()),
        source: err,
    })?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err(CoreError::Io {
            context: format!("verifying containment for {label} at {}", cursor.display()),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "{label} {} escapes root {}",
                    cursor.display(),
                    root.display()
                ),
            ),
        });
    }

    Ok(cursor)
}

/// Verify all existing directory segments from `root` down toward `root/rel` are non-symlink
/// directories confined inside `canonical_root`, stopping cleanly when a segment does not exist yet.
pub(crate) fn verify_existing_dir_prefix_no_symlinks(
    root: &Path,
    rel: &Path,
    label: &str,
) -> Result<(), CoreError> {
    let components: Vec<_> = rel.components().collect();
    if components.is_empty() || !components.iter().all(|c| matches!(c, Component::Normal(_))) {
        return Err(CoreError::Io {
            context: format!("validating relative path for {label}"),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{label} path '{}' must be strictly relative", rel.display()),
            ),
        });
    }

    match fs::symlink_metadata(root) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(CoreError::Io {
                    context: format!("inspecting root directory {} for {label}", root.display()),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("root directory for {label} must not be a symlink"),
                    ),
                });
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(CoreError::Io {
                context: format!("inspecting root directory {} for {label}", root.display()),
                source: err,
            });
        }
    }

    let canonical_root = fs::canonicalize(root).map_err(|err| CoreError::Io {
        context: format!(
            "canonicalizing root directory {} for {label}",
            root.display()
        ),
        source: err,
    })?;

    let mut cursor = root.to_path_buf();
    for comp in components {
        cursor.push(comp.as_os_str());
        match fs::symlink_metadata(&cursor) {
            Ok(meta) => {
                if meta.file_type().is_symlink() || !meta.is_dir() {
                    return Err(CoreError::Io {
                        context: format!("inspecting {label} at {}", cursor.display()),
                        source: std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!(
                                "refusing symlink or non-directory in {label} path: {}",
                                cursor.display()
                            ),
                        ),
                    });
                }
                let canonical_cur = fs::canonicalize(&cursor).map_err(|err| CoreError::Io {
                    context: format!("canonicalizing {label} at {}", cursor.display()),
                    source: err,
                })?;
                if !canonical_cur.starts_with(&canonical_root) {
                    return Err(CoreError::Io {
                        context: format!(
                            "verifying containment for {label} at {}",
                            cursor.display()
                        ),
                        source: std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!(
                                "{label} {} escapes root {}",
                                cursor.display(),
                                root.display()
                            ),
                        ),
                    });
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => {
                return Err(CoreError::Io {
                    context: format!("inspecting {label} at {}", cursor.display()),
                    source: err,
                });
            }
        }
    }

    Ok(())
}

/// Probe an optional directory `root/rel`: returns `Ok(None)` if any segment does not exist,
/// returns `Ok(Some(path))` if every segment is a non-symlink directory inside `canonical_root`,
/// and fails closed if any existing segment is a symlink, non-directory, or escapes `canonical_root`.
pub(crate) fn verify_optional_contained_dir_no_symlinks(
    root: &Path,
    rel: &Path,
    label: &str,
) -> Result<Option<PathBuf>, CoreError> {
    verify_existing_dir_prefix_no_symlinks(root, rel, label)?;
    let target = root.join(rel);
    match fs::symlink_metadata(&target) {
        Ok(_) => verify_contained_path_no_symlinks(root, rel, true, label).map(Some),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(CoreError::Io {
            context: format!("inspecting {label} at {}", target.display()),
            source: err,
        }),
    }
}

/// Compute `width * height` safely using checked arithmetic and enforce `MAX_IMAGE_DIM` and
/// `MAX_IMAGE_TOTAL_PIXELS` bounds fail-closed.
pub(crate) fn checked_pixel_count(
    width: u32,
    height: u32,
    context: &str,
) -> Result<usize, CoreError> {
    if width == 0 || height == 0 || width > MAX_IMAGE_DIM || height > MAX_IMAGE_DIM {
        return Err(CoreError::EditFailed(format!(
            "{context} dimensions {width}x{height} out of bounds 1..={MAX_IMAGE_DIM}"
        )));
    }
    let total_u64 = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| {
            CoreError::EditFailed(format!(
                "{context} pixel count overflow for {width}x{height}"
            ))
        })?;
    if total_u64 > MAX_IMAGE_TOTAL_PIXELS {
        return Err(CoreError::EditFailed(format!(
            "{context} total pixel count {total_u64} ({width}x{height}) exceeds limit {MAX_IMAGE_TOTAL_PIXELS}"
        )));
    }
    usize::try_from(total_u64).map_err(|_| {
        CoreError::EditFailed(format!(
            "{context} pixel count {total_u64} exceeds addressable usize range"
        ))
    })
}

/// Decode PNG bytes fail-closed with explicit `image::Limits` and total-pixel bounds.
pub(crate) fn decode_png_checked(path: &Path, bytes: &[u8]) -> Result<DynamicImage, CoreError> {
    decode_png_checked_with_limits(
        path,
        bytes,
        MAX_IMAGE_DIM,
        MAX_IMAGE_TOTAL_PIXELS,
        MAX_IMAGE_DECODE_ALLOC_BYTES,
    )
}

/// Decode PNG bytes fail-closed with caller-supplied dimension, pixel-count, and allocation limits.
pub(crate) fn decode_png_checked_with_limits(
    path: &Path,
    bytes: &[u8],
    max_dim: u32,
    max_total_pixels: u64,
    max_alloc: u64,
) -> Result<DynamicImage, CoreError> {
    if bytes.len() < PNG_SIGNATURE.len() || bytes[..PNG_SIGNATURE.len()] != PNG_SIGNATURE {
        return Err(CoreError::InvalidPng {
            path: path.to_path_buf(),
            reason: "missing valid 8-byte PNG signature".to_string(),
        });
    }

    let mut limits = Limits::default();
    limits.max_image_width = Some(max_dim);
    limits.max_image_height = Some(max_dim);
    limits.max_alloc = Some(max_alloc);

    let mut dim_reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
    dim_reader.limits(limits.clone());
    let (w, h) = dim_reader
        .into_dimensions()
        .map_err(|err| CoreError::InvalidPng {
            path: path.to_path_buf(),
            reason: format!("PNG header inspection failed: {err}"),
        })?;
    if w == 0 || h == 0 || w > max_dim || h > max_dim {
        return Err(CoreError::InvalidPng {
            path: path.to_path_buf(),
            reason: format!("PNG dimensions {w}x{h} exceed allowed bounds 1..={max_dim}"),
        });
    }
    let total_pixels =
        u64::from(w)
            .checked_mul(u64::from(h))
            .ok_or_else(|| CoreError::InvalidPng {
                path: path.to_path_buf(),
                reason: format!("PNG pixel count overflow for {w}x{h}"),
            })?;
    if total_pixels > max_total_pixels {
        return Err(CoreError::InvalidPng {
            path: path.to_path_buf(),
            reason: format!(
                "PNG total pixel count {total_pixels} ({w}x{h}) exceeds limit {max_total_pixels}"
            ),
        });
    }

    let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
    reader.limits(limits);
    reader.decode().map_err(|err| CoreError::InvalidPng {
        path: path.to_path_buf(),
        reason: format!("PNG decode failed: {err}"),
    })
}

/// Read a regular non-symlink file using `O_NOFOLLOW` on Unix up to `MAX_REGULAR_FILE_BYTES`.
pub(crate) fn read_regular_file_nofollow(
    path: &Path,
    context_label: &str,
) -> Result<Vec<u8>, CoreError> {
    read_regular_file_nofollow_bounded(path, context_label, MAX_REGULAR_FILE_BYTES)
}

/// Read a regular non-symlink file using `O_NOFOLLOW` on Unix with an explicit byte-size cap.
pub(crate) fn read_regular_file_nofollow_bounded(
    path: &Path,
    context_label: &str,
    max_bytes: u64,
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
    if lstat_meta.len() > max_bytes {
        return Err(CoreError::Io {
            context: format!("reading {context_label} at {}", path.display()),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{context_label} size ({} bytes) exceeds maximum allowed {max_bytes} bytes",
                    lstat_meta.len()
                ),
            ),
        });
    }

    #[cfg(unix)]
    {
        let file = OpenOptions::new()
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
        if meta.len() > max_bytes {
            return Err(CoreError::Io {
                context: format!("reading {context_label} at {}", path.display()),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "{context_label} size ({} bytes) exceeds maximum allowed {max_bytes} bytes",
                        meta.len()
                    ),
                ),
            });
        }
        let mut bytes = Vec::with_capacity(usize::try_from(meta.len().min(max_bytes)).unwrap_or(0));
        file.take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|err| CoreError::Io {
                context: format!("reading {context_label} at {}", path.display()),
                source: err,
            })?;
        if bytes.len() as u64 > max_bytes {
            return Err(CoreError::Io {
                context: format!("reading {context_label} at {}", path.display()),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{context_label} exceeds maximum allowed {max_bytes} bytes"),
                ),
            });
        }
        Ok(bytes)
    }

    #[cfg(not(unix))]
    {
        let file = File::open(path).map_err(|err| CoreError::Io {
            context: format!("opening {context_label} at {}", path.display()),
            source: err,
        })?;
        let meta = file.metadata().map_err(|err| CoreError::Io {
            context: format!("inspecting opened {context_label} at {}", path.display()),
            source: err,
        })?;
        if meta.len() > max_bytes {
            return Err(CoreError::Io {
                context: format!("reading {context_label} at {}", path.display()),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "{context_label} size ({} bytes) exceeds maximum allowed {max_bytes} bytes",
                        meta.len()
                    ),
                ),
            });
        }
        let mut bytes = Vec::with_capacity(usize::try_from(meta.len().min(max_bytes)).unwrap_or(0));
        file.take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|err| CoreError::Io {
                context: format!("reading {context_label} at {}", path.display()),
                source: err,
            })?;
        if bytes.len() as u64 > max_bytes {
            return Err(CoreError::Io {
                context: format!("reading {context_label} at {}", path.display()),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{context_label} exceeds maximum allowed {max_bytes} bytes"),
                ),
            });
        }
        Ok(bytes)
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

impl SeedInput {
    /// Load and validate a `SeedInput` from a regular non-symlink PNG file on disk.
    pub fn from_png_path(path: &Path, label: Option<String>) -> Result<Self, CoreError> {
        let bytes = read_regular_png_bytes_nofollow(path)?;
        let _ = decode_png_checked(path, &bytes)?;

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

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;
    use crate::calibration::{CalibrationStripLayout, SheetStructuralReport};
    use crate::subject::SubjectKind;

    fn sample_png_bytes(fill: [u8; 4]) -> Vec<u8> {
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_pixel(32, 32, Rgba(fill));
        let mut buf = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        buf
    }

    #[test]
    fn run_store_increments_versions_enforces_canonical_immutability_and_supersedes() {
        let root = TempDir::new().expect("tempdir");
        let ws = root.path();
        let runs_dir = ws.join("runs");
        let library_dir = ws.join("library");
        let run_store = RunStore::new(runs_dir);
        let lock_store = ManifestLockStore::new(&library_dir);

        // 1. Record v001
        let png1 = sample_png_bytes([30, 60, 90, 255]);
        let r1 = run_store
            .record_run(RecordRunParams {
                slug: "harbor-dock".to_string(),
                visibility: Visibility::Public,
                png_bytes: png1.clone(),
                input_hashes: BTreeMap::from([("scene.yaml".to_string(), "abc123".to_string())]),
                compiled_prompt: "[SCENE]\nid: harbor-dock".to_string(),
                backend: RunBackendMetadata {
                    name: "codex-native".to_string(),
                    width: 32,
                    height: 32,
                    raw_width: 32,
                    raw_height: 32,
                    thread_id: Some("thr_1".to_string()),
                },
                seed: None,
                qa_reports: RunQaReports::default(),
            })
            .expect("record v001");
        assert_eq!(r1.metadata.version, 1);
        assert_eq!(r1.metadata.version_label, "v001");
        assert_eq!(r1.metadata.lifecycle, Lifecycle::Draft);
        assert_eq!(r1.metadata.image_sha256, sha256_hex(&png1));

        // 2. Promote v001: draft -> candidate -> validated -> canonical
        run_store
            .transition_run_lifecycle("harbor-dock", 1, Lifecycle::Candidate)
            .unwrap();
        run_store
            .transition_run_lifecycle("harbor-dock", 1, Lifecycle::Validated)
            .unwrap();
        let (canon1, lock1) = lock_store
            .promote_run_canonical(&run_store, "harbor-dock", 1, ws)
            .expect("promote v001 to canonical");
        assert_eq!(canon1.lifecycle, Lifecycle::Canonical);
        assert_eq!(lock1.canonical["run:harbor-dock"].version, 1);
        assert!(lock1.superseded.is_empty());
        lock_store.verify_canonical_checksums(ws).unwrap();

        // 3. Direct QA mutation on canonical v001 must fail closed
        let mut_err = run_store
            .update_run_qa("harbor-dock", 1, RunQaReports::default())
            .expect_err("canonical run must be immutable");
        assert!(matches!(mut_err, CoreError::CanonicalImmutable { .. }));

        // 4. Record v002 and promote it; v001 must become superseded
        let png2 = sample_png_bytes([40, 80, 120, 255]);
        let r2 = run_store
            .record_run(RecordRunParams {
                slug: "harbor-dock".to_string(),
                visibility: Visibility::Public,
                png_bytes: png2,
                input_hashes: BTreeMap::new(),
                compiled_prompt: "[SCENE]\nid: harbor-dock".to_string(),
                backend: RunBackendMetadata {
                    name: "codex-native".to_string(),
                    width: 32,
                    height: 32,
                    raw_width: 32,
                    raw_height: 32,
                    thread_id: None,
                },
                seed: None,
                qa_reports: RunQaReports::default(),
            })
            .expect("record v002");
        assert_eq!(r2.metadata.version, 2);
        assert_eq!(r2.metadata.version_label, "v002");

        run_store
            .transition_run_lifecycle("harbor-dock", 2, Lifecycle::Candidate)
            .unwrap();
        run_store
            .transition_run_lifecycle("harbor-dock", 2, Lifecycle::Validated)
            .unwrap();
        let (canon2, lock2) = lock_store
            .promote_run_canonical(&run_store, "harbor-dock", 2, ws)
            .expect("promote v002 to canonical");
        assert_eq!(canon2.lifecycle, Lifecycle::Canonical);
        assert_eq!(lock2.canonical["run:harbor-dock"].version, 2);
        assert_eq!(lock2.superseded.len(), 1);
        assert_eq!(lock2.superseded[0].version, 1);
        assert_eq!(lock2.superseded[0].lifecycle, Lifecycle::Superseded);

        let v1_after = run_store.load_run("harbor-dock", 1).unwrap();
        assert_eq!(v1_after.lifecycle, Lifecycle::Superseded);
        lock_store.verify_canonical_checksums(ws).unwrap();

        // 5. Tampering with canonical PNG fails checksum verification fail-closed
        fs::write(&r2.image_path, sample_png_bytes([255, 0, 0, 255])).unwrap();
        let tamper_err = lock_store
            .verify_canonical_checksums(ws)
            .expect_err("tampered canonical PNG must fail checksum check");
        assert!(matches!(tamper_err, CoreError::ManifestLockInvalid(_)));
    }

    #[test]
    fn seed_input_computes_sha256_and_rejects_symlinks() {
        let dir = TempDir::new().expect("tempdir");
        let seed_path = dir.path().join("seed.png");
        let buf = sample_png_bytes([100, 120, 140, 255]);
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
    #[case::equal_version_rejected(2)]
    #[case::older_version_rejected(1)]
    fn promote_run_canonical_rejects_non_increasing_version_without_split_brain(
        #[case] target_version: u32,
    ) {
        let root = TempDir::new().expect("tempdir");
        let ws = root.path();
        let run_store = RunStore::new(ws.join("runs"));
        let lock_store = ManifestLockStore::new(&ws.join("library"));

        for v in 1..=2 {
            let rec = run_store
                .record_run(RecordRunParams {
                    slug: "harbor-dock".to_string(),
                    visibility: Visibility::Public,
                    png_bytes: sample_png_bytes([v as u8 * 20, 60, 90, 255]),
                    input_hashes: BTreeMap::new(),
                    compiled_prompt: "[SCENE]\nid: harbor-dock".to_string(),
                    backend: RunBackendMetadata {
                        name: "codex-native".to_string(),
                        width: 32,
                        height: 32,
                        raw_width: 32,
                        raw_height: 32,
                        thread_id: None,
                    },
                    seed: None,
                    qa_reports: RunQaReports::default(),
                })
                .unwrap();
            assert_eq!(rec.metadata.version, v);
            run_store
                .transition_run_lifecycle("harbor-dock", v, Lifecycle::Candidate)
                .unwrap();
            run_store
                .transition_run_lifecycle("harbor-dock", v, Lifecycle::Validated)
                .unwrap();
        }

        // Promote v002 first so canonical version is 2, while v001 remains Validated.
        let (_, lock_before) = lock_store
            .promote_run_canonical(&run_store, "harbor-dock", 2, ws)
            .unwrap();
        assert_eq!(lock_before.canonical["run:harbor-dock"].version, 2);

        // Attempting to promote v001 (or v002 again) must fail cleanly without mutating v002.
        let err = lock_store
            .promote_run_canonical(&run_store, "harbor-dock", target_version, ws)
            .expect_err("non-increasing version must fail closed");
        assert!(matches!(
            err,
            CoreError::ManifestLockInvalid(_) | CoreError::LifecycleTransitionInvalid { .. }
        ));

        let v2_after = run_store.load_run("harbor-dock", 2).unwrap();
        assert_eq!(
            v2_after.lifecycle,
            Lifecycle::Canonical,
            "v002 must remain Canonical without split-brain Superseding"
        );
        let v1_after = run_store.load_run("harbor-dock", 1).unwrap();
        assert_eq!(
            v1_after.lifecycle,
            Lifecycle::Validated,
            "v001 must remain Validated when promotion fails"
        );
        let lock_after = lock_store.load_or_init().unwrap();
        assert_eq!(lock_after, lock_before);
    }

    fn seed_validated_pack_on_disk(
        ws: &Path,
        version: u32,
        extra_artifact_key: Option<&str>,
    ) -> PathBuf {
        let pack_dir = ws.join("library/subjects/courier-ren/packs/franco-belgian-line");
        fs::create_dir_all(pack_dir.join("slices")).unwrap();
        let sheet_bytes = sample_png_bytes([40, 80, 120, 255]);
        fs::write(pack_dir.join("sheet.png"), &sheet_bytes).unwrap();

        let mut artifact_hashes = BTreeMap::new();
        artifact_hashes.insert("sheet.png".to_string(), sha256_hex(&sheet_bytes));
        if let Some(extra) = extra_artifact_key {
            artifact_hashes.insert(extra.to_string(), "deadbeef".to_string());
        }

        let manifest = ReferencePackManifest {
            schema_version: 1,
            version,
            subject_slug: "courier-ren".to_string(),
            style_slug: "franco-belgian-line".to_string(),
            subject_kind: SubjectKind::Prop,
            visibility: Visibility::Public,
            lifecycle: Lifecycle::Validated,
            seed: None,
            sheets: vec![crate::reference_pack::GeneratedSheetRecord {
                role: crate::reference_pack::ReferenceSheetRole::SingleSheet,
                filename: "sheet.png".to_string(),
                sha256: sha256_hex(&sheet_bytes),
                structural_report: SheetStructuralReport {
                    width: 32,
                    height: 32,
                    bg_coverage: 0.8,
                    border_bg_coverage: 0.9,
                    foreground_pixels: 300,
                },
                calibration_strip: CalibrationStripLayout {
                    strip_y: 24,
                    strip_height: 8,
                    cell_size_px: 4,
                    gray_wedge_cells: Vec::new(),
                    palette_swatch_cells: Vec::new(),
                },
                thread_id: None,
            }],
            slices: Vec::new(),
            artifact_hashes,
            created_at_iso: now_utc_iso(),
        };
        write_atomic(
            &pack_dir.join("pack.json"),
            &serde_json::to_vec_pretty(&manifest).unwrap(),
            "pack.json",
        )
        .unwrap();
        pack_dir
    }

    #[rstest]
    #[case::parent_dir_traversal("../outside.png")]
    #[case::nested_parent_dir_traversal("slices/../../outside.png")]
    #[case::absolute_path("/etc/passwd")]
    #[case::backslash_path("slices\\bad.png")]
    #[case::cur_dir_prefix("./sheet.png")]
    fn promote_reference_pack_canonical_rejects_unsafe_artifact_keys_without_mutating_pack_json(
        #[case] unsafe_key: &str,
    ) {
        let root = TempDir::new().expect("tempdir");
        let ws = root.path();
        fs::write(ws.join("outside.png"), sample_png_bytes([1, 2, 3, 255])).unwrap();
        let pack_dir = seed_validated_pack_on_disk(ws, 1, Some(unsafe_key));
        let lock_store = ManifestLockStore::new(&ws.join("library"));

        let err = lock_store
            .promote_reference_pack_canonical(&pack_dir, ws)
            .expect_err("unsafe artifact_hashes key must fail closed");
        assert!(matches!(err, CoreError::ManifestLockInvalid(_)));

        let persisted: ReferencePackManifest =
            serde_json::from_slice(&fs::read(pack_dir.join("pack.json")).unwrap()).unwrap();
        assert_eq!(
            persisted.lifecycle,
            Lifecycle::Validated,
            "pack.json must remain Validated when promotion fails preflight"
        );
        assert!(!lock_store.lock_path().exists());
    }

    #[rstest]
    #[case::equal_version_rejected(2)]
    #[case::older_version_rejected(1)]
    fn promote_reference_pack_canonical_rejects_non_increasing_version_without_mutating_pack_json(
        #[case] next_version: u32,
    ) {
        let root = TempDir::new().expect("tempdir");
        let ws = root.path();
        let pack_dir = seed_validated_pack_on_disk(ws, 2, None);
        let lock_store = ManifestLockStore::new(&ws.join("library"));

        let (_, lock_before) = lock_store
            .promote_reference_pack_canonical(&pack_dir, ws)
            .expect("v2 promotion succeeds");

        // Replace pack.json with a Validated v1 or v2 manifest and ensure preflight rejects it.
        seed_validated_pack_on_disk(ws, next_version, None);
        let err = lock_store
            .promote_reference_pack_canonical(&pack_dir, ws)
            .expect_err("non-increasing pack version must fail closed");
        assert!(matches!(err, CoreError::ManifestLockInvalid(_)));

        let persisted: ReferencePackManifest =
            serde_json::from_slice(&fs::read(pack_dir.join("pack.json")).unwrap()).unwrap();
        assert_eq!(persisted.lifecycle, Lifecycle::Validated);
        assert_eq!(lock_store.load_or_init().unwrap(), lock_before);
    }

    fn minimal_png_with_ihdr_dims(w: u32, h: u32) -> Vec<u8> {
        let mut png = Vec::from(PNG_SIGNATURE);
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
        png.extend_from_slice(&(13u32).to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&ihdr);
        png.extend_from_slice(&[0u8; 4]); // placeholder CRC (header dimensions read before decode)
        png
    }

    #[rstest]
    #[case::within_limits(32, 32, 64, 1024, 1024 * 1024, true)]
    #[case::exceeds_max_dim(64, 32, 32, 4096, 1024 * 1024, false)]
    #[case::exceeds_total_pixels(32, 32, 64, 512, 1024 * 1024, false)]
    #[case::exceeds_max_alloc(32, 32, 64, 4096, 512, false)]
    fn decode_png_checked_with_limits_rejects_oversized_dimensions_and_alloc(
        #[case] w: u32,
        #[case] h: u32,
        #[case] max_dim: u32,
        #[case] max_total_pixels: u64,
        #[case] max_alloc: u64,
        #[case] expect_ok: bool,
    ) {
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(w, h, Rgba([10, 20, 30, 255]));
        let mut buf = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();

        let res = decode_png_checked_with_limits(
            Path::new("test.png"),
            &buf,
            max_dim,
            max_total_pixels,
            max_alloc,
        );
        assert_eq!(res.is_ok(), expect_ok);
        if !expect_ok {
            assert!(matches!(res.unwrap_err(), CoreError::InvalidPng { .. }));
        }
    }

    #[rstest]
    #[case::width_exceeds_max_image_dim(MAX_IMAGE_DIM + 1, 16)]
    #[case::total_pixels_exceeds_cap(16_384, 16_384)]
    fn decode_png_checked_rejects_oversized_ihdr_before_allocating(#[case] w: u32, #[case] h: u32) {
        let forged = minimal_png_with_ihdr_dims(w, h);
        let err = decode_png_checked(Path::new("bomb.png"), &forged)
            .expect_err("oversized IHDR must fail closed before pixel buffer allocation");
        assert!(matches!(err, CoreError::InvalidPng { .. }));
    }

    #[rstest]
    #[case::within_cap(64, 128, true)]
    #[case::exact_cap(128, 128, true)]
    #[case::exceeds_cap(129, 128, false)]
    fn read_regular_file_nofollow_bounded_enforces_size_cap(
        #[case] file_len: usize,
        #[case] cap_bytes: u64,
        #[case] expect_ok: bool,
    ) {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("payload.bin");
        fs::write(&file_path, vec![0xAAu8; file_len]).unwrap();
        let res = read_regular_file_nofollow_bounded(&file_path, "test payload", cap_bytes);
        assert_eq!(res.is_ok(), expect_ok);
    }

    #[rstest]
    #[case::internal_run_detected(Visibility::Internal, true, Some(Visibility::Internal))]
    #[case::restricted_run_detected(Visibility::Restricted, true, Some(Visibility::Restricted))]
    #[case::outside_runs_dir_returns_none(Visibility::Internal, false, None)]
    fn inspect_run_image_visibility_detects_sibling_metadata_inside_runs_dir(
        #[case] run_vis: Visibility,
        #[case] inside_runs: bool,
        #[case] expected: Option<Visibility>,
    ) {
        let root = TempDir::new().unwrap();
        let store = RunStore::new(root.path().join("runs"));
        let recorded = store
            .record_run(RecordRunParams {
                slug: "secret-scene".to_string(),
                visibility: run_vis,
                png_bytes: sample_png_bytes([10, 20, 30, 255]),
                input_hashes: BTreeMap::new(),
                compiled_prompt: "test".to_string(),
                backend: RunBackendMetadata {
                    name: "stub".to_string(),
                    width: 32,
                    height: 32,
                    raw_width: 32,
                    raw_height: 32,
                    thread_id: None,
                },
                seed: None,
                qa_reports: RunQaReports::default(),
            })
            .unwrap();

        let target = if inside_runs {
            recorded.image_path
        } else {
            let outside = root.path().join("external.png");
            fs::write(&outside, sample_png_bytes([10, 20, 30, 255])).unwrap();
            outside
        };
        assert_eq!(
            store.inspect_run_image_visibility(&target).unwrap(),
            expected
        );
    }
}
