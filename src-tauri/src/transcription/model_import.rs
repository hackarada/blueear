//! Manual import of user-supplied model bundles.
//!
//! SECURITY-REVIEW: this module is Blue Ear's only user-controlled file
//! boundary. Everything else the app reads it wrote itself, at a path it
//! derived server-side. A model bundle is a directory a human chose, so it is
//! treated as hostile input throughout: the manifest is read with a size
//! bound, every declared path is checked to be relative and normal, the actual
//! tree is enumerated and compared against the manifest in both directions,
//! symlinks are rejected outright, every file's SHA-256 is verified while
//! streaming, and nothing is promoted until a complete copy has been verified
//! a second time from Blue Ear's own storage.
//!
//! No file from a bundle is ever executed, and no model is ever loaded from
//! the user's original path -- only from the app-owned copy.

use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};
use crate::transcription::store;
use crate::transcription::types::ProviderId;

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
const MANIFEST_FILE: &str = "manifest.json";

/// A manifest larger than this is rejected without being parsed. Real
/// manifests are a few kilobytes; the bound exists so a hostile directory
/// cannot make the app read a multi-gigabyte "JSON" file into memory.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// The single FluidAudio bundle Blue Ear accepts. Its ID is also the directory
/// the bundle is promoted into, and therefore the models root the FluidAudio
/// adapter is pointed at.
pub const FLUIDAUDIO_BUNDLE_ID: &str = "fluidaudio-v1";

/// Whisper.cpp ggml model bundle. Layout: `ggml/<model>.bin` plus manifest.
pub const WHISPER_BUNDLE_ID: &str = "whisper-v1";

/// What an allowlisted bundle must contain. Pinning this in code, rather than
/// trusting the manifest to describe itself, is what stops a well-formed but
/// unexpected bundle from installing.
pub struct AllowedBundle {
    pub bundle_id: &'static str,
    pub provider: ProviderId,
    /// Top-level directories that must be present, named exactly as the
    /// provider's loader expects. Mirrored in `FluidAudioAdapter`.
    pub required_dirs: &'static [&'static str],
    pub min_total_bytes: u64,
    pub max_total_bytes: u64,
    /// Provider SDK versions this bundle layout is known to work with.
    pub sdk_versions: &'static [&'static str],
}

const FLUIDAUDIO_ENTRY: AllowedBundle = AllowedBundle {
    bundle_id: FLUIDAUDIO_BUNDLE_ID,
    provider: ProviderId::FluidAudio,
    required_dirs: &["parakeet-tdt-0.6b-v3", "speaker-diarization-coreml"],
    // Parakeet 0.6B int8 plus the diarization pair is roughly 1 GB. The bounds
    // are wide enough for repackaging differences and tight enough that a
    // wildly wrong directory fails before anything is copied.
    min_total_bytes: 100 * 1024 * 1024,
    max_total_bytes: 4 * 1024 * 1024 * 1024,
    sdk_versions: &["0.15.5"],
};

const WHISPER_ENTRY: AllowedBundle = AllowedBundle {
    bundle_id: WHISPER_BUNDLE_ID,
    provider: ProviderId::Whisper,
    required_dirs: &["ggml"],
    // ggml-tiny ~75MB through ggml-large-v3 ~3GB
    min_total_bytes: 10 * 1024 * 1024,
    max_total_bytes: 4 * 1024 * 1024 * 1024,
    sdk_versions: &["1.7.0", "1.7.1", "1.7.2", "1.7.3", "1.7.4"],
};

#[cfg(not(test))]
pub const ALLOWLIST: &[AllowedBundle] = &[FLUIDAUDIO_ENTRY, WHISPER_ENTRY];

/// The tests add a deliberately tiny entry so the import pipeline can
/// be exercised end to end -- copy, digest, stage, promote -- against
/// kilobyte-sized fixtures. Writing and hashing a realistic gigabyte bundle a
/// dozen times would turn `cargo test` into a two-minute wait for no extra
/// coverage; the real entry's size bounds are checked directly instead.
#[cfg(test)]
pub const ALLOWLIST: &[AllowedBundle] = &[
    FLUIDAUDIO_ENTRY,
    WHISPER_ENTRY,
    AllowedBundle {
        bundle_id: tests::TEST_BUNDLE_ID,
        provider: ProviderId::FluidAudio,
        required_dirs: &["parakeet-tdt-0.6b-v3", "speaker-diarization-coreml"],
        min_total_bytes: 1,
        max_total_bytes: 1024 * 1024,
        sdk_versions: &["0.15.5"],
    },
];

