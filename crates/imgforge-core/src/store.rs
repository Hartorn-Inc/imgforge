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
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::backend::PNG_SIGNATURE;
use crate::critic::CriticReport;
use crate::error::CoreError;
use crate::lifecycle::{
    Lifecycle, SeedInput, now_utc_iso, read_regular_file_nofollow, sha256_hex, validate_slug,
};
use crate::reference_pack::ReferencePackManifest;
use crate::scene_spec::Visibility;

/// Supported `RunMetadata` and `ManifestLock` schema version.
pub const SUPPORTED_STORE_SCHEMA_VERSION: u32 = 1;

/// Canonical filename for the workspace canonical lockfile under `library/`.
pub const MANIFEST_LOCK_FILENAME: &str = "manifest.lock.json";

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
        let slug_dir = self.runs_dir.join(slug);
        match fs::symlink_metadata(&slug_dir) {
            Ok(meta) => {
                if meta.file_type().is_symlink() || !meta.is_dir() {
                    return Err(CoreError::Io {
                        context: format!("inspecting Run directory {}", slug_dir.display()),
                        source: std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "Run directory must not be a symlink",
                        ),
                    });
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(1),
            Err(err) => {
                return Err(CoreError::Io {
                    context: format!("inspecting Run directory {}", slug_dir.display()),
                    source: err,
                });
            }
        }

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

        let slug_dir = self.runs_dir.join(&params.slug);
        fs::create_dir_all(&slug_dir).map_err(|err| CoreError::Io {
            context: format!("creating Run directory {}", slug_dir.display()),
            source: err,
        })?;

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
        let meta_path = self
            .runs_dir
            .join(slug)
            .join(format!("{}.meta.json", Self::format_version_label(version)));
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
        let slug_dir = self.runs_dir.join(slug);
        if !slug_dir.exists() {
            return Ok(Vec::new());
        }
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

/// Canonical entity classification recorded in `library/manifest.lock.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestEntityKind {
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
    /// Kind of locked entity.
    pub entity_kind: ManifestEntityKind,
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

/// Atomic lockfile (`library/manifest.lock.json`) freezing all `canonical` entities and `ReferencePack`s.
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

    /// Atomically promote a `canonical` entry into `manifest.lock.json`, marking any prior
    /// `canonical` entry for the same `key` as `superseded`.
    pub fn promote_canonical(&self, entry: ManifestEntry) -> Result<ManifestLock, CoreError> {
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
        self.save_atomic(&lock)?;
        Ok(lock)
    }

    /// Promote a `Validated` `Run` to `Canonical`, mark any prior `Canonical` version of the
    /// same `Run` stream `Superseded` on disk and in `manifest.lock.json`, and lock SHA-256 hashes.
    pub fn promote_run_canonical(
        &self,
        run_store: &RunStore,
        slug: &str,
        version: u32,
        workspace_root: &Path,
    ) -> Result<(RunMetadata, ManifestLock), CoreError> {
        let current = run_store.load_run(slug, version)?;
        if current.lifecycle != Lifecycle::Validated {
            let _ = current.lifecycle.transition_to(Lifecycle::Canonical)?;
        }

        let image_path = run_store.run_image_path(slug, version);
        let meta_path = run_store.run_meta_path(slug, version);

        // Supersede any prior canonical entry for this run stream before promoting the new one.
        let lock = self.load_or_init()?;
        let lock_key = format!("run:{slug}");
        if let Some(prior_entry) = lock.canonical.get(&lock_key)
            && prior_entry.version != version
        {
            let _ = run_store.transition_run_lifecycle(
                slug,
                prior_entry.version,
                Lifecycle::Superseded,
            )?;
        }

        let promoted_meta =
            run_store.transition_run_lifecycle(slug, version, Lifecycle::Canonical)?;

        let img_bytes = read_regular_file_nofollow(&image_path, "Run PNG")?;
        let meta_bytes = read_regular_file_nofollow(&meta_path, "Run metadata")?;

        let rel_img = relative_workspace_key(workspace_root, &image_path)?;
        let rel_meta = relative_workspace_key(workspace_root, &meta_path)?;

        let mut artifact_hashes = BTreeMap::new();
        artifact_hashes.insert(rel_img, sha256_hex(&img_bytes));
        artifact_hashes.insert(rel_meta, sha256_hex(&meta_bytes));

        let updated_lock = self.promote_canonical(ManifestEntry {
            entity_kind: ManifestEntityKind::Run,
            key: lock_key,
            version: promoted_meta.version,
            version_label: promoted_meta.version_label.clone(),
            lifecycle: Lifecycle::Canonical,
            visibility: promoted_meta.visibility,
            artifact_hashes,
            promoted_at_iso: now_utc_iso(),
        })?;

        Ok((promoted_meta, updated_lock))
    }

    /// Promote a `Validated` style-scoped `ReferencePack` (`pack.json`) to `Canonical`
    /// and record SHA-256 checksums for all sheets, slices, and manifests in `manifest.lock.json`.
    pub fn promote_reference_pack_canonical(
        &self,
        pack_dir: &Path,
        workspace_root: &Path,
    ) -> Result<(ReferencePackManifest, ManifestLock), CoreError> {
        let pack_path = pack_dir.join("pack.json");
        let raw = read_regular_file_nofollow(&pack_path, "pack.json")?;
        let mut manifest: ReferencePackManifest = serde_json::from_slice(&raw)
            .map_err(|err| CoreError::ManifestLockInvalid(err.to_string()))?;

        manifest.lifecycle = manifest.lifecycle.transition_to(Lifecycle::Canonical)?;
        let updated_pack_bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(|err| CoreError::ManifestLockInvalid(err.to_string()))?;
        write_atomic(&pack_path, &updated_pack_bytes, "pack.json")?;

        let mut artifact_hashes = BTreeMap::new();
        artifact_hashes.insert(
            relative_workspace_key(workspace_root, &pack_path)?,
            sha256_hex(&updated_pack_bytes),
        );
        for rel_name in manifest.artifact_hashes.keys() {
            let full = pack_dir.join(rel_name);
            let bytes = read_regular_file_nofollow(&full, rel_name)?;
            artifact_hashes.insert(
                relative_workspace_key(workspace_root, &full)?,
                sha256_hex(&bytes),
            );
        }

        let lock_key = format!("pack:{}/{}", manifest.subject_slug, manifest.style_slug);
        let version_label = RunStore::format_version_label(manifest.version);
        let updated_lock = self.promote_canonical(ManifestEntry {
            entity_kind: ManifestEntityKind::ReferencePack,
            key: lock_key,
            version: manifest.version,
            version_label,
            lifecycle: Lifecycle::Canonical,
            visibility: manifest.visibility,
            artifact_hashes,
            promoted_at_iso: now_utc_iso(),
        })?;

        Ok((manifest, updated_lock))
    }

    /// Verify that every artifact in `manifest.lock.json`'s `canonical` table exists on disk
    /// as a regular non-symlink file inside `workspace_root` with matching SHA-256 checksum.
    pub fn verify_canonical_checksums(&self, workspace_root: &Path) -> Result<(), CoreError> {
        let lock = self.load_or_init()?;
        let canonical_ws = fs::canonicalize(workspace_root).map_err(|err| CoreError::Io {
            context: format!("canonicalizing workspace {}", workspace_root.display()),
            source: err,
        })?;

        for (key, entry) in &lock.canonical {
            for (rel_str, expected_hash) in &entry.artifact_hashes {
                let rel_path = Path::new(rel_str);
                if rel_path.is_absolute()
                    || rel_path
                        .components()
                        .any(|c| matches!(c, Component::ParentDir))
                {
                    return Err(CoreError::ManifestLockInvalid(format!(
                        "canonical entry '{key}' has unsafe path '{rel_str}'"
                    )));
                }
                let full_path = workspace_root.join(rel_path);
                let canonical_file = fs::canonicalize(&full_path).map_err(|err| {
                    CoreError::ManifestLockInvalid(format!(
                        "missing canonical artifact '{}' for '{key}': {err}",
                        full_path.display()
                    ))
                })?;
                if !canonical_file.starts_with(&canonical_ws) {
                    return Err(CoreError::ManifestLockInvalid(format!(
                        "canonical artifact '{rel_str}' escapes workspace root"
                    )));
                }
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

    fn save_atomic(&self, lock: &ManifestLock) -> Result<(), CoreError> {
        let bytes = serde_json::to_vec_pretty(lock)
            .map_err(|err| CoreError::ManifestLockInvalid(err.to_string()))?;
        write_atomic(&self.lock_path, &bytes, "manifest.lock.json")
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
pub(crate) fn write_atomic(dest: &Path, bytes: &[u8], label: &str) -> Result<(), CoreError> {
    let parent = dest.parent().ok_or_else(|| CoreError::Io {
        context: format!(
            "resolving parent directory for {label} at {}",
            dest.display()
        ),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing parent directory"),
    })?;
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
    use tempfile::TempDir;

    use super::*;

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
}