fn allowlist_entry(bundle_id: &str) -> Option<&'static AllowedBundle> {
    ALLOWLIST.iter().find(|entry| entry.bundle_id == bundle_id)
}

// MARK: - Manifest

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestModel {
    pub id: String,
    pub role: String,
    pub license: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestFile {
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleManifest {
    pub schema_version: u32,
    pub bundle_id: String,
    pub display_name: String,
    pub provider: ProviderId,
    pub sdk_version: String,
    pub models: Vec<ManifestModel>,
    pub files: Vec<ManifestFile>,
}

/// A bundle that has been validated and promoted into app-owned storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledBundle {
    pub bundle_id: String,
    pub display_name: String,
    pub provider: ProviderId,
    pub sdk_version: String,
    pub models: Vec<ManifestModel>,
    pub total_bytes: u64,
    pub installed_at: DateTime<Utc>,
}

// MARK: - Public API

/// Validates a user-selected bundle directory and installs it.
///
/// The whole operation is all-or-nothing: on any failure the staging directory
/// is removed and whatever was previously installed is untouched.
pub fn import_bundle(source: &Path) -> AppResult<InstalledBundle> {
    let manifest = read_manifest(source).map_err(|reason| {
        log::error!("model bundle manifest rejected: {reason}");
        AppError::transcription_invalid_bundle()
    })?;

    let entry = allowlist_entry(&manifest.bundle_id).ok_or_else(|| {
        log::error!("model bundle rejected: bundle id is not allowlisted");
        AppError::transcription_invalid_bundle()
    })?;

    validate_manifest(&manifest, entry).map_err(|reason| {
        log::error!("model bundle rejected: {reason}");
        AppError::transcription_invalid_bundle()
    })?;

    verify_tree(source, &manifest).map_err(|reason| {
        log::error!("model bundle rejected: {reason}");
        AppError::transcription_invalid_bundle()
    })?;

    let total_bytes = manifest.files.iter().map(|f| f.size_bytes).sum();
    let installed = InstalledBundle {
        bundle_id: manifest.bundle_id.clone(),
        display_name: manifest.display_name.clone(),
        provider: manifest.provider,
        sdk_version: manifest.sdk_version.clone(),
        models: manifest.models.clone(),
        total_bytes,
        installed_at: Utc::now(),
    };

    stage_and_promote(source, &manifest).map_err(|reason| {
        log::error!("model bundle install failed: {reason}");
        AppError::transcription_invalid_bundle()
    })?;

    Ok(installed)
}

pub fn list_installed_bundles() -> Vec<InstalledBundle> {
    let root = store::models_root();
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };

    let mut bundles: Vec<InstalledBundle> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter_map(|path| {
            let manifest: BundleManifest = read_json_file(&path.join(MANIFEST_FILE)).ok()?;
            allowlist_entry(&manifest.bundle_id)?;
            let installed_at = fs::metadata(&path)
                .and_then(|m| m.modified())
                .map(DateTime::<Utc>::from)
                .unwrap_or_else(|_| Utc::now());
            Some(InstalledBundle {
                total_bytes: manifest.files.iter().map(|f| f.size_bytes).sum(),
                bundle_id: manifest.bundle_id,
                display_name: manifest.display_name,
                provider: manifest.provider,
                sdk_version: manifest.sdk_version,
                models: manifest.models,
                installed_at,
            })
        })
        .collect();

    bundles.sort_by(|a, b| a.bundle_id.cmp(&b.bundle_id));
    bundles
}

/// Deletes an installed bundle. Only allowlisted IDs are accepted, so the
/// bundle ID coming from the frontend can never name an arbitrary directory.
pub fn delete_bundle(bundle_id: &str) -> AppResult<()> {
    // SECURITY-REVIEW: `bundle_id` originates in the webview. Resolving it
    // through the allowlist rather than joining it onto a path is what makes
    // path traversal impossible here.
    let entry = allowlist_entry(bundle_id).ok_or_else(AppError::transcription_invalid_bundle)?;
    let path = bundle_root(entry.bundle_id);
    if !path.exists() {
        return Ok(());
    }
    fs::remove_dir_all(&path).map_err(|_| AppError::internal("remove model bundle"))
}

/// Where a provider's models live once installed.
pub fn bundle_root(bundle_id: &str) -> PathBuf {
    store::models_root().join(bundle_id)
}

// MARK: - Validation

fn read_manifest(source: &Path) -> Result<BundleManifest, String> {
    if !source.is_dir() {
        return Err("source is not a directory".to_string());
    }
    let path = source.join(MANIFEST_FILE);
    let metadata = fs::symlink_metadata(&path).map_err(|_| "manifest.json is missing".to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("manifest.json is a symlink".to_string());
    }
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err("manifest.json exceeds the size bound".to_string());
    }

    let mut buffer = Vec::with_capacity(metadata.len() as usize);
    fs::File::open(&path)
        .map_err(|_| "manifest.json could not be opened".to_string())?
        .take(MAX_MANIFEST_BYTES)
        .read_to_end(&mut buffer)
        .map_err(|_| "manifest.json could not be read".to_string())?;

    serde_json::from_slice(&buffer).map_err(|_| "manifest.json is not valid".to_string())
}

fn validate_manifest(manifest: &BundleManifest, entry: &AllowedBundle) -> Result<(), String> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err("unsupported manifest schema version".to_string());
    }
    if manifest.provider != entry.provider {
        return Err("manifest provider does not match the allowlist entry".to_string());
    }
    if !entry.sdk_versions.contains(&manifest.sdk_version.as_str()) {
        return Err("manifest declares an incompatible provider SDK version".to_string());
    }
    if manifest.files.is_empty() {
        return Err("manifest declares no files".to_string());
    }

    let mut seen: Vec<&str> = Vec::with_capacity(manifest.files.len());
    for file in &manifest.files {
        validate_relative_path(&file.path)?;
        if !is_hex_sha256(&file.sha256) {
            return Err("manifest contains a malformed digest".to_string());
        }
        if seen.contains(&file.path.as_str()) {
            return Err("manifest declares the same path twice".to_string());
        }
        seen.push(&file.path);
    }

    let total: u64 = manifest.files.iter().map(|f| f.size_bytes).sum();
    if total < entry.min_total_bytes || total > entry.max_total_bytes {
        return Err("manifest total size is outside the expected bounds".to_string());
    }

    for required in entry.required_dirs {
        let prefix = format!("{required}/");
        if !manifest.files.iter().any(|f| f.path.starts_with(&prefix)) {
            return Err("manifest is missing a required model directory".to_string());
        }
    }

    Ok(())
}

/// Rejects anything that is not a plain relative path built from ordinary
/// components: absolute paths, `..`, `.`, root and prefix components, and
/// empty strings. Without this, a manifest could name `../../../../etc/passwd`
/// and the copy step would happily follow it.
fn validate_relative_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("manifest contains an empty path".to_string());
    }
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return Err("manifest contains an absolute path".to_string());
    }
    for component in candidate.components() {
        match component {
            Component::Normal(part) if !part.is_empty() => {}
            _ => return Err("manifest contains a non-relative path component".to_string()),
        }
    }
    Ok(())
}

fn is_hex_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Compares the manifest against the real directory in both directions: every
/// declared file must exist with the declared size and digest, and every file
/// on disk must be declared. The second direction matters as much as the
/// first, because it is what stops a bundle from smuggling extra payload into
/// app-owned storage alongside legitimate models.
fn verify_tree(source: &Path, manifest: &BundleManifest) -> Result<(), String> {
    let mut declared: Vec<String> = manifest.files.iter().map(|f| f.path.clone()).collect();
    declared.push(MANIFEST_FILE.to_string());
    declared.sort();

    let mut actual = Vec::new();
    collect_files(source, source, &mut actual)?;
    actual.sort();

    if actual != declared {
        return Err("bundle contents do not match the manifest exactly".to_string());
    }

    for file in &manifest.files {
        let path = source.join(&file.path);
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| "a declared file is missing".to_string())?;
        if metadata.len() != file.size_bytes {
            return Err("a declared file has an unexpected size".to_string());
        }
        let digest = sha256_file(&path).map_err(|_| "a declared file could not be read".to_string())?;
        if digest != file.sha256.to_ascii_lowercase() {
            return Err("a declared file failed digest verification".to_string());
        }
    }

    Ok(())
}

/// Walks the tree, rejecting symlinks and anything that is neither a regular
/// file nor a directory. Symlinks are refused rather than skipped: a bundle
/// containing one is malformed, and following one would let a copy escape the
/// directory the user actually chose.
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|_| "bundle directory could not be read".to_string())?;
    for entry in entries {
        let entry = entry.map_err(|_| "bundle directory could not be read".to_string())?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|_| "bundle entry could not be inspected".to_string())?;

        if file_type.is_symlink() {
            return Err("bundle contains a symlink".to_string());
        }
        if file_type.is_dir() {
            collect_files(root, &path, out)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| "bundle contains an unreachable path".to_string())?;
            out.push(relative.to_string_lossy().into_owned());
        } else {
            return Err("bundle contains an unsupported file type".to_string());
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

// MARK: - Staging and promotion

/// Copies the verified bundle into a staging directory, re-verifies the copy,
/// and only then renames it into place.
///
/// The second verification pass is not paranoia for its own sake: the copy is
/// what will actually be loaded, and a truncated write from a full disk would
/// otherwise install a bundle that passed validation and cannot be used.
fn stage_and_promote(source: &Path, manifest: &BundleManifest) -> Result<(), String> {
    let root = store::models_root();
    fs::create_dir_all(&root).map_err(|_| "models directory could not be created".to_string())?;

    let staging = root.join(format!(".staging-{}", uuid::Uuid::new_v4()));
    let result = copy_and_verify(source, &staging, manifest);

    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
        return result;
    }

    let destination = root.join(&manifest.bundle_id);
    // Replacing an existing install is two steps rather than one, since
    // `rename` onto a non-empty directory fails. A crash between them leaves
    // no bundle rather than a half-merged one, and re-importing recovers.
    if destination.exists() {
        let retired = root.join(format!(".staging-retired-{}", uuid::Uuid::new_v4()));
        fs::rename(&destination, &retired)
            .map_err(|_| "the previous bundle could not be replaced".to_string())?;
        let _ = fs::remove_dir_all(&retired);
    }

    fs::rename(&staging, &destination).map_err(|_| {
        let _ = fs::remove_dir_all(&staging);
        "the bundle could not be promoted".to_string()
    })
}

fn copy_and_verify(source: &Path, staging: &Path, manifest: &BundleManifest) -> Result<(), String> {
    fs::create_dir_all(staging)
        .map_err(|_| "staging directory could not be created".to_string())?;

    for file in &manifest.files {
        let target = staging.join(&file.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|_| "staging directory could not be created".to_string())?;
        }
        fs::copy(source.join(&file.path), &target)
            .map_err(|_| "a bundle file could not be copied".to_string())?;
    }

    fs::copy(source.join(MANIFEST_FILE), staging.join(MANIFEST_FILE))
        .map_err(|_| "the manifest could not be copied".to_string())?;

    for file in &manifest.files {
        let digest = sha256_file(&staging.join(&file.path))
            .map_err(|_| "a copied file could not be read".to_string())?;
        if digest != file.sha256.to_ascii_lowercase() {
            return Err("a copied file failed digest verification".to_string());
        }
    }

    Ok(())
}

fn read_json_file<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<T> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::HOME_ENV_LOCK;
    use tempfile::{tempdir, TempDir};

    pub const TEST_BUNDLE_ID: &str = "blueear-test-bundle";

    struct BundleFixture {
        dir: TempDir,
        manifest: BundleManifest,
    }

    fn build_bundle() -> BundleFixture {
        let dir = tempdir().unwrap();
        let files = [
            "parakeet-tdt-0.6b-v3/Preprocessor.mlmodelc/model.mil",
            "parakeet-tdt-0.6b-v3/parakeet_vocab.json",
            "speaker-diarization-coreml/pyannote_segmentation.mlmodelc/model.mil",
        ];

        let mut manifest_files = Vec::new();
        for relative in files {
            let path = dir.path().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let content: Vec<u8> = relative.bytes().cycle().take(4096).collect();
            fs::write(&path, &content).unwrap();
            manifest_files.push(ManifestFile {
                path: relative.to_string(),
                size_bytes: content.len() as u64,
                sha256: sha256_file(&path).unwrap(),
            });
        }

        let manifest = BundleManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            bundle_id: TEST_BUNDLE_ID.to_string(),
            display_name: "Parakeet v3 and diarizer".to_string(),
            provider: ProviderId::FluidAudio,
            sdk_version: "0.15.5".to_string(),
            models: vec![ManifestModel {
                id: "parakeet-tdt-0.6b-v3".to_string(),
                role: "asr".to_string(),
                license: "CC-BY-4.0".to_string(),
            }],
            files: manifest_files,
        };
        write_manifest(dir.path(), &manifest);

        BundleFixture { dir, manifest }
    }

    fn synthetic_manifest(total_bytes: u64) -> BundleManifest {
        BundleManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            bundle_id: FLUIDAUDIO_BUNDLE_ID.to_string(),
            display_name: "Synthetic".to_string(),
            provider: ProviderId::FluidAudio,
            sdk_version: "0.15.5".to_string(),
            models: vec![],
            files: vec![
                ManifestFile {
                    path: "parakeet-tdt-0.6b-v3/Preprocessor.mlmodelc/model.mil".to_string(),
                    size_bytes: total_bytes,
                    sha256: "0".repeat(64),
                },
                ManifestFile {
                    path: "speaker-diarization-coreml/pyannote_segmentation.mlmodelc/model.mil"
                        .to_string(),
                    size_bytes: 0,
                    sha256: "0".repeat(64),
                },
            ],
        }
    }

    fn write_manifest(dir: &Path, manifest: &BundleManifest) {
        fs::write(
            dir.join(MANIFEST_FILE),
            serde_json::to_vec_pretty(manifest).unwrap(),
        )
        .unwrap();
    }

    fn with_home<T>(f: impl FnOnce() -> T) -> T {
        let _guard = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let home = tempdir().unwrap();
        std::env::set_var("HOME", home.path());
        f()
    }

    #[test]
    fn a_well_formed_bundle_installs_and_is_listed() {
        with_home(|| {
            let fixture = build_bundle();
            let installed = import_bundle(fixture.dir.path()).unwrap();

            assert_eq!(installed.bundle_id, TEST_BUNDLE_ID);
            assert_eq!(installed.provider, ProviderId::FluidAudio);

            let root = bundle_root(TEST_BUNDLE_ID);
            assert!(root.join("parakeet-tdt-0.6b-v3").is_dir());
            assert!(root.join("speaker-diarization-coreml").is_dir());
            assert!(root.join(MANIFEST_FILE).is_file());

            let listed = list_installed_bundles();
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].bundle_id, TEST_BUNDLE_ID);
        });
    }

    #[test]
    fn importing_twice_replaces_the_previous_install_atomically() {
        with_home(|| {
            let fixture = build_bundle();
            import_bundle(fixture.dir.path()).unwrap();
            import_bundle(fixture.dir.path()).unwrap();

            assert_eq!(list_installed_bundles().len(), 1);
            let leftovers: Vec<_> = fs::read_dir(store::models_root())
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with(".staging"))
                .collect();
            assert!(leftovers.is_empty(), "staging directories must not survive");
        });
    }

    #[test]
    fn a_bundle_whose_id_is_not_allowlisted_is_rejected() {
        with_home(|| {
            let fixture = build_bundle();
            let mut manifest = fixture.manifest.clone();
            manifest.bundle_id = "something-else".to_string();
            write_manifest(fixture.dir.path(), &manifest);

            assert!(import_bundle(fixture.dir.path()).is_err());
            assert!(list_installed_bundles().is_empty());
        });
    }

    #[test]
    fn a_tampered_file_fails_digest_verification_and_installs_nothing() {
        with_home(|| {
            let fixture = build_bundle();
            let victim = fixture
                .dir
                .path()
                .join("parakeet-tdt-0.6b-v3/parakeet_vocab.json");
            let mut content = fs::read(&victim).unwrap();
            content[0] ^= 0xff;
            fs::write(&victim, content).unwrap();

            assert!(import_bundle(fixture.dir.path()).is_err());
            assert!(!bundle_root(TEST_BUNDLE_ID).exists());
        });
    }

    #[test]
    fn an_undeclared_extra_file_is_rejected() {
        with_home(|| {
            let fixture = build_bundle();
            fs::write(fixture.dir.path().join("stowaway.sh"), b"#!/bin/sh\n").unwrap();

            assert!(import_bundle(fixture.dir.path()).is_err());
            assert!(!bundle_root(TEST_BUNDLE_ID).exists());
        });
    }

    #[test]
    fn a_symlink_anywhere_in_the_tree_is_rejected() {
        with_home(|| {
            let fixture = build_bundle();
            std::os::unix::fs::symlink(
                "/etc/passwd",
                fixture.dir.path().join("parakeet-tdt-0.6b-v3/link"),
            )
            .unwrap();

            assert!(import_bundle(fixture.dir.path()).is_err());
            assert!(!bundle_root(TEST_BUNDLE_ID).exists());
        });
    }

    #[test]
    fn traversal_and_absolute_paths_are_rejected_before_anything_is_copied() {
        for path in [
            "../escape.bin",
            "/etc/passwd",
            "parakeet-tdt-0.6b-v3/../../escape.bin",
            "./relative.bin",
            "",
        ] {
            assert!(
                validate_relative_path(path).is_err(),
                "should have rejected {path:?}"
            );
        }
        assert!(validate_relative_path("a/b/c.mil").is_ok());
    }

    #[test]
    fn a_manifest_declaring_a_traversal_path_is_rejected() {
        with_home(|| {
            let fixture = build_bundle();
            let mut manifest = fixture.manifest.clone();
            manifest.files[0].path = "../escape.bin".to_string();
            write_manifest(fixture.dir.path(), &manifest);

            assert!(import_bundle(fixture.dir.path()).is_err());
        });
    }

    #[test]
    fn a_duplicate_declared_path_is_rejected() {
        with_home(|| {
            let fixture = build_bundle();
            let mut manifest = fixture.manifest.clone();
            let duplicate = manifest.files[0].clone();
            manifest.files.push(duplicate);
            write_manifest(fixture.dir.path(), &manifest);

            assert!(import_bundle(fixture.dir.path()).is_err());
        });
    }

    #[test]
    fn a_bundle_missing_a_required_model_directory_is_rejected() {
        with_home(|| {
            let fixture = build_bundle();
            let mut manifest = fixture.manifest.clone();
            manifest
                .files
                .retain(|f| !f.path.starts_with("speaker-diarization-coreml/"));
            write_manifest(fixture.dir.path(), &manifest);
            fs::remove_dir_all(fixture.dir.path().join("speaker-diarization-coreml")).unwrap();

            assert!(import_bundle(fixture.dir.path()).is_err());
        });
    }

    /// Exercises the real FluidAudio entry's bounds directly, since the
    /// end-to-end tests deliberately run against the tiny test entry.
    #[test]
    fn the_fluidaudio_entry_rejects_implausible_total_sizes() {
        let entry = allowlist_entry(FLUIDAUDIO_BUNDLE_ID).unwrap();
        assert!(validate_manifest(&synthetic_manifest(1024), entry).is_err());
        assert!(validate_manifest(&synthetic_manifest(8 * 1024 * 1024 * 1024), entry).is_err());
        assert!(validate_manifest(&synthetic_manifest(900 * 1024 * 1024), entry).is_ok());
    }

    #[test]
    fn a_declared_size_that_disagrees_with_the_file_on_disk_is_rejected() {
        with_home(|| {
            let fixture = build_bundle();
            let mut manifest = fixture.manifest.clone();
            manifest.files[0].size_bytes += 1;
            write_manifest(fixture.dir.path(), &manifest);

            assert!(import_bundle(fixture.dir.path()).is_err());
            assert!(!bundle_root(TEST_BUNDLE_ID).exists());
        });
    }

    #[test]
    fn an_incompatible_sdk_version_is_rejected() {
        with_home(|| {
            let fixture = build_bundle();
            let mut manifest = fixture.manifest.clone();
            manifest.sdk_version = "0.99.0".to_string();
            write_manifest(fixture.dir.path(), &manifest);

            assert!(import_bundle(fixture.dir.path()).is_err());
        });
    }

    #[test]
    fn an_oversized_manifest_is_rejected_without_being_parsed() {
        with_home(|| {
            let fixture = build_bundle();
            fs::write(
                fixture.dir.path().join(MANIFEST_FILE),
                vec![b'{'; (MAX_MANIFEST_BYTES + 1) as usize],
            )
            .unwrap();

            assert!(import_bundle(fixture.dir.path()).is_err());
        });
    }

    #[test]
    fn deleting_a_bundle_only_accepts_allowlisted_ids() {
        with_home(|| {
            let fixture = build_bundle();
            import_bundle(fixture.dir.path()).unwrap();

            assert!(delete_bundle("../../../etc").is_err());
            assert!(delete_bundle(TEST_BUNDLE_ID).is_ok());
            assert!(list_installed_bundles().is_empty());
            // Deleting something already gone is not an error.
            assert!(delete_bundle(TEST_BUNDLE_ID).is_ok());
        });
    }

    #[test]
    fn interrupted_imports_leave_staging_directories_that_startup_cleans_up() {
        with_home(|| {
            let staging = store::models_root().join(".staging-abandoned");
            fs::create_dir_all(&staging).unwrap();
            fs::write(staging.join("partial.bin"), b"x").unwrap();

            store::clean_model_staging_dirs();
            assert!(!staging.exists());
        });
    }
}
