//! Atomic, lossless top-level frontmatter patches for built-in modules.
//!
//! Computation engines never receive filesystem access. They return a patch,
//! and the module runner applies that patch here after verifying the exact
//! source hash it evaluated.

use std::collections::{BTreeMap, BTreeSet, HashMap};
#[cfg(unix)]
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Component, Path};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use yaml_edit::{Document, YamlNode};

use crate::error::{Error, Result};
use crate::index::state::Index;
use crate::index::types::{
    ComputedDependencySnapshot, ComputedFieldDiagnostic, ComputedFieldEntry,
};
#[cfg(any(not(unix), test))]
use crate::parser::parse_markdown_file;
use crate::parser::{compute_content_hash, MarkdownFile};

const COMPUTED_INTENT_VERSION: u32 = 1;
const COMPUTED_INTENT_FILE: &str = "computed-write-intent.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IntentComputedFieldEntry {
    module: String,
    definition_fingerprint: String,
    input_fingerprint: Option<String>,
    #[serde(default)]
    dependency_snapshot: ComputedDependencySnapshot,
    value_json: Option<String>,
    #[serde(default)]
    materialized_value_json: Option<String>,
    diagnostic: Option<ComputedFieldDiagnostic>,
}

impl From<&ComputedFieldEntry> for IntentComputedFieldEntry {
    fn from(entry: &ComputedFieldEntry) -> Self {
        Self {
            module: entry.module.clone(),
            definition_fingerprint: entry.definition_fingerprint.clone(),
            input_fingerprint: entry.input_fingerprint.clone(),
            dependency_snapshot: entry.dependency_snapshot.clone(),
            value_json: entry.value_json.clone(),
            materialized_value_json: entry.materialized_value_json.clone(),
            diagnostic: entry.diagnostic.clone(),
        }
    }
}

impl From<IntentComputedFieldEntry> for ComputedFieldEntry {
    fn from(entry: IntentComputedFieldEntry) -> Self {
        Self {
            module: entry.module,
            definition_fingerprint: entry.definition_fingerprint,
            input_fingerprint: entry.input_fingerprint,
            dependency_snapshot: entry.dependency_snapshot,
            value_json: entry.value_json,
            materialized_value_json: entry.materialized_value_json,
            diagnostic: entry.diagnostic,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ComputedWriteIntent {
    before_content_hash: String,
    after_content_hash: String,
    #[serde(default)]
    after_file_identity: Option<ComputedFileIdentity>,
    fields: BTreeMap<String, IntentComputedFieldEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ComputedFileIdentity {
    device: u64,
    inode: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ComputedWriteIntentLog {
    version: u32,
    entries: BTreeMap<String, ComputedWriteIntent>,
}

fn computed_intent_path(project_root: &Path) -> std::path::PathBuf {
    project_root.join(".markdownvdb").join(COMPUTED_INTENT_FILE)
}

fn validate_relative_path(relative_path: &Path) -> Result<()> {
    if relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(Error::Config(format!(
            "module source path must stay inside the project: {}",
            relative_path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn rustix_io(error: rustix::io::Errno) -> Error {
    Error::Io(std::io::Error::from_raw_os_error(error.raw_os_error()))
}

/// One stable filesystem boundary for a complete computed source write.
/// Descendant source, intent-state, and lock opens are all relative to this
/// descriptor so retargeting the caller's root pathname cannot split them.
#[cfg(unix)]
struct SecureProjectRoot {
    directory: std::fs::File,
}

#[cfg(unix)]
impl SecureProjectRoot {
    fn open(project_root: &Path) -> Result<Self> {
        let directory = std::fs::File::open(project_root)?;
        if !directory.metadata()?.is_dir() {
            return Err(Error::Config(format!(
                "project root is not a directory: {}",
                project_root.display()
            )));
        }
        Ok(Self { directory })
    }

    fn identity(&self) -> Result<ComputedFileIdentity> {
        use std::os::unix::fs::MetadataExt;

        let metadata = self.directory.metadata()?;
        Ok(ComputedFileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

/// Identity captured from the exact root descriptor used to open the module
/// lock. Every later pathname-based operation must still resolve to this root.
pub(crate) struct ProjectRootGuard {
    #[cfg(unix)]
    identity: ComputedFileIdentity,
    #[cfg(not(unix))]
    canonical_path: std::path::PathBuf,
}

impl ProjectRootGuard {
    pub(crate) fn verify(&self, project_root: &Path) -> Result<()> {
        #[cfg(unix)]
        let matches = SecureProjectRoot::open(project_root)?.identity()? == self.identity;
        #[cfg(not(unix))]
        let matches = std::fs::canonicalize(project_root)? == self.canonical_path;

        if !matches {
            return Err(Error::Config(format!(
                "project root changed while the computed-module lock was held: {}",
                project_root.display()
            )));
        }
        Ok(())
    }
}

/// A source parent opened component-by-component beneath the captured project
/// root. Every descendant open rejects symlinks and all subsequent source CAS
/// reads and renames are relative to this stable directory descriptor.
#[cfg(unix)]
struct SecureSourceParent {
    directory: std::fs::File,
    file_name: OsString,
}

#[cfg(unix)]
struct SecureSourceSnapshot {
    source: String,
    permissions: std::fs::Permissions,
    identity: ComputedFileIdentity,
    modified_at: u64,
}

#[cfg(unix)]
impl SecureSourceParent {
    fn open(project_root: &SecureProjectRoot, relative_path: &Path) -> Result<Self> {
        use rustix::fs::{openat, Mode, OFlags};

        let mut directory = project_root.directory.try_clone()?;
        if let Some(parent) = relative_path.parent() {
            for component in parent.components() {
                match component {
                    Component::CurDir => {}
                    Component::Normal(name) => {
                        let descriptor = openat(
                            &directory,
                            name,
                            OFlags::RDONLY
                                | OFlags::DIRECTORY
                                | OFlags::NOFOLLOW
                                | OFlags::CLOEXEC,
                            Mode::empty(),
                        )
                        .map_err(|error| {
                            if error == rustix::io::Errno::NOENT {
                                Error::Io(std::io::Error::from_raw_os_error(
                                    error.raw_os_error(),
                                ))
                            } else {
                                Error::Config(format!(
                                    "refusing computed write through an ancestor outside project or symlink `{}`: {error}",
                                    relative_path.display()
                                ))
                            }
                        })?;
                        directory = std::fs::File::from(descriptor);
                    }
                    _ => {
                        return Err(Error::Config(format!(
                            "module source path must stay inside the project: {}",
                            relative_path.display()
                        )));
                    }
                }
            }
        }
        let file_name = relative_path.file_name().ok_or_else(|| {
            Error::Config(format!(
                "module source has no file name: {}",
                relative_path.display()
            ))
        })?;
        Ok(Self {
            directory,
            file_name: file_name.to_os_string(),
        })
    }

    fn read_source(&self, relative_path: &Path) -> Result<SecureSourceSnapshot> {
        use std::os::unix::fs::MetadataExt;

        use rustix::fs::{openat, Mode, OFlags};

        let descriptor = openat(
            &self.directory,
            &self.file_name,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            if error == rustix::io::Errno::NOENT {
                Error::Io(std::io::Error::from_raw_os_error(error.raw_os_error()))
            } else {
                Error::Config(format!(
                    "refusing computed write through unsafe source `{}`: {error}",
                    relative_path.display()
                ))
            }
        })?;
        let mut file = std::fs::File::from(descriptor);
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(Error::Config(format!(
                "refusing computed write to non-file record `{}`",
                relative_path.display()
            )));
        }
        if metadata.nlink() > 1 {
            return Err(Error::Config(format!(
                "refusing computed write to hard-linked record `{}`",
                relative_path.display()
            )));
        }
        let permissions = metadata.permissions();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let source = String::from_utf8(bytes).map_err(|_| Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "file is not valid UTF-8".to_string(),
        })?;
        Ok(SecureSourceSnapshot {
            source,
            permissions,
            identity: ComputedFileIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
            },
            modified_at: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs())
                .unwrap_or(0),
        })
    }

    fn create_temporary(
        &self,
        permissions: std::fs::Permissions,
        rendered: &str,
    ) -> Result<SecureTemporary> {
        create_secure_temporary(
            &self.directory,
            ".mdvdb-computed-",
            permissions,
            rendered.as_bytes(),
        )
    }

    fn same_directory(&self, other: &Self) -> Result<bool> {
        use std::os::unix::fs::MetadataExt;

        let left = self.directory.metadata()?;
        let right = other.directory.metadata()?;
        Ok(left.dev() == right.dev() && left.ino() == right.ino())
    }
}

#[cfg(unix)]
fn markdown_from_secure_snapshot(
    relative_path: &Path,
    snapshot: &SecureSourceSnapshot,
) -> MarkdownFile {
    let content = &snapshot.source;
    let (frontmatter, body) = crate::parser::extract_frontmatter(content);
    MarkdownFile {
        path: relative_path.to_path_buf(),
        frontmatter: frontmatter.clone(),
        headings: crate::parser::extract_headings(body),
        body: body.to_string(),
        content_hash: compute_content_hash(content),
        file_size: content.len() as u64,
        links: crate::parser::extract_links(body),
        modified_at: snapshot.modified_at,
        frontmatter_links: crate::parser::extract_frontmatter_links(frontmatter.as_ref()),
    }
}

#[cfg(unix)]
fn read_secure_markdown(
    project_root: &SecureProjectRoot,
    relative_path: &Path,
) -> Result<(MarkdownFile, ComputedFileIdentity)> {
    let parent = SecureSourceParent::open(project_root, relative_path)?;
    let snapshot = parent.read_source(relative_path)?;
    let identity = snapshot.identity.clone();
    Ok((
        markdown_from_secure_snapshot(relative_path, &snapshot),
        identity,
    ))
}

#[cfg(unix)]
struct SecureTemporary {
    directory: std::fs::File,
    // Keep the descriptor alive until rename; this also makes permission and
    // durability guarantees independent of pathname replacement.
    file: std::fs::File,
    name: OsString,
    committed: bool,
}

#[cfg(unix)]
impl SecureTemporary {
    fn identity(&self) -> Result<ComputedFileIdentity> {
        use std::os::unix::fs::MetadataExt;

        let metadata = self.file.metadata()?;
        Ok(ComputedFileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn persist(mut self, target: &std::ffi::OsStr) -> Result<()> {
        rustix::fs::renameat(&self.directory, &self.name, &self.directory, target)
            .map_err(rustix_io)?;
        self.committed = true;
        self.directory.sync_all()?;
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for SecureTemporary {
    fn drop(&mut self) {
        if !self.committed {
            let _ = rustix::fs::unlinkat(&self.directory, &self.name, rustix::fs::AtFlags::empty());
        }
        let _ = self.file.sync_all();
    }
}

#[cfg(unix)]
fn create_secure_temporary(
    directory: &std::fs::File,
    prefix: &str,
    permissions: std::fs::Permissions,
    bytes: &[u8],
) -> Result<SecureTemporary> {
    use rustix::fs::{openat, Mode, OFlags};

    for _ in 0..128 {
        let name = OsString::from(format!("{prefix}{:016x}.tmp", rand::random::<u64>()));
        let descriptor = match openat(
            directory,
            &name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        ) {
            Ok(descriptor) => descriptor,
            Err(rustix::io::Errno::EXIST) => continue,
            Err(error) => return Err(rustix_io(error)),
        };
        let mut temporary = SecureTemporary {
            directory: directory.try_clone()?,
            file: std::fs::File::from(descriptor),
            name,
            committed: false,
        };
        temporary.file.set_permissions(permissions)?;
        temporary.file.write_all(bytes)?;
        temporary.file.flush()?;
        temporary.file.sync_all()?;
        return Ok(temporary);
    }
    Err(Error::Config(
        "could not allocate a unique computed-write temporary file".to_string(),
    ))
}

#[cfg(unix)]
struct SecureStateDirectory {
    directory: std::fs::File,
}

#[cfg(unix)]
impl SecureStateDirectory {
    fn open(project_root: &SecureProjectRoot, create: bool) -> Result<Option<Self>> {
        use rustix::fs::{mkdirat, openat, Mode, OFlags};

        let open_directory = || {
            openat(
                &project_root.directory,
                ".markdownvdb",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
        };
        let descriptor = match open_directory() {
            Ok(descriptor) => descriptor,
            Err(rustix::io::Errno::NOENT) if !create => return Ok(None),
            Err(rustix::io::Errno::NOENT) => {
                match mkdirat(
                    &project_root.directory,
                    ".markdownvdb",
                    Mode::from_raw_mode(0o700),
                ) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                    Err(error) => return Err(rustix_io(error)),
                }
                let descriptor = open_directory().map_err(|error| {
                    Error::Config(format!(
                        "refusing unsafe computed-intent state directory: {error}"
                    ))
                })?;
                // Persist the newly-created state-directory entry before any
                // intent or lock inside it is treated as durable.
                project_root.directory.sync_all()?;
                descriptor
            }
            Err(error) => {
                return Err(Error::Config(format!(
                    "refusing unsafe computed-intent state directory: {error}"
                )));
            }
        };
        Ok(Some(Self {
            directory: std::fs::File::from(descriptor),
        }))
    }

    fn open_lock_file(&self) -> Result<std::fs::File> {
        use std::os::unix::fs::MetadataExt;

        use rustix::fs::{openat, Mode, OFlags};

        let descriptor = openat(
            &self.directory,
            "modules.lock",
            OFlags::RDWR | OFlags::CREATE | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|error| {
            Error::Config(format!(
                "refusing unsafe computed-module lock file: {error}"
            ))
        })?;
        let file = std::fs::File::from(descriptor);
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() > 1 {
            return Err(Error::Config(
                "refusing unsafe computed-module lock file".to_string(),
            ));
        }
        // This is intentionally unconditional: it also durably orders the
        // first creation of the lock name without a separate racy preflight.
        self.directory.sync_all()?;
        Ok(file)
    }

    fn read(&self) -> Result<Option<Vec<u8>>> {
        use std::os::unix::fs::MetadataExt;

        use rustix::fs::{openat, Mode, OFlags};

        let descriptor = match openat(
            &self.directory,
            COMPUTED_INTENT_FILE,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(descriptor) => descriptor,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(error) => {
                return Err(Error::Config(format!(
                    "refusing unsafe computed-intent state file: {error}"
                )));
            }
        };
        let mut file = std::fs::File::from(descriptor);
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() > 1 {
            return Err(Error::Config(
                "refusing unsafe computed-intent state file".to_string(),
            ));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    fn write(&self, bytes: &[u8]) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        // Refuse a pre-existing final symlink/hard link rather than replacing
        // an object whose provenance is ambiguous. The final rename itself is
        // still dirfd-relative, so a race can at worst replace a name here.
        let _existing = self.read()?;
        let temporary = create_secure_temporary(
            &self.directory,
            ".computed-write-intent-",
            std::fs::Permissions::from_mode(0o600),
            bytes,
        )?;
        temporary.persist(OsStr::new(COMPUTED_INTENT_FILE))
    }

    fn remove(&self) -> Result<()> {
        if self.read()?.is_none() {
            return Ok(());
        }
        match rustix::fs::unlinkat(
            &self.directory,
            COMPUTED_INTENT_FILE,
            rustix::fs::AtFlags::empty(),
        ) {
            Ok(()) => self.directory.sync_all().map_err(Error::Io),
            Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(error) => Err(rustix_io(error)),
        }
    }
}

#[cfg(unix)]
pub(crate) fn open_module_run_lock_file(
    project_root: &Path,
) -> Result<(std::fs::File, ProjectRootGuard)> {
    let project_root = SecureProjectRoot::open(project_root)?;
    let identity = project_root.identity()?;
    let lock = SecureStateDirectory::open(&project_root, true)?
        .expect("create=true always returns a state directory")
        .open_lock_file()?;
    Ok((lock, ProjectRootGuard { identity }))
}

#[cfg(not(unix))]
pub(crate) fn open_module_run_lock_file(
    project_root: &Path,
) -> Result<(std::fs::File, ProjectRootGuard)> {
    let canonical_path = std::fs::canonicalize(project_root)?;
    let state_dir = project_root.join(".markdownvdb");
    std::fs::create_dir_all(&state_dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state_dir.join("modules.lock"))
        .map_err(Error::Io)?;
    Ok((lock, ProjectRootGuard { canonical_path }))
}

fn decode_computed_intents(path: &Path, bytes: &[u8]) -> Result<Option<ComputedWriteIntentLog>> {
    let log: ComputedWriteIntentLog = serde_json::from_slice(bytes).map_err(|error| {
        Error::Serialization(format!(
            "computed write intent `{}` is unreadable: {error}",
            path.display()
        ))
    })?;
    if log.version != COMPUTED_INTENT_VERSION {
        return Err(Error::Serialization(format!(
            "computed write intent `{}` has unsupported version {}",
            path.display(),
            log.version
        )));
    }
    Ok(Some(log))
}

#[cfg(unix)]
fn read_computed_intents_with_root(
    project_root_path: &Path,
    project_root: &SecureProjectRoot,
) -> Result<Option<ComputedWriteIntentLog>> {
    let bytes = match SecureStateDirectory::open(project_root, false)? {
        Some(directory) => match directory.read()? {
            Some(bytes) => bytes,
            None => return Ok(None),
        },
        None => return Ok(None),
    };
    decode_computed_intents(&computed_intent_path(project_root_path), &bytes)
}

#[cfg(any(not(unix), test))]
fn read_computed_intents(project_root: &Path) -> Result<Option<ComputedWriteIntentLog>> {
    #[cfg(unix)]
    {
        let secure_root = SecureProjectRoot::open(project_root)?;
        read_computed_intents_with_root(project_root, &secure_root)
    }
    #[cfg(not(unix))]
    {
        let path = computed_intent_path(project_root);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(Error::Io(error)),
        };
        decode_computed_intents(&path, &bytes)
    }
}

#[cfg(unix)]
fn write_computed_intents_with_root(
    project_root: &SecureProjectRoot,
    log: &ComputedWriteIntentLog,
) -> Result<()> {
    let encoded = serde_json::to_vec(log)
        .map_err(|error| Error::Serialization(format!("computed write intent: {error}")))?;
    let state = SecureStateDirectory::open(project_root, true)?
        .expect("create=true always returns a state directory");
    state.write(&encoded)
}

#[cfg(any(not(unix), test))]
fn write_computed_intents(project_root: &Path, log: &ComputedWriteIntentLog) -> Result<()> {
    #[cfg(unix)]
    {
        let secure_root = SecureProjectRoot::open(project_root)?;
        write_computed_intents_with_root(&secure_root, log)
    }
    #[cfg(not(unix))]
    {
        let encoded = serde_json::to_vec(log)
            .map_err(|error| Error::Serialization(format!("computed write intent: {error}")))?;
        let state_dir = project_root.join(".markdownvdb");
        std::fs::create_dir_all(&state_dir)?;
        let path = computed_intent_path(project_root);
        let mut temporary = tempfile::Builder::new()
            .prefix(".computed-write-intent-")
            .suffix(".tmp")
            .tempfile_in(&state_dir)?;
        temporary.write_all(&encoded)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&path)
            .map_err(|error| Error::Io(error.error))?;
        sync_directory(&state_dir)
    }
}

#[cfg(not(unix))]
fn sync_directory(path: &Path) -> Result<()> {
    let _ = path;
    Ok(())
}

fn record_computed_intent(
    project_root: &Path,
    #[cfg(unix)] secure_root: &SecureProjectRoot,
    relative_path: &Path,
    before_content_hash: &str,
    after_content_hash: &str,
    after_file_identity: Option<ComputedFileIdentity>,
    fields: &HashMap<String, ComputedFieldEntry>,
) -> Result<()> {
    #[cfg(unix)]
    let existing = read_computed_intents_with_root(project_root, secure_root)?;
    #[cfg(not(unix))]
    let existing = read_computed_intents(project_root)?;
    let mut log = existing.unwrap_or(ComputedWriteIntentLog {
        version: COMPUTED_INTENT_VERSION,
        entries: BTreeMap::new(),
    });
    let path = crate::path_util::to_slash(relative_path);
    let fields = fields
        .iter()
        .map(|(field, entry)| (field.clone(), IntentComputedFieldEntry::from(entry)))
        .collect();
    if let Some(existing) = log.entries.get_mut(&path) {
        existing.after_content_hash = after_content_hash.to_string();
        existing.after_file_identity = after_file_identity;
        existing.fields = fields;
    } else {
        log.entries.insert(
            path,
            ComputedWriteIntent {
                before_content_hash: before_content_hash.to_string(),
                after_content_hash: after_content_hash.to_string(),
                after_file_identity,
                fields,
            },
        );
    }
    #[cfg(unix)]
    {
        write_computed_intents_with_root(secure_root, &log)
    }
    #[cfg(not(unix))]
    {
        write_computed_intents(project_root, &log)
    }
}

/// Roll forward any source replacement that reached disk before its matching
/// index generation. The caller must hold the project-wide module run lock.
pub(crate) fn recover_computed_intents(project_root: &Path, index: &Index) -> Result<bool> {
    #[cfg(unix)]
    let secure_root = SecureProjectRoot::open(project_root)?;
    #[cfg(unix)]
    let log = read_computed_intents_with_root(project_root, &secure_root)?;
    #[cfg(not(unix))]
    let log = read_computed_intents(project_root)?;
    let Some(log) = log else {
        return Ok(false);
    };
    for (path, intent) in log.entries {
        let relative_path = Path::new(&path);
        validate_relative_path(relative_path)?;
        let Some(stored) = index.get_file(&path) else {
            continue;
        };
        #[cfg(unix)]
        let parsed = read_secure_markdown(&secure_root, relative_path)
            .map(|(file, identity)| (file, Some(identity)));
        #[cfg(not(unix))]
        let parsed = parse_markdown_file(project_root, relative_path).map(|file| (file, None));
        let (file, current_identity) = match parsed {
            Ok(parsed) => parsed,
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };

        if file.content_hash == intent.after_content_hash {
            let identity_matches = intent
                .after_file_identity
                .as_ref()
                .is_some_and(|expected| current_identity.as_ref() == Some(expected));
            if !identity_matches {
                // Identical bytes are not proof that our interrupted temporary
                // file reached the source name. A user or another process may
                // have independently produced those bytes; never adopt module
                // ownership without the exact future (dev, ino) recorded before
                // rename. Legacy intents therefore fail closed.
                index.refresh_source_metadata(&file)?;
                continue;
            }
            let fields: HashMap<String, ComputedFieldEntry> = intent
                .fields
                .into_iter()
                .map(|(field, entry)| (field, entry.into()))
                .collect();
            if stored.content_hash == intent.before_content_hash {
                index.apply_module_source_state(&stored.content_hash, &file, fields)?;
            } else if stored.content_hash == intent.after_content_hash {
                index.replace_computed_fields(&path, fields)?;
            } else {
                // A newer raw ingest superseded the planned provenance. Keep its
                // source snapshot and let the ensuing full module pass recompute.
                index.refresh_source_metadata(&file)?;
            }
        } else if stored.content_hash != file.content_hash {
            // The user edited the record after the interrupted replacement, or
            // the crash happened before rename. Source bytes remain authoritative.
            index.refresh_source_metadata(&file)?;
        }
    }
    Ok(true)
}

/// Remove the durable intent only after every affected source hash is present
/// in the index generation that was just saved.
pub(crate) fn finish_computed_intents(project_root: &Path, index: &Index) -> Result<()> {
    #[cfg(unix)]
    let secure_root = SecureProjectRoot::open(project_root)?;
    #[cfg(unix)]
    let log = read_computed_intents_with_root(project_root, &secure_root)?;
    #[cfg(not(unix))]
    let log = read_computed_intents(project_root)?;
    let Some(log) = log else {
        return Ok(());
    };
    for path in log.entries.keys() {
        let relative_path = Path::new(path);
        validate_relative_path(relative_path)?;
        #[cfg(unix)]
        let parsed = read_secure_markdown(&secure_root, relative_path).map(|(file, _)| file);
        #[cfg(not(unix))]
        let parsed = parse_markdown_file(project_root, relative_path);
        match (parsed, index.get_file(path)) {
            (Ok(file), Some(stored)) if file.content_hash == stored.content_hash => {}
            (Err(Error::Io(error)), None) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => {
                return Err(Error::SourceChanged {
                    path: relative_path.to_path_buf(),
                });
            }
        }
    }

    // Order the two durable commits: first make the newly renamed index
    // generation persistent while the intent still exists, then retire the
    // intent and persist that deletion.
    #[cfg(unix)]
    {
        let Some(state) = SecureStateDirectory::open(&secure_root, false)? else {
            return Ok(());
        };
        state.directory.sync_all()?;
        state.remove()
    }
    #[cfg(not(unix))]
    {
        let intent_path = computed_intent_path(project_root);
        let state_dir = intent_path
            .parent()
            .expect("computed intent always has a state directory");
        sync_directory(state_dir)?;
        match std::fs::remove_file(&intent_path) {
            Ok(()) => sync_directory(state_dir),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(Error::Io(error)),
        }
    }
}

#[derive(Debug)]
pub struct WritebackResult {
    pub file: MarkdownFile,
    pub changed: bool,
    /// Set fields whose current value is demonstrably module-authored: either
    /// this write changed the semantic value, or a prior matching ownership
    /// proof already authorized the no-op.
    pub materialized_fields: BTreeSet<String>,
}

pub(crate) fn normalize_committed_ownership(
    fields: &mut HashMap<String, ComputedFieldEntry>,
    set: &BTreeMap<String, JsonValue>,
    unset: &BTreeSet<String>,
    materialized_fields: &BTreeSet<String>,
) -> Result<()> {
    for field in unset {
        if !set.contains_key(field) {
            if let Some(entry) = fields.get_mut(field) {
                entry.materialized_value_json = None;
            }
        }
    }
    for (field, value) in set {
        let Some(entry) = fields.get_mut(field) else {
            return Err(Error::Config(format!(
                "computed write is missing provenance for frontmatter key `{field}`"
            )));
        };
        let encoded = serde_json::to_string(value)
            .map_err(|error| Error::Serialization(format!("computed value: {error}")))?;
        if entry
            .value_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<JsonValue>(raw).ok())
            .as_ref()
            != Some(value)
            || entry.diagnostic.is_some()
        {
            return Err(Error::Config(format!(
                "computed write provenance does not match frontmatter key `{field}`"
            )));
        }
        entry.materialized_value_json = materialized_fields.contains(field).then_some(encoded);
    }
    Ok(())
}

#[derive(Debug)]
struct FrontmatterBounds {
    yaml_start: usize,
    yaml_end: usize,
    body_start: usize,
    newline: &'static str,
}

fn frontmatter_bounds(content: &str, relative_path: &Path) -> Result<Option<FrontmatterBounds>> {
    let bom_len = usize::from(content.starts_with('\u{feff}')) * '\u{feff}'.len_utf8();
    let source = &content[bom_len..];
    if !source.starts_with("---") {
        return Ok(None);
    }

    let opening_end = source.find('\n').ok_or_else(|| Error::MarkdownParse {
        path: relative_path.to_path_buf(),
        message: "frontmatter opening delimiter has no closing delimiter".to_string(),
    })? + 1;
    let opening_line = &source[..opening_end];
    if !crate::parser::is_frontmatter_delimiter_line(opening_line) {
        return Ok(None);
    }
    let newline = if opening_line.ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };

    let yaml_start = bom_len + opening_end;
    let mut cursor = yaml_start;
    while cursor < content.len() {
        let remaining = &content[cursor..];
        let line_len = remaining
            .find('\n')
            .map_or(remaining.len(), |index| index + 1);
        let line = &remaining[..line_len];
        if crate::parser::is_frontmatter_delimiter_line(line) {
            return Ok(Some(FrontmatterBounds {
                yaml_start,
                yaml_end: cursor,
                body_start: cursor + line_len,
                newline,
            }));
        }
        cursor += line_len;
    }

    Err(Error::MarkdownParse {
        path: relative_path.to_path_buf(),
        message: "frontmatter opening delimiter has no closing delimiter".to_string(),
    })
}

struct EditableMapping {
    document: Document,
    prefix: String,
    suffix: String,
}

#[derive(Debug)]
enum MarkedYamlEventKind {
    StreamStart,
    StreamEnd,
    DocumentStart,
    DocumentEnd,
    Scalar(String),
    Alias,
    SequenceStart,
    SequenceEnd,
    MappingStart,
    MappingEnd,
}

#[derive(Debug)]
struct MarkedYamlEvent {
    kind: MarkedYamlEventKind,
    start: usize,
    end: usize,
    start_line: usize,
    end_line: usize,
}

#[derive(Debug)]
struct YamlNodeSpan {
    scalar: Option<String>,
    start: usize,
    end: usize,
    start_line: usize,
    end_line: usize,
}

#[derive(Debug)]
struct TopLevelYamlEntry {
    key: String,
    key_start: usize,
    key_end: usize,
    key_line: usize,
    value_end: usize,
    value_end_line: usize,
}

/// Parse YAML syntax events while retaining libyaml's exact UTF-8 byte marks.
///
/// yaml-edit 0.2.3 only includes the first token of an unquoted multi-word key
/// in its mapping CST. That makes `Document::set("Client Name", ...)` append a
/// duplicate pair. libyaml correctly reports the complete decoded scalar and
/// byte range, so we use its marks to canonicalize only keys owned by the
/// current patch before handing the document to yaml-edit.
fn marked_yaml_events(source: &str, relative_path: &Path) -> Result<Vec<MarkedYamlEvent>> {
    use std::mem::MaybeUninit;
    use std::slice;
    use unsafe_libyaml::{
        yaml_event_delete, yaml_event_t, yaml_parser_delete, yaml_parser_initialize,
        yaml_parser_parse, yaml_parser_set_input_string, yaml_parser_t, YAML_ALIAS_EVENT,
        YAML_DOCUMENT_END_EVENT, YAML_DOCUMENT_START_EVENT, YAML_MAPPING_END_EVENT,
        YAML_MAPPING_START_EVENT, YAML_SCALAR_EVENT, YAML_SEQUENCE_END_EVENT,
        YAML_SEQUENCE_START_EVENT, YAML_STREAM_END_EVENT, YAML_STREAM_START_EVENT,
    };

    let mut parser = MaybeUninit::<yaml_parser_t>::uninit();
    let parser = parser.as_mut_ptr();
    let mut events = Vec::new();

    // SAFETY: libyaml initializes `parser` before it is read. `source` remains
    // alive until the parser is deleted, and each successful event is copied
    // and deleted exactly once before the next parse call.
    unsafe {
        if yaml_parser_initialize(parser).fail {
            return Err(Error::MarkdownParse {
                path: relative_path.to_path_buf(),
                message: "could not initialize the YAML safety parser".to_string(),
            });
        }
        yaml_parser_set_input_string(parser, source.as_ptr(), source.len() as u64);

        let mut event = MaybeUninit::<yaml_event_t>::uninit();
        let event = event.as_mut_ptr();
        loop {
            if yaml_parser_parse(parser, event).fail {
                yaml_parser_delete(parser);
                return Err(Error::MarkdownParse {
                    path: relative_path.to_path_buf(),
                    message: "malformed frontmatter rejected by the YAML safety parser".to_string(),
                });
            }
            let event_type = (*event).type_;
            let kind = if event_type == YAML_STREAM_START_EVENT {
                MarkedYamlEventKind::StreamStart
            } else if event_type == YAML_STREAM_END_EVENT {
                MarkedYamlEventKind::StreamEnd
            } else if event_type == YAML_DOCUMENT_START_EVENT {
                MarkedYamlEventKind::DocumentStart
            } else if event_type == YAML_DOCUMENT_END_EVENT {
                MarkedYamlEventKind::DocumentEnd
            } else if event_type == YAML_SCALAR_EVENT {
                let scalar = (*event).data.scalar;
                let bytes = slice::from_raw_parts(scalar.value, scalar.length as usize);
                let value = match std::str::from_utf8(bytes) {
                    Ok(value) => value.to_string(),
                    Err(_) => {
                        yaml_event_delete(event);
                        yaml_parser_delete(parser);
                        return Err(Error::MarkdownParse {
                            path: relative_path.to_path_buf(),
                            message: "frontmatter scalar is not valid UTF-8".to_string(),
                        });
                    }
                };
                MarkedYamlEventKind::Scalar(value)
            } else if event_type == YAML_ALIAS_EVENT {
                MarkedYamlEventKind::Alias
            } else if event_type == YAML_SEQUENCE_START_EVENT {
                MarkedYamlEventKind::SequenceStart
            } else if event_type == YAML_SEQUENCE_END_EVENT {
                MarkedYamlEventKind::SequenceEnd
            } else if event_type == YAML_MAPPING_START_EVENT {
                MarkedYamlEventKind::MappingStart
            } else if event_type == YAML_MAPPING_END_EVENT {
                MarkedYamlEventKind::MappingEnd
            } else {
                yaml_event_delete(event);
                yaml_parser_delete(parser);
                return Err(Error::MarkdownParse {
                    path: relative_path.to_path_buf(),
                    message: "frontmatter contains an unsupported YAML event".to_string(),
                });
            };
            events.push(MarkedYamlEvent {
                kind,
                start: (*event).start_mark.index as usize,
                end: (*event).end_mark.index as usize,
                start_line: (*event).start_mark.line as usize,
                end_line: (*event).end_mark.line as usize,
            });
            yaml_event_delete(event);
            if event_type == YAML_STREAM_END_EVENT {
                break;
            }
        }
        yaml_parser_delete(parser);
    }

    Ok(events)
}

fn consume_yaml_node(
    events: &[MarkedYamlEvent],
    cursor: &mut usize,
    depth: usize,
    relative_path: &Path,
) -> Result<YamlNodeSpan> {
    if depth > 128 {
        return Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "frontmatter nesting exceeds the computed-writer safety limit".to_string(),
        });
    }
    let event = events.get(*cursor).ok_or_else(|| Error::MarkdownParse {
        path: relative_path.to_path_buf(),
        message: "frontmatter YAML event stream ended unexpectedly".to_string(),
    })?;
    match &event.kind {
        MarkedYamlEventKind::Scalar(value) => {
            *cursor += 1;
            Ok(YamlNodeSpan {
                scalar: Some(value.clone()),
                start: event.start,
                end: event.end,
                start_line: event.start_line,
                end_line: event.end_line,
            })
        }
        MarkedYamlEventKind::Alias => {
            *cursor += 1;
            Ok(YamlNodeSpan {
                scalar: None,
                start: event.start,
                end: event.end,
                start_line: event.start_line,
                end_line: event.end_line,
            })
        }
        MarkedYamlEventKind::SequenceStart | MarkedYamlEventKind::MappingStart => {
            let mapping = matches!(event.kind, MarkedYamlEventKind::MappingStart);
            let start = event.start;
            let start_line = event.start_line;
            *cursor += 1;
            loop {
                let next = events.get(*cursor).ok_or_else(|| Error::MarkdownParse {
                    path: relative_path.to_path_buf(),
                    message: "unterminated YAML collection in frontmatter".to_string(),
                })?;
                let at_end = if mapping {
                    matches!(next.kind, MarkedYamlEventKind::MappingEnd)
                } else {
                    matches!(next.kind, MarkedYamlEventKind::SequenceEnd)
                };
                if at_end {
                    let span = YamlNodeSpan {
                        scalar: None,
                        start,
                        end: next.end,
                        start_line,
                        end_line: next.end_line,
                    };
                    *cursor += 1;
                    return Ok(span);
                }
                let _ = consume_yaml_node(events, cursor, depth + 1, relative_path)?;
                if mapping {
                    let _ = consume_yaml_node(events, cursor, depth + 1, relative_path)?;
                }
            }
        }
        _ => Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "frontmatter contains an unexpected YAML collection boundary".to_string(),
        }),
    }
}

fn top_level_yaml_entries(source: &str, relative_path: &Path) -> Result<Vec<TopLevelYamlEntry>> {
    if source.trim().is_empty() {
        return Ok(Vec::new());
    }
    let events = marked_yaml_events(source, relative_path)?;
    let mut cursor = 0usize;
    if matches!(
        events.get(cursor).map(|event| &event.kind),
        Some(MarkedYamlEventKind::StreamStart)
    ) {
        cursor += 1;
    }
    if matches!(
        events.get(cursor).map(|event| &event.kind),
        Some(MarkedYamlEventKind::DocumentStart)
    ) {
        cursor += 1;
    }

    // An empty YAML document is represented as a null/empty scalar.
    if matches!(
        events.get(cursor).map(|event| &event.kind),
        Some(MarkedYamlEventKind::Scalar(value)) if value.is_empty()
    ) {
        return Ok(Vec::new());
    }
    if matches!(
        events.get(cursor).map(|event| &event.kind),
        Some(MarkedYamlEventKind::DocumentEnd | MarkedYamlEventKind::StreamEnd)
    ) {
        return Ok(Vec::new());
    }
    if !matches!(
        events.get(cursor).map(|event| &event.kind),
        Some(MarkedYamlEventKind::MappingStart)
    ) {
        return Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "frontmatter must be a top-level mapping".to_string(),
        });
    }
    cursor += 1;

    let mut entries = Vec::new();
    loop {
        let event = events.get(cursor).ok_or_else(|| Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "unterminated top-level frontmatter mapping".to_string(),
        })?;
        if matches!(event.kind, MarkedYamlEventKind::MappingEnd) {
            break;
        }
        let key = consume_yaml_node(&events, &mut cursor, 0, relative_path)?;
        let value = consume_yaml_node(&events, &mut cursor, 0, relative_path)?;
        let key_value = key.scalar.ok_or_else(|| Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "computed writes require scalar top-level frontmatter keys".to_string(),
        })?;
        entries.push(TopLevelYamlEntry {
            key: key_value,
            key_start: key.start,
            key_end: key.end,
            key_line: key.start_line,
            value_end: value.end,
            value_end_line: value.end_line,
        });
    }
    Ok(entries)
}

fn line_start(source: &str, offset: usize) -> usize {
    source[..offset.min(source.len())]
        .rfind('\n')
        .map_or(0, |index| index + 1)
}

fn line_end(source: &str, offset: usize) -> usize {
    source[offset.min(source.len())..]
        .find('\n')
        .map_or(source.len(), |index| offset.min(source.len()) + index + 1)
}

fn requires_quoted_key(field: &str) -> bool {
    let mut bytes = field.bytes();
    let syntactically_plain_string = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    if !syntactically_plain_string {
        return true;
    }
    !matches!(
        serde_yaml::from_str::<serde_yaml::Value>(field),
        Ok(serde_yaml::Value::String(value)) if value == field
    )
}

/// Quote every touched key before yaml-edit parses the mapping. For a duplicate
/// touched key left by an interrupted/older computed write, retain the first
/// pair and remove only later one-line occurrences. Unrelated duplicates and
/// ambiguous multi-line duplicates remain hard failures and leave disk bytes
/// untouched.
fn canonicalize_touched_keys(
    source: &str,
    relative_path: &Path,
    set: &BTreeMap<String, JsonValue>,
    unset: &BTreeSet<String>,
) -> Result<String> {
    let entries = top_level_yaml_entries(source, relative_path)?;
    let mut touched = unset.clone();
    touched.extend(set.keys().cloned());
    let mut edits: Vec<(usize, usize, String)> = Vec::new();

    for field in touched {
        let occurrences: Vec<_> = entries.iter().filter(|entry| entry.key == field).collect();
        if occurrences.is_empty() {
            continue;
        }
        let keep_first = set.contains_key(&field);
        for (index, entry) in occurrences.iter().enumerate() {
            if keep_first && index == 0 {
                if requires_quoted_key(&field) {
                    edits.push((
                        entry.key_start,
                        entry.key_end,
                        serde_json::to_string(&field).map_err(|error| {
                            Error::Serialization(format!("computed frontmatter key: {error}"))
                        })?,
                    ));
                }
                continue;
            }
            if occurrences.len() == 1 {
                // A unique unset is removed losslessly by yaml-edit after its
                // key has been made unambiguous to the CST parser.
                if requires_quoted_key(&field) {
                    edits.push((
                        entry.key_start,
                        entry.key_end,
                        serde_json::to_string(&field).map_err(|error| {
                            Error::Serialization(format!("computed frontmatter key: {error}"))
                        })?,
                    ));
                }
                continue;
            }

            if entry.key_line != entry.value_end_line
                || entries.iter().any(|candidate| {
                    !std::ptr::eq(candidate, *entry) && candidate.key_line == entry.key_line
                })
            {
                return Err(Error::MarkdownParse {
                    path: relative_path.to_path_buf(),
                    message: format!(
                        "refusing to canonicalize ambiguous duplicate computed key `{field}`"
                    ),
                });
            }
            let start = line_start(source, entry.key_start);
            if !source[start..entry.key_start].trim().is_empty() {
                return Err(Error::MarkdownParse {
                    path: relative_path.to_path_buf(),
                    message: format!(
                        "refusing to canonicalize non-top-level duplicate computed key `{field}`"
                    ),
                });
            }
            edits.push((start, line_end(source, entry.value_end), String::new()));
        }
    }

    edits.sort_by_key(|(start, _, _)| *start);
    for pair in edits.windows(2) {
        if pair[0].1 > pair[1].0 {
            return Err(Error::MarkdownParse {
                path: relative_path.to_path_buf(),
                message: "computed frontmatter edits overlap; refusing the write".to_string(),
            });
        }
    }
    let mut canonical = source.to_string();
    for (start, end, replacement) in edits.into_iter().rev() {
        canonical.replace_range(start..end, &replacement);
    }
    Ok(canonical)
}

fn yaml_trivia_only(source: &str) -> bool {
    source
        .lines()
        .all(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
}

fn validate_uniform_yaml_newlines(source: &str, relative_path: &Path) -> Result<()> {
    let bytes = source.as_bytes();
    let has_crlf = bytes.windows(2).any(|pair| pair == b"\r\n");
    let has_lone_lf = bytes
        .iter()
        .enumerate()
        .any(|(index, byte)| *byte == b'\n' && (index == 0 || bytes[index - 1] != b'\r'));
    let has_lone_cr = bytes
        .iter()
        .enumerate()
        .any(|(index, byte)| *byte == b'\r' && bytes.get(index + 1) != Some(&b'\n'));
    if usize::from(has_crlf) + usize::from(has_lone_lf) + usize::from(has_lone_cr) > 1 {
        return Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "frontmatter uses mixed newline styles; refusing a lossy computed write"
                .to_string(),
        });
    }
    Ok(())
}

fn yaml_without_touched_entries(
    source: &str,
    relative_path: &Path,
    touched: &BTreeSet<String>,
) -> Result<String> {
    let entries = top_level_yaml_entries(source, relative_path)?;
    let mut ranges = Vec::new();
    for entry in entries.iter().filter(|entry| touched.contains(&entry.key)) {
        let start = line_start(source, entry.key_start);
        let end = line_end(source, entry.value_end);
        if entries.iter().any(|other| {
            !touched.contains(&other.key)
                && ((other.key_start >= start && other.key_start < end)
                    || (other.value_end > start && other.value_end <= end))
        }) {
            return Err(Error::MarkdownParse {
                path: relative_path.to_path_buf(),
                message: format!(
                    "computed key `{}` is not isolated from unrelated YAML on its line",
                    entry.key
                ),
            });
        }
        ranges.push((start, end));
    }
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        if let Some(last) = merged.last_mut().filter(|last| start <= last.1) {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    let mut projected = source.to_string();
    for (start, end) in merged.into_iter().rev() {
        projected.replace_range(start..end, "");
    }
    Ok(projected)
}

fn parse_mapping(source: &str, relative_path: &Path) -> Result<EditableMapping> {
    // Validate semantics with the same YAML implementation used by the Markdown
    // parser, then use yaml-edit's concrete syntax tree for lossless mutation.
    let semantic = serde_yaml::from_str::<serde_yaml::Value>(source).map_err(|error| {
        Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: format!("malformed frontmatter: {error}"),
        }
    })?;
    if !semantic.is_mapping() && !semantic.is_null() {
        return Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "frontmatter must be a top-level mapping".to_string(),
        });
    }

    if semantic.is_null() {
        let core = source.trim_end_matches(['\r', '\n']);
        let separator = if core.is_empty() {
            ""
        } else if source.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        return Ok(EditableMapping {
            document: Document::new_mapping(),
            prefix: format!("{core}{separator}"),
            suffix: String::new(),
        });
    }
    let document = Document::from_str(source).map_err(|error| Error::MarkdownParse {
        path: relative_path.to_path_buf(),
        message: format!("malformed frontmatter: {error}"),
    })?;
    let Some(mapping) = document.as_mapping() else {
        return Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "frontmatter must be a top-level mapping".to_string(),
        });
    };
    let range = mapping.byte_range();
    let prefix = &source[..range.start as usize];
    let suffix = &source[range.end as usize..];
    if !yaml_trivia_only(prefix) || !yaml_trivia_only(suffix) {
        return Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message:
                "the lossless YAML parser did not cover every frontmatter pair; refusing the write"
                    .to_string(),
        });
    }
    Ok(EditableMapping {
        document,
        prefix: prefix.to_string(),
        suffix: suffix.to_string(),
    })
}

fn json_value_node(value: &JsonValue, relative_path: &Path) -> Result<YamlNode> {
    let source = serde_json::to_string(value)
        .map_err(|error| Error::Serialization(format!("computed value: {error}")))?;
    let document = Document::from_str(&source).map_err(|error| Error::MarkdownParse {
        path: relative_path.to_path_buf(),
        message: format!("computed value cannot be represented as YAML: {error}"),
    })?;
    if let Some(value) = document.as_scalar() {
        Ok(YamlNode::Scalar(value))
    } else if let Some(value) = document.as_sequence() {
        Ok(YamlNode::Sequence(value))
    } else if let Some(value) = document.as_mapping() {
        Ok(YamlNode::Mapping(value))
    } else {
        Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "computed value has no YAML representation".to_string(),
        })
    }
}

fn quoted_key_node(field: &str, relative_path: &Path) -> Result<YamlNode> {
    if requires_quoted_key(field) {
        json_value_node(&JsonValue::String(field.to_string()), relative_path)
    } else {
        let document = Document::from_str(field).map_err(|error| Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: format!("computed frontmatter key cannot be represented as YAML: {error}"),
        })?;
        document
            .as_scalar()
            .map(|scalar| YamlNode::Scalar(scalar.clone()))
            .ok_or_else(|| Error::MarkdownParse {
                path: relative_path.to_path_buf(),
                message: "computed frontmatter key has no scalar representation".to_string(),
            })
    }
}

fn normalize_newlines(source: &str, newline: &str) -> String {
    let normalized = source.replace("\r\n", "\n");
    if newline == "\n" {
        normalized
    } else {
        normalized.replace('\n', "\r\n")
    }
}

fn strict_frontmatter_map(
    content: &str,
    relative_path: &Path,
) -> Result<serde_json::Map<String, JsonValue>> {
    let Some(bounds) = frontmatter_bounds(content, relative_path)? else {
        return Ok(serde_json::Map::new());
    };
    let yaml = &content[bounds.yaml_start..bounds.yaml_end];
    let semantic =
        serde_yaml::from_str::<serde_yaml::Value>(yaml).map_err(|error| Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: format!("rendered frontmatter is malformed: {error}"),
        })?;
    if semantic.is_null() {
        return Ok(serde_json::Map::new());
    }
    let Some(mapping) = semantic.as_mapping() else {
        return Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "rendered frontmatter is not a top-level mapping".to_string(),
        });
    };
    if mapping
        .keys()
        .any(|key| !matches!(key, serde_yaml::Value::String(_)))
    {
        return Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "rendered frontmatter contains a non-string top-level key".to_string(),
        });
    }
    match crate::parser::extract_frontmatter(content).0 {
        Some(JsonValue::Object(map)) => Ok(map),
        _ => Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "rendered frontmatter could not be decoded as an object".to_string(),
        }),
    }
}

fn body_without_frontmatter<'a>(content: &'a str, relative_path: &Path) -> Result<&'a str> {
    if let Some(bounds) = frontmatter_bounds(content, relative_path)? {
        Ok(&content[bounds.body_start..])
    } else {
        Ok(content.trim_start_matches('\u{feff}'))
    }
}

fn verify_rendered_patch(
    original: &str,
    rendered: &str,
    relative_path: &Path,
    canonical_yaml_before: &str,
    canonical_before: &serde_json::Map<String, JsonValue>,
    set: &BTreeMap<String, JsonValue>,
    unset: &BTreeSet<String>,
) -> Result<()> {
    let original_has_frontmatter = frontmatter_bounds(original, relative_path)?.is_some();
    let rendered_has_frontmatter = frontmatter_bounds(rendered, relative_path)?.is_some();
    if original_has_frontmatter && !rendered_has_frontmatter {
        return Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "computed patch would remove the frontmatter envelope".to_string(),
        });
    }
    if !original_has_frontmatter && !set.is_empty() && !rendered_has_frontmatter {
        return Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "computed patch failed to create a frontmatter envelope".to_string(),
        });
    }
    if original.starts_with('\u{feff}') != rendered.starts_with('\u{feff}') {
        return Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "computed patch would change the UTF-8 BOM".to_string(),
        });
    }
    if body_without_frontmatter(original, relative_path)?
        != body_without_frontmatter(rendered, relative_path)?
    {
        return Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "computed patch would change Markdown body bytes".to_string(),
        });
    }

    if original_has_frontmatter {
        let rendered_bounds = frontmatter_bounds(rendered, relative_path)?
            .expect("frontmatter envelope was verified above");
        let rendered_yaml = &rendered[rendered_bounds.yaml_start..rendered_bounds.yaml_end];
        let mut touched = unset.clone();
        touched.extend(set.keys().cloned());
        let before_projection =
            yaml_without_touched_entries(canonical_yaml_before, relative_path, &touched)?;
        let after_projection =
            yaml_without_touched_entries(rendered_yaml, relative_path, &touched)?;
        if before_projection != after_projection
            && !(before_projection.trim().is_empty() && after_projection.trim().is_empty())
        {
            return Err(Error::MarkdownParse {
                path: relative_path.to_path_buf(),
                message: "computed patch would change unrelated frontmatter bytes".to_string(),
            });
        }
    }

    let mut expected = canonical_before.clone();
    for field in unset {
        if !set.contains_key(field) {
            expected.remove(field);
        }
    }
    for (field, value) in set {
        expected.insert(field.clone(), value.clone());
    }
    let actual = strict_frontmatter_map(rendered, relative_path)?;
    if actual != expected {
        return Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "computed patch failed the frontmatter preservation invariant".to_string(),
        });
    }
    Ok(())
}

fn render_patch(
    original: &str,
    relative_path: &Path,
    set: &BTreeMap<String, JsonValue>,
    unset: &BTreeSet<String>,
) -> Result<String> {
    let bounds = frontmatter_bounds(original, relative_path)?;
    if bounds.is_none() && set.is_empty() {
        return Ok(original.to_string());
    }

    let existing_values = crate::parser::extract_frontmatter(original)
        .0
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    if bounds.is_some()
        && set
            .iter()
            .all(|(field, value)| existing_values.get(field) == Some(value))
        && unset
            .iter()
            .all(|field| set.contains_key(field) || !existing_values.contains_key(field))
    {
        return Ok(original.to_string());
    }
    let (editable, newline) = if let Some(bounds) = &bounds {
        let source = &original[bounds.yaml_start..bounds.yaml_end];
        validate_uniform_yaml_newlines(source, relative_path)?;
        let canonical = canonicalize_touched_keys(source, relative_path, set, unset)?;
        (parse_mapping(&canonical, relative_path)?, bounds.newline)
    } else {
        (
            EditableMapping {
                document: Document::new_mapping(),
                prefix: String::new(),
                suffix: String::new(),
            },
            if original.contains("\r\n") {
                "\r\n"
            } else {
                "\n"
            },
        )
    };

    for field in unset {
        if set.contains_key(field) {
            continue;
        }
        editable
            .document
            .remove(quoted_key_node(field, relative_path)?);
    }
    for (field, value) in set {
        if existing_values.get(field) == Some(value) {
            continue;
        }
        editable.document.set(
            quoted_key_node(field, relative_path)?,
            json_value_node(value, relative_path)?,
        );
    }

    let rendered = if let Some(bounds) = bounds {
        let mapping = editable
            .document
            .as_mapping()
            .expect("validated mapping remains a mapping");
        let edited_yaml = format!("{}{}{}", editable.prefix, mapping, editable.suffix);
        let original_yaml = &original[bounds.yaml_start..bounds.yaml_end];
        let trailing_start = original_yaml.trim_end_matches(['\r', '\n']).len();
        let original_trailing = &original_yaml[trailing_start..];
        let trailing = if original_trailing.is_empty() {
            bounds.newline
        } else {
            original_trailing
        };
        let yaml = normalize_newlines(edited_yaml.trim_end_matches(['\r', '\n']), bounds.newline);
        format!(
            "{}{}{}{}",
            &original[..bounds.yaml_start],
            yaml,
            trailing,
            &original[bounds.yaml_end..]
        )
    } else {
        let bom_len = usize::from(original.starts_with('\u{feff}')) * '\u{feff}'.len_utf8();
        let mapping = editable
            .document
            .as_mapping()
            .expect("new computed frontmatter is a mapping");
        let yaml = normalize_newlines(mapping.to_string().trim_end_matches(['\r', '\n']), newline);
        format!(
            "{}---{newline}{yaml}{newline}---{newline}{}",
            &original[..bom_len],
            &original[bom_len..]
        )
    };

    // Validate against the canonical pre-edit map rather than the possibly
    // malformed original. This lets us safely self-heal duplicate occurrences
    // of a touched computed key while rejecting every unrelated YAML defect.
    let canonical_source = if let Some(bounds) = frontmatter_bounds(original, relative_path)? {
        canonicalize_touched_keys(
            &original[bounds.yaml_start..bounds.yaml_end],
            relative_path,
            set,
            unset,
        )?
    } else {
        String::new()
    };
    let canonical_document = if canonical_source.trim().is_empty() {
        format!("---{newline}---{newline}")
    } else {
        format!("---{newline}{canonical_source}{newline}---{newline}")
    };
    let canonical_before = strict_frontmatter_map(&canonical_document, relative_path)?;
    verify_rendered_patch(
        original,
        &rendered,
        relative_path,
        &canonical_source,
        &canonical_before,
        set,
        unset,
    )?;
    Ok(rendered)
}

/// Apply one module patch atomically after verifying the exact bytes evaluated.
pub fn apply_frontmatter_patch(
    project_root: &Path,
    relative_path: &Path,
    expected_content_hash: &str,
    set: &BTreeMap<String, JsonValue>,
    unset: &BTreeSet<String>,
) -> Result<WritebackResult> {
    apply_frontmatter_patch_inner(
        project_root,
        relative_path,
        expected_content_hash,
        set,
        unset,
        None,
    )
}

#[cfg(test)]
pub(crate) fn apply_frontmatter_patch_with_intent(
    project_root: &Path,
    relative_path: &Path,
    expected_content_hash: &str,
    set: &BTreeMap<String, JsonValue>,
    unset: &BTreeSet<String>,
    owned_unset: &BTreeSet<String>,
    fields: &HashMap<String, ComputedFieldEntry>,
) -> Result<WritebackResult> {
    apply_frontmatter_patch_with_intent_and_guard(
        project_root,
        relative_path,
        expected_content_hash,
        set,
        unset,
        ComputedWriteContext {
            owned_unset,
            fields,
            pre_commit_guard: None,
        },
    )
}

pub(crate) struct ComputedWriteContext<'a> {
    pub owned_unset: &'a BTreeSet<String>,
    pub fields: &'a HashMap<String, ComputedFieldEntry>,
    pub pre_commit_guard: Option<&'a dyn Fn() -> Result<()>>,
}

pub(crate) fn apply_frontmatter_patch_with_intent_and_guard(
    project_root: &Path,
    relative_path: &Path,
    expected_content_hash: &str,
    set: &BTreeMap<String, JsonValue>,
    unset: &BTreeSet<String>,
    context: ComputedWriteContext<'_>,
) -> Result<WritebackResult> {
    apply_frontmatter_patch_inner(
        project_root,
        relative_path,
        expected_content_hash,
        set,
        unset,
        Some(context),
    )
}

fn apply_frontmatter_patch_inner(
    project_root: &Path,
    relative_path: &Path,
    expected_content_hash: &str,
    set: &BTreeMap<String, JsonValue>,
    unset: &BTreeSet<String>,
    context: Option<ComputedWriteContext<'_>>,
) -> Result<WritebackResult> {
    let owned_unset = context.as_ref().map(|context| context.owned_unset);
    let intent_fields = context.as_ref().map(|context| context.fields);
    let pre_commit_guard = context.and_then(|context| context.pre_commit_guard);
    validate_relative_path(relative_path)?;

    #[cfg(not(unix))]
    let full_path = project_root.join(relative_path);
    #[cfg(not(unix))]
    let parent = full_path.parent().ok_or_else(|| {
        Error::Config(format!(
            "module source has no parent directory: {}",
            full_path.display()
        ))
    })?;
    #[cfg(unix)]
    let secure_root = SecureProjectRoot::open(project_root)?;
    #[cfg(unix)]
    let secure_parent = SecureSourceParent::open(&secure_root, relative_path)?;
    #[cfg(unix)]
    let (original, source_permissions) = {
        let snapshot = secure_parent.read_source(relative_path)?;
        (snapshot.source, snapshot.permissions)
    };
    #[cfg(not(unix))]
    let (original, source_permissions) = {
        let canonical_root = std::fs::canonicalize(project_root)?;
        let canonical_parent = std::fs::canonicalize(parent)?;
        if !canonical_parent.starts_with(&canonical_root) {
            return Err(Error::Config(format!(
                "refusing computed write through an ancestor outside project `{}`",
                relative_path.display()
            )));
        }
        let link_metadata = std::fs::symlink_metadata(&full_path)?;
        if link_metadata.file_type().is_symlink() {
            return Err(Error::Config(format!(
                "refusing computed write through symlink `{}`",
                relative_path.display()
            )));
        }
        let bytes = std::fs::read(&full_path)?;
        let source = String::from_utf8(bytes).map_err(|_| Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "file is not valid UTF-8".to_string(),
        })?;
        (source, link_metadata.permissions())
    };
    if compute_content_hash(&original) != expected_content_hash {
        return Err(Error::SourceChanged {
            path: relative_path.to_path_buf(),
        });
    }

    let existing_bounds = frontmatter_bounds(&original, relative_path)?;
    let existing_values = crate::parser::extract_frontmatter(&original)
        .0
        .and_then(|value| value.as_object().cloned());
    if let Some(owned_unset) = owned_unset {
        let unauthorized: Vec<_> = unset
            .iter()
            .filter(|field| {
                if set.contains_key(*field) || owned_unset.contains(*field) {
                    return false;
                }
                match (&existing_bounds, &existing_values) {
                    (None, _) => false,
                    (Some(_), Some(existing)) => existing.contains_key(*field),
                    // A malformed frontmatter block is ambiguous. Never let an
                    // unowned cleanup use that ambiguity as deletion authority.
                    (Some(_), None) => true,
                }
            })
            .cloned()
            .collect();
        if !unauthorized.is_empty() {
            return Err(Error::Config(format!(
                "refusing computed write to `{}`: module does not own frontmatter key(s) {}",
                relative_path.display(),
                unauthorized.join(", ")
            )));
        }

        let unauthorized_sets: Vec<_> = set
            .iter()
            .filter(|(field, value)| {
                if owned_unset.contains(*field) {
                    return false;
                }
                match (&existing_bounds, &existing_values) {
                    (None, _) => false,
                    (Some(_), Some(existing)) => existing
                        .get(*field)
                        .is_some_and(|existing| existing != *value),
                    // No exact semantic snapshot means there is no proof that
                    // a same-named key is absent or module-authored.
                    (Some(_), None) => true,
                }
            })
            .map(|(field, _)| field.clone())
            .collect();
        if !unauthorized_sets.is_empty() {
            return Err(Error::Config(format!(
                "refusing computed write to `{}`: module does not own existing frontmatter key(s) {}",
                relative_path.display(),
                unauthorized_sets.join(", ")
            )));
        }
    }

    let materialized_fields: BTreeSet<String> = set
        .iter()
        .filter(|(field, value)| {
            owned_unset.is_some_and(|owned| owned.contains(*field))
                || existing_values
                    .as_ref()
                    .and_then(|existing| existing.get(*field))
                    != Some(*value)
        })
        .map(|(field, _)| field.clone())
        .collect();
    let normalized_intent_fields = if let Some(fields) = intent_fields {
        let mut fields = fields.clone();
        normalize_committed_ownership(&mut fields, set, unset, &materialized_fields)?;
        Some(fields)
    } else {
        None
    };

    let rendered = render_patch(&original, relative_path, set, unset)?;
    if rendered == original {
        #[cfg(unix)]
        let file = {
            // Resolve the parent from the captured root before dependency
            // verification. The final owner read then remains the last
            // filesystem observation before accepting the no-op.
            let rebound = SecureSourceParent::open(&secure_root, relative_path)?;
            if !secure_parent.same_directory(&rebound)? {
                return Err(Error::SourceChanged {
                    path: relative_path.to_path_buf(),
                });
            }
            if let Some(guard) = pre_commit_guard {
                guard()?;
            }
            let final_rebound = SecureSourceParent::open(&secure_root, relative_path)?;
            if !secure_parent.same_directory(&final_rebound)? {
                return Err(Error::SourceChanged {
                    path: relative_path.to_path_buf(),
                });
            }
            let snapshot = secure_parent.read_source(relative_path)?;
            if compute_content_hash(&snapshot.source) != expected_content_hash {
                return Err(Error::SourceChanged {
                    path: relative_path.to_path_buf(),
                });
            }
            markdown_from_secure_snapshot(relative_path, &snapshot)
        };
        #[cfg(not(unix))]
        let file = {
            if let Some(guard) = pre_commit_guard {
                guard()?;
            }
            parse_markdown_file(project_root, relative_path)?
        };
        if file.content_hash != expected_content_hash {
            return Err(Error::SourceChanged {
                path: relative_path.to_path_buf(),
            });
        }
        return Ok(WritebackResult {
            file,
            changed: false,
            materialized_fields,
        });
    }

    #[cfg(unix)]
    let temporary = secure_parent.create_temporary(source_permissions, &rendered)?;
    #[cfg(unix)]
    let temporary_identity = temporary.identity()?;
    #[cfg(not(unix))]
    let mut temporary = tempfile::Builder::new()
        .prefix(".mdvdb-computed-")
        .suffix(".tmp")
        .tempfile_in(parent)?;
    #[cfg(not(unix))]
    {
        temporary.as_file().set_permissions(source_permissions)?;
        temporary.write_all(rendered.as_bytes())?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
    }

    // Rendering and syncing the temporary file can take long enough for an
    // editor save to land after the first CAS check.  Re-read immediately
    // before the atomic rename and refuse to replace a newer record.  This is
    // deliberately a full-content check (not mtime/size) because editors may
    // preserve either metadata value.
    #[cfg(unix)]
    let current = secure_parent.read_source(relative_path)?.source;
    #[cfg(not(unix))]
    let current = {
        let current = std::fs::read(&full_path)?;
        String::from_utf8(current).map_err(|_| Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "file is not valid UTF-8".to_string(),
        })?
    };
    if compute_content_hash(&current) != expected_content_hash {
        return Err(Error::SourceChanged {
            path: relative_path.to_path_buf(),
        });
    }
    let rendered_hash = compute_content_hash(&rendered);
    if let Some(fields) = normalized_intent_fields.as_ref() {
        record_computed_intent(
            project_root,
            #[cfg(unix)]
            &secure_root,
            relative_path,
            expected_content_hash,
            &rendered_hash,
            #[cfg(unix)]
            Some(temporary_identity.clone()),
            #[cfg(not(unix))]
            None,
            fields,
        )?;
    }
    #[cfg(unix)]
    {
        // The durable intent can take time and the dependency guard may touch
        // many files. Rebind the parent first, verify dependencies second, and
        // make the owner content CAS the final operation immediately before
        // the dirfd-relative rename.
        let rebound = SecureSourceParent::open(&secure_root, relative_path)?;
        if !secure_parent.same_directory(&rebound)? {
            return Err(Error::SourceChanged {
                path: relative_path.to_path_buf(),
            });
        }
        if let Some(guard) = pre_commit_guard {
            guard()?;
        }
        let final_rebound = SecureSourceParent::open(&secure_root, relative_path)?;
        if !secure_parent.same_directory(&final_rebound)? {
            return Err(Error::SourceChanged {
                path: relative_path.to_path_buf(),
            });
        }
        let current = secure_parent.read_source(relative_path)?.source;
        if compute_content_hash(&current) != expected_content_hash {
            return Err(Error::SourceChanged {
                path: relative_path.to_path_buf(),
            });
        }
        temporary.persist(&secure_parent.file_name)?;
    }
    #[cfg(not(unix))]
    {
        if let Some(guard) = pre_commit_guard {
            guard()?;
        }
        let current = std::fs::read(&full_path)?;
        let current = String::from_utf8(current).map_err(|_| Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "file is not valid UTF-8".to_string(),
        })?;
        if compute_content_hash(&current) != expected_content_hash {
            return Err(Error::SourceChanged {
                path: relative_path.to_path_buf(),
            });
        }
        temporary
            .persist(&full_path)
            .map_err(|error| Error::Io(error.error))?;
    }

    // The intent remains durable until the matching index generation commits,
    // so a directory-fsync failure is recoverable and must be reported rather
    // than allowing a non-durable source rename to retire the intent later.
    #[cfg(not(unix))]
    {
        if intent_fields.is_some() {
            sync_directory(parent)?;
        } else if let Ok(directory) = std::fs::File::open(parent) {
            let _ = directory.sync_all();
        }
    }

    #[cfg(unix)]
    let file = {
        let snapshot = secure_parent.read_source(relative_path)?;
        if snapshot.identity != temporary_identity {
            return Err(Error::SourceChanged {
                path: relative_path.to_path_buf(),
            });
        }
        markdown_from_secure_snapshot(relative_path, &snapshot)
    };
    #[cfg(not(unix))]
    let file = parse_markdown_file(project_root, relative_path)?;
    if file.content_hash != rendered_hash {
        return Err(Error::SourceChanged {
            path: relative_path.to_path_buf(),
        });
    }
    let expected_frontmatter = strict_frontmatter_map(&rendered, relative_path)?;
    let reparsed_frontmatter = file
        .frontmatter
        .as_ref()
        .and_then(JsonValue::as_object)
        .cloned()
        .unwrap_or_default();
    if reparsed_frontmatter != expected_frontmatter {
        return Err(Error::MarkdownParse {
            path: relative_path.to_path_buf(),
            message: "post-write frontmatter verification failed".to_string(),
        });
    }
    Ok(WritebackResult {
        file,
        changed: true,
        materialized_fields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(path: &Path, value: &str) {
        std::fs::write(path, value).unwrap();
    }

    #[test]
    fn writes_exact_json_numbers_and_preserves_unrelated_yaml() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("invoice.md");
        let original = "---\n# keep me\nprice: 0.10 # original\ntags:\n  - one\ntotal: 0 # formula note\n---\n# Body\n";
        write(&path, original);
        let expected = compute_content_hash(original);
        let exact: JsonValue = serde_json::from_str("0.3000000000000000000000000001").unwrap();

        let result = apply_frontmatter_patch(
            dir.path(),
            Path::new("invoice.md"),
            &expected,
            &BTreeMap::from([("total".to_string(), exact)]),
            &BTreeSet::new(),
        )
        .unwrap();

        assert!(result.changed);
        let rewritten = std::fs::read_to_string(path).unwrap();
        assert!(rewritten.contains("# keep me"), "{rewritten:?}");
        assert!(rewritten.contains("price: 0.10 # original"));
        assert!(rewritten.contains("tags:\n  - one"));
        assert!(rewritten.contains("total: 0.3000000000000000000000000001"));
        assert_eq!(rewritten.matches("total:").count(), 1, "{rewritten:?}");
        assert!(rewritten.contains("# formula note"));
        assert!(rewritten.ends_with("---\n# Body\n"));
    }

    #[test]
    fn replaces_a_spaced_computed_key_once_and_preserves_every_other_byte() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("invoice.md");
        let original = concat!(
            "---\n",
            "# account fields\n",
            "title: Invoice 1\n",
            "Client Name: \"Acme Corp\" # computed\n",
            "currency: EUR\n",
            "---\n",
            "\n",
            "# Invoice 1\n",
            "Body stays byte exact.\n",
        );
        write(&path, original);

        let result = apply_frontmatter_patch(
            dir.path(),
            Path::new("invoice.md"),
            &compute_content_hash(original),
            &BTreeMap::from([(
                "Client Name".to_string(),
                JsonValue::String("manufacturing".to_string()),
            )]),
            &BTreeSet::from(["Client Name".to_string()]),
        )
        .unwrap();
        assert!(result.changed);

        let rewritten = std::fs::read_to_string(&path).unwrap();
        assert_eq!(rewritten.matches("Client Name").count(), 1, "{rewritten:?}");
        assert!(rewritten.contains("\"Client Name\": \"manufacturing\" # computed"));
        assert!(rewritten.contains("# account fields\ntitle: Invoice 1\n"));
        assert!(rewritten.contains("\ncurrency: EUR\n---\n"));
        assert_eq!(
            body_without_frontmatter(original, Path::new("invoice.md")).unwrap(),
            body_without_frontmatter(&rewritten, Path::new("invoice.md")).unwrap()
        );
        assert_eq!(
            result.file.frontmatter.unwrap()["Client Name"],
            "manufacturing"
        );

        let second = apply_frontmatter_patch(
            dir.path(),
            Path::new("invoice.md"),
            &compute_content_hash(&rewritten),
            &BTreeMap::from([(
                "Client Name".to_string(),
                JsonValue::String("manufacturing".to_string()),
            )]),
            &BTreeSet::from(["Client Name".to_string()]),
        )
        .unwrap();
        assert!(!second.changed);
        assert_eq!(std::fs::read_to_string(path).unwrap(), rewritten);
    }

    #[test]
    fn equal_spaced_key_is_a_byte_exact_noop() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("contact.md");
        let original = "---\ntitle: Alice\nClient Name: manufacturing\n---\n\nBody\n";
        write(&path, original);

        let result = apply_frontmatter_patch(
            dir.path(),
            Path::new("contact.md"),
            &compute_content_hash(original),
            &BTreeMap::from([(
                "Client Name".to_string(),
                JsonValue::String("manufacturing".to_string()),
            )]),
            &BTreeSet::from(["Client Name".to_string()]),
        )
        .unwrap();

        assert!(!result.changed);
        assert!(result.materialized_fields.is_empty());
        assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    }

    #[test]
    fn canonicalizes_only_duplicate_occurrences_of_the_touched_computed_key() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("invoice.md");
        let damaged = concat!(
            "---\n",
            "title: Invoice 1\n",
            "Client Name: manufacturing\n",
            "Client Name: Acme Corp\n",
            "currency: EUR\n",
            "---\n",
            "Body\n",
        );
        write(&path, damaged);

        let repaired = apply_frontmatter_patch(
            dir.path(),
            Path::new("invoice.md"),
            &compute_content_hash(damaged),
            &BTreeMap::from([(
                "Client Name".to_string(),
                JsonValue::String("manufacturing".to_string()),
            )]),
            &BTreeSet::from(["Client Name".to_string()]),
        )
        .unwrap();
        assert!(repaired.changed);
        let repaired_source = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            repaired_source.matches("Client Name").count(),
            1,
            "{repaired_source:?}"
        );
        assert_eq!(
            repaired.file.frontmatter.as_ref().unwrap()["Client Name"],
            "manufacturing"
        );
        assert!(repaired_source.contains("title: Invoice 1\n"));
        assert!(repaired_source.contains("currency: EUR\n"));
        assert!(repaired_source.ends_with("---\nBody\n"));

        let removed = apply_frontmatter_patch(
            dir.path(),
            Path::new("invoice.md"),
            &compute_content_hash(&repaired_source),
            &BTreeMap::new(),
            &BTreeSet::from(["Client Name".to_string()]),
        )
        .unwrap();
        assert!(removed.changed);
        let removed_source = std::fs::read_to_string(path).unwrap();
        assert!(!removed_source.contains("Client Name"));
        assert_eq!(
            removed.file.frontmatter.as_ref().unwrap()["title"],
            "Invoice 1"
        );
        assert_eq!(
            removed.file.frontmatter.as_ref().unwrap()["currency"],
            "EUR"
        );
    }

    #[test]
    fn unrelated_duplicate_or_partial_yaml_fails_closed_without_changing_bytes() {
        let dir = TempDir::new().unwrap();
        for (name, original) in [
            (
                "duplicate.md",
                "---\nduplicate: 1\nduplicate: 2\ncomputed: old\n---\nBody\n",
            ),
            (
                "partial.md",
                "---\ntitle: Safe\nOther Name: keep exactly\ncomputed: old\n---\nBody\n",
            ),
        ] {
            let path = dir.path().join(name);
            write(&path, original);
            let result = apply_frontmatter_patch(
                dir.path(),
                Path::new(name),
                &compute_content_hash(original),
                &BTreeMap::from([("computed".to_string(), serde_json::json!("new"))]),
                &BTreeSet::from(["computed".to_string()]),
            );
            assert!(matches!(result, Err(Error::MarkdownParse { .. })), "{name}");
            assert_eq!(std::fs::read_to_string(path).unwrap(), original, "{name}");
        }
    }

    #[test]
    fn module_patch_cannot_remove_unowned_or_all_frontmatter_keys() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("record.md");
        let original = "---\ntitle: Never delete me\nstatus: active\ncomputed: old\n---\nBody\n";
        write(&path, original);
        let unset = BTreeSet::from([
            "title".to_string(),
            "status".to_string(),
            "computed".to_string(),
        ]);
        let owned = BTreeSet::from(["computed".to_string()]);

        let result = apply_frontmatter_patch_with_intent(
            dir.path(),
            Path::new("record.md"),
            &compute_content_hash(original),
            &BTreeMap::new(),
            &unset,
            &owned,
            &HashMap::new(),
        );

        assert!(matches!(result, Err(Error::Config(message)) if message.contains("does not own")));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert!(!computed_intent_path(dir.path()).exists());

        let harmless_missing_cleanup = apply_frontmatter_patch_with_intent(
            dir.path(),
            Path::new("record.md"),
            &compute_content_hash(original),
            &BTreeMap::new(),
            &BTreeSet::from(["never_materialized".to_string()]),
            &BTreeSet::new(),
            &HashMap::new(),
        )
        .unwrap();
        assert!(!harmless_missing_cleanup.changed);
        assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    }

    #[test]
    fn module_set_cannot_overwrite_or_claim_an_existing_ordinary_key() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("record.md");
        let original = "---\ntitle: Safe\nordinary: user-authored\n---\nBody\n";
        write(&path, original);
        let fields = HashMap::from([(
            "ordinary".to_string(),
            ComputedFieldEntry {
                module: "lookup_rollup".to_string(),
                definition_fingerprint: "definition".to_string(),
                input_fingerprint: Some("inputs".to_string()),
                dependency_snapshot: ComputedDependencySnapshot::default(),
                value_json: Some("\"computed\"".to_string()),
                materialized_value_json: None,
                diagnostic: None,
            },
        )]);

        let result = apply_frontmatter_patch_with_intent(
            dir.path(),
            Path::new("record.md"),
            &compute_content_hash(original),
            &BTreeMap::from([("ordinary".to_string(), serde_json::json!("computed"))]),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &fields,
        );

        assert!(matches!(result, Err(Error::Config(message)) if message.contains("does not own")));
        assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
        assert!(!computed_intent_path(dir.path()).exists());
    }

    #[test]
    fn module_set_may_create_an_absent_key_but_never_claims_an_equal_ordinary_value() {
        let dir = TempDir::new().unwrap();
        let absent_path = dir.path().join("absent.md");
        let absent_original = "---\ntitle: Safe\n---\nBody\n";
        write(&absent_path, absent_original);
        let new_fields = HashMap::from([(
            "computed".to_string(),
            ComputedFieldEntry {
                module: "lookup_rollup".to_string(),
                definition_fingerprint: "definition".to_string(),
                input_fingerprint: Some("inputs".to_string()),
                dependency_snapshot: ComputedDependencySnapshot::default(),
                value_json: Some("2".to_string()),
                materialized_value_json: None,
                diagnostic: None,
            },
        )]);
        let created = apply_frontmatter_patch_with_intent(
            dir.path(),
            Path::new("absent.md"),
            &compute_content_hash(absent_original),
            &BTreeMap::from([("computed".to_string(), serde_json::json!(2))]),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &new_fields,
        )
        .unwrap();
        assert!(created.changed);
        assert!(created.materialized_fields.contains("computed"));

        let equal_path = dir.path().join("equal.md");
        let equal_original = "---\ntitle: Safe\nordinary: same\n---\nBody\n";
        write(&equal_path, equal_original);
        let equal_fields = HashMap::from([(
            "ordinary".to_string(),
            ComputedFieldEntry {
                module: "lookup_rollup".to_string(),
                definition_fingerprint: "definition".to_string(),
                input_fingerprint: Some("inputs".to_string()),
                dependency_snapshot: ComputedDependencySnapshot::default(),
                value_json: Some("\"same\"".to_string()),
                materialized_value_json: None,
                diagnostic: None,
            },
        )]);
        let equal = apply_frontmatter_patch_with_intent(
            dir.path(),
            Path::new("equal.md"),
            &compute_content_hash(equal_original),
            &BTreeMap::from([("ordinary".to_string(), serde_json::json!("same"))]),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &equal_fields,
        )
        .unwrap();
        assert!(!equal.changed);
        assert!(equal.materialized_fields.is_empty());
        assert_eq!(
            std::fs::read(&equal_path).unwrap(),
            equal_original.as_bytes()
        );
    }

    #[test]
    fn spaced_computed_key_preserves_an_arbitrary_precision_number() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("precise.md");
        let original = "---\ntitle: Precise\nPrecise Total: 0\n---\nBody\n";
        write(&path, original);
        let exact: JsonValue = serde_json::from_str("0.3000000000000000000000000001").unwrap();

        let result = apply_frontmatter_patch(
            dir.path(),
            Path::new("precise.md"),
            &compute_content_hash(original),
            &BTreeMap::from([("Precise Total".to_string(), exact.clone())]),
            &BTreeSet::from(["Precise Total".to_string()]),
        )
        .unwrap();
        let rewritten = std::fs::read_to_string(path).unwrap();
        assert!(rewritten.contains("\"Precise Total\": 0.3000000000000000000000000001"));
        assert_eq!(rewritten.matches("Precise Total").count(), 1);
        assert_eq!(result.file.frontmatter.unwrap()["Precise Total"], exact);
    }

    #[test]
    fn no_op_does_not_rewrite_and_removal_keeps_an_empty_frontmatter_boundary() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("generated.md");
        let original = "---\ntotal: 3\n---\nBody";
        write(&path, original);
        let expected = compute_content_hash(original);
        let value = serde_json::json!(3);

        let no_op = apply_frontmatter_patch(
            dir.path(),
            Path::new("generated.md"),
            &expected,
            &BTreeMap::from([("total".to_string(), value)]),
            &BTreeSet::from(["total".to_string()]),
        )
        .unwrap();
        assert!(!no_op.changed, "{:?}", no_op.file.frontmatter);

        let removed = apply_frontmatter_patch(
            dir.path(),
            Path::new("generated.md"),
            &expected,
            &BTreeMap::new(),
            &BTreeSet::from(["total".to_string()]),
        )
        .unwrap();
        assert!(removed.changed);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "---\n\n---\nBody");
        assert!(removed.file.frontmatter.is_none());
    }

    #[test]
    fn rendered_patch_validator_refuses_to_drop_an_existing_frontmatter_envelope() {
        let relative_path = Path::new("record.md");
        let original = "---\ntotal: 3\n---\nBody\n";
        let canonical_yaml = "total: 3\n";
        let canonical_before = strict_frontmatter_map(original, relative_path).unwrap();
        let set = BTreeMap::<String, JsonValue>::new();
        let unset = BTreeSet::from(["total".to_string()]);

        let result = verify_rendered_patch(
            original,
            "Body\n",
            relative_path,
            canonical_yaml,
            &canonical_before,
            &set,
            &unset,
        );

        assert!(
            matches!(result, Err(Error::MarkdownParse { message, .. }) if message.contains("remove the frontmatter envelope"))
        );
    }

    #[test]
    fn indented_delimiter_inside_block_scalar_is_never_treated_as_frontmatter_end() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("block-scalar.md");
        let original = concat!(
            "---\n",
            "title: Keep\n",
            "notes: |-\n",
            "  before\n",
            "  ---\n",
            "  after\n",
            "computed: 1\n",
            "ordinary: safe\n",
            "---\n",
            "# Body\n",
            "Unchanged.\n",
        );
        write(&path, original);

        let (frontmatter, body) = crate::parser::extract_frontmatter(original);
        let frontmatter = frontmatter.unwrap();
        assert_eq!(frontmatter["ordinary"], "safe");
        assert_eq!(frontmatter["notes"], "before\n---\nafter");
        assert_eq!(body, "# Body\nUnchanged.\n");

        let result = apply_frontmatter_patch(
            dir.path(),
            Path::new("block-scalar.md"),
            &compute_content_hash(original),
            &BTreeMap::from([("computed".to_string(), serde_json::json!(2))]),
            &BTreeSet::from(["computed".to_string()]),
        );

        // yaml-edit currently rejects an indented delimiter inside a valid
        // block scalar as a multi-document stream. Refusal is acceptable;
        // treating that line as the envelope boundary and rewriting is not.
        assert!(matches!(result, Err(Error::MarkdownParse { .. })));
        assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
    }

    #[test]
    fn suffix_bearing_delimiter_lines_are_rejected_without_rewriting_bytes() {
        let dir = TempDir::new().unwrap();
        for (name, suffix) in [
            ("comment.md", "--- #comment"),
            ("garbage.md", "--- garbage"),
        ] {
            let path = dir.path().join(name);
            let original =
                format!("---\ntitle: Safe\n{suffix}\ncomputed: 1\nordinary: keep\n---\nBody\n");
            write(&path, &original);

            let result = apply_frontmatter_patch(
                dir.path(),
                Path::new(name),
                &compute_content_hash(&original),
                &BTreeMap::from([("computed".to_string(), serde_json::json!(2))]),
                &BTreeSet::from(["computed".to_string()]),
            );

            assert!(matches!(result, Err(Error::MarkdownParse { .. })), "{name}");
            assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes(), "{name}");
        }
    }

    #[test]
    fn module_intent_last_field_removal_keeps_the_frontmatter_envelope() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("generated.md");
        let original = "---\ntotal: 3\n---\nBody\n";
        write(&path, original);

        let result = apply_frontmatter_patch_with_intent(
            dir.path(),
            Path::new("generated.md"),
            &compute_content_hash(original),
            &BTreeMap::new(),
            &BTreeSet::from(["total".to_string()]),
            &BTreeSet::from(["total".to_string()]),
            &HashMap::new(),
        )
        .unwrap();

        assert!(result.changed);
        let rewritten = std::fs::read_to_string(path).unwrap();
        assert_eq!(rewritten, "---\n\n---\nBody\n");
        assert_eq!(rewritten.lines().filter(|line| *line == "---").count(), 2);
        assert!(computed_intent_path(dir.path()).is_file());
    }

    #[test]
    fn final_commit_guard_prevents_dependency_and_owner_race_overwrites() {
        let fields_for = || {
            HashMap::from([(
                "computed".to_string(),
                ComputedFieldEntry {
                    module: "lookup_rollup".to_string(),
                    definition_fingerprint: "definition".to_string(),
                    input_fingerprint: Some("inputs".to_string()),
                    dependency_snapshot: ComputedDependencySnapshot::default(),
                    value_json: Some("2".to_string()),
                    materialized_value_json: Some("2".to_string()),
                    diagnostic: None,
                },
            )])
        };
        let set = BTreeMap::from([("computed".to_string(), serde_json::json!(2))]);
        let owned = BTreeSet::new();

        let dependency_dir = TempDir::new().unwrap();
        let dependency_path = dependency_dir.path().join("owner.md");
        let dependency_original = "---\ntitle: Safe\n---\nBody\n";
        write(&dependency_path, dependency_original);
        let dependency_guard = || {
            Err(Error::DependencyChanged {
                dependency: "clients/acme.md".to_string(),
            })
        };
        let dependency_result = apply_frontmatter_patch_with_intent_and_guard(
            dependency_dir.path(),
            Path::new("owner.md"),
            &compute_content_hash(dependency_original),
            &set,
            &BTreeSet::new(),
            ComputedWriteContext {
                owned_unset: &owned,
                fields: &fields_for(),
                pre_commit_guard: Some(&dependency_guard),
            },
        );
        assert!(matches!(
            dependency_result,
            Err(Error::DependencyChanged { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(dependency_path).unwrap(),
            dependency_original
        );

        let owner_dir = TempDir::new().unwrap();
        let owner_path = owner_dir.path().join("owner.md");
        let owner_original = "---\ntitle: Safe\n---\nBody\n";
        let concurrent_edit = "---\ntitle: User edit\n---\nBody\n";
        write(&owner_path, owner_original);
        let owner_guard = || {
            write(&owner_path, concurrent_edit);
            Ok(())
        };
        let owner_result = apply_frontmatter_patch_with_intent_and_guard(
            owner_dir.path(),
            Path::new("owner.md"),
            &compute_content_hash(owner_original),
            &set,
            &BTreeSet::new(),
            ComputedWriteContext {
                owned_unset: &owned,
                fields: &fields_for(),
                pre_commit_guard: Some(&owner_guard),
            },
        );
        assert!(matches!(owner_result, Err(Error::SourceChanged { .. })));
        assert_eq!(
            std::fs::read_to_string(owner_path).unwrap(),
            concurrent_edit
        );
    }

    #[test]
    fn scalar_like_computed_output_keys_are_quoted_and_decode_as_strings() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("keys.md");
        let original = "---\ntitle: Keys\n---\nBody\n";
        write(&path, original);
        let fields = ["true", "null", "123", "2026-08-02", "-"];
        let set = fields
            .iter()
            .map(|field| ((*field).to_string(), serde_json::json!(field)))
            .collect();

        let result = apply_frontmatter_patch(
            dir.path(),
            Path::new("keys.md"),
            &compute_content_hash(original),
            &set,
            &BTreeSet::new(),
        )
        .unwrap();

        let rewritten = std::fs::read_to_string(path).unwrap();
        let frontmatter = result.file.frontmatter.unwrap();
        for field in fields {
            assert!(
                rewritten
                    .lines()
                    .any(|line| line.starts_with(&format!("\"{field}\":"))),
                "computed key must remain a YAML string: {rewritten:?}"
            );
            assert_eq!(frontmatter[field], field);
        }
    }

    #[test]
    fn trailing_yaml_blank_lines_and_footer_comments_remain_byte_exact() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("footer.md");
        let original = concat!(
            "---\n",
            "title: Keep\n",
            "computed: old\n",
            "# footer one\n",
            "# footer two\n",
            "\n",
            "\n",
            "---\n",
            "Body stays exact.\n",
            "\n",
        );
        let preserved_tail = "# footer one\n# footer two\n\n\n---\nBody stays exact.\n\n";
        write(&path, original);

        apply_frontmatter_patch(
            dir.path(),
            Path::new("footer.md"),
            &compute_content_hash(original),
            &BTreeMap::from([("computed".to_string(), serde_json::json!("new"))]),
            &BTreeSet::from(["computed".to_string()]),
        )
        .unwrap();

        let rewritten = std::fs::read_to_string(path).unwrap();
        assert!(rewritten.ends_with(preserved_tail), "{rewritten:?}");
        assert_eq!(
            body_without_frontmatter(original, Path::new("footer.md")).unwrap(),
            body_without_frontmatter(&rewritten, Path::new("footer.md")).unwrap()
        );
    }

    #[test]
    fn mixed_newline_frontmatter_fails_closed_without_changing_source_bytes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("mixed.md");
        let original =
            "---\r\ntitle: Keep\r\ncomputed: old\n# footer\r\n---\r\nBody stays exact.\n";
        write(&path, original);

        let result = apply_frontmatter_patch(
            dir.path(),
            Path::new("mixed.md"),
            &compute_content_hash(original),
            &BTreeMap::from([("computed".to_string(), serde_json::json!("new"))]),
            &BTreeSet::from(["computed".to_string()]),
        );

        assert!(
            matches!(result, Err(Error::MarkdownParse { message, .. }) if message.contains("mixed newline styles"))
        );
        assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
        assert!(!computed_intent_path(dir.path()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlinked_ancestor_that_escapes_the_project() {
        use std::os::unix::fs::symlink;

        let project = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let outside_path = outside.path().join("record.md");
        let original = "---\ntitle: Outside\ncomputed: old\n---\nBody\n";
        write(&outside_path, original);
        symlink(outside.path(), project.path().join("escape")).unwrap();

        let result = apply_frontmatter_patch(
            project.path(),
            Path::new("escape/record.md"),
            &compute_content_hash(original),
            &BTreeMap::from([("computed".to_string(), serde_json::json!("new"))]),
            &BTreeSet::from(["computed".to_string()]),
        );

        assert!(
            matches!(result, Err(Error::Config(message)) if message.contains("ancestor outside project"))
        );
        assert_eq!(std::fs::read(&outside_path).unwrap(), original.as_bytes());
        assert!(!computed_intent_path(project.path()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn ancestor_symlink_swap_before_commit_cannot_redirect_the_atomic_rename() {
        use std::os::unix::fs::symlink;

        let project = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let owner_dir = project.path().join("owners");
        let relocated_dir = project.path().join("owners-relocated");
        std::fs::create_dir(&owner_dir).unwrap();
        let owner_path = owner_dir.join("record.md");
        let outside_path = outside.path().join("record.md");
        let original = "---\ntitle: Safe\ncomputed: 1\n---\nBody\n";
        write(&owner_path, original);
        write(&outside_path, original);

        let fields = HashMap::from([(
            "computed".to_string(),
            ComputedFieldEntry {
                module: "lookup_rollup".to_string(),
                definition_fingerprint: "definition".to_string(),
                input_fingerprint: Some("inputs".to_string()),
                dependency_snapshot: ComputedDependencySnapshot::default(),
                value_json: Some("2".to_string()),
                materialized_value_json: None,
                diagnostic: None,
            },
        )]);
        let owned = BTreeSet::from(["computed".to_string()]);
        let guard = || {
            std::fs::rename(&owner_dir, &relocated_dir)?;
            symlink(outside.path(), &owner_dir)?;

            // NamedTempFile persists by pathname. Mirror the observed temp
            // name into the swapped directory to model an adversary racing
            // that pathname after the writer's initial canonicalization.
            let temporary = std::fs::read_dir(&relocated_dir)?
                .filter_map(std::result::Result::ok)
                .find(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".mdvdb-computed-")
                })
                .ok_or_else(|| Error::Config("computed temporary file was not found".into()))?;
            std::fs::hard_link(temporary.path(), outside.path().join(temporary.file_name()))?;
            Ok(())
        };

        let result = apply_frontmatter_patch_with_intent_and_guard(
            project.path(),
            Path::new("owners/record.md"),
            &compute_content_hash(original),
            &BTreeMap::from([("computed".to_string(), serde_json::json!(2))]),
            &BTreeSet::new(),
            ComputedWriteContext {
                owned_unset: &owned,
                fields: &fields,
                pre_commit_guard: Some(&guard),
            },
        );

        assert!(result.is_err(), "ancestor swap must abort the write");
        assert_eq!(std::fs::read(&outside_path).unwrap(), original.as_bytes());
        assert_eq!(
            std::fs::read(relocated_dir.join("record.md")).unwrap(),
            original.as_bytes()
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_hard_linked_record_without_touching_the_other_name() {
        let project = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let outside_path = outside.path().join("record.md");
        let project_path = project.path().join("record.md");
        let original = "---\ntitle: Outside\ncomputed: old\n---\nBody\n";
        write(&outside_path, original);
        std::fs::hard_link(&outside_path, &project_path).unwrap();

        let result = apply_frontmatter_patch(
            project.path(),
            Path::new("record.md"),
            &compute_content_hash(original),
            &BTreeMap::from([("computed".to_string(), serde_json::json!("new"))]),
            &BTreeSet::from(["computed".to_string()]),
        );

        assert!(
            matches!(result, Err(Error::Config(message)) if message.contains("hard-linked record"))
        );
        assert_eq!(std::fs::read(&project_path).unwrap(), original.as_bytes());
        assert_eq!(std::fs::read(&outside_path).unwrap(), original.as_bytes());
        assert!(!computed_intent_path(project.path()).exists());
    }

    #[test]
    fn setting_a_comments_only_frontmatter_preserves_every_prior_byte_and_order() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("comments.md");
        let original = concat!(
            "---\n",
            "# heading comment\n",
            "\n",
            "# footer comment\n",
            "\n",
            "---\n",
            "# Body\n",
            "Unchanged.\n",
        );
        write(&path, original);

        let result = apply_frontmatter_patch(
            dir.path(),
            Path::new("comments.md"),
            &compute_content_hash(original),
            &BTreeMap::from([("computed".to_string(), serde_json::json!(7))]),
            &BTreeSet::new(),
        );
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
                panic!("comments-only frontmatter must accept a lossless set: {error}");
            }
        };

        let rewritten = std::fs::read_to_string(path).unwrap();
        assert_eq!(rewritten.matches("computed: 7").count(), 1, "{rewritten:?}");
        assert_eq!(rewritten.replacen("computed: 7\n", "", 1), original);
        assert_eq!(
            body_without_frontmatter(original, Path::new("comments.md")).unwrap(),
            body_without_frontmatter(&rewritten, Path::new("comments.md")).unwrap()
        );
        assert_eq!(result.file.frontmatter.unwrap()["computed"], 7);
    }

    #[test]
    fn refuses_stale_or_malformed_sources() {
        let dir = TempDir::new().unwrap();
        write(&dir.path().join("stale.md"), "value");
        let stale = apply_frontmatter_patch(
            dir.path(),
            Path::new("stale.md"),
            "wrong",
            &BTreeMap::from([("total".to_string(), serde_json::json!(1))]),
            &BTreeSet::new(),
        );
        assert!(matches!(stale, Err(Error::SourceChanged { .. })));

        let malformed = "---\nitems: [\n---\nBody";
        write(&dir.path().join("malformed.md"), malformed);
        let result = apply_frontmatter_patch(
            dir.path(),
            Path::new("malformed.md"),
            &compute_content_hash(malformed),
            &BTreeMap::from([("total".to_string(), serde_json::json!(1))]),
            &BTreeSet::new(),
        );
        assert!(matches!(result, Err(Error::MarkdownParse { .. })));
    }

    #[test]
    fn preserves_bom_crlf_body_and_writes_nested_values() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested.md");
        let original =
            "\u{feff}---\r\n# settings\r\nname: Example\r\n---\r\n# Body\r\nunchanged\r\n";
        write(&path, original);
        let nested: JsonValue =
            serde_json::from_str(r#"{"amounts":[0.1000000000000000000000000001,2],"active":true}"#)
                .unwrap();

        let result = apply_frontmatter_patch(
            dir.path(),
            Path::new("nested.md"),
            &compute_content_hash(original),
            &BTreeMap::from([("summary".to_string(), nested.clone())]),
            &BTreeSet::new(),
        )
        .unwrap();
        assert!(result.changed);

        let rewritten = std::fs::read_to_string(path).unwrap();
        assert!(rewritten.starts_with("\u{feff}---\r\n# settings\r\n"));
        assert!(rewritten.ends_with("---\r\n# Body\r\nunchanged\r\n"));
        assert!(!rewritten.replace("\r\n", "").contains('\n'));
        assert_eq!(result.file.frontmatter.unwrap()["summary"], nested);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_replacement_preserves_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("private.md");
        let original = "---\nname: private\n---\nBody\n";
        write(&path, original);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

        apply_frontmatter_patch(
            dir.path(),
            Path::new("private.md"),
            &compute_content_hash(original),
            &BTreeMap::from([("computed".to_string(), serde_json::json!(true))]),
            &BTreeSet::new(),
        )
        .unwrap();

        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[test]
    fn durable_intent_rolls_provenance_forward_after_interrupted_index_save() {
        let dir = TempDir::new().unwrap();
        let state_dir = dir.path().join(".markdownvdb");
        std::fs::create_dir_all(&state_dir).unwrap();
        let source_path = dir.path().join("invoice.md");
        let original = "---\nprice: 2\n---\nBody\n";
        write(&source_path, original);

        let index_path = state_dir.join("index");
        let index = Index::create(
            &index_path,
            &crate::index::types::EmbeddingConfig {
                provider: "test".to_string(),
                model: "test".to_string(),
                dimensions: 2,
            },
        )
        .unwrap();
        let parsed = parse_markdown_file(dir.path(), Path::new("invoice.md")).unwrap();
        index.upsert(&parsed, &[], &[]).unwrap();
        index.save().unwrap();

        let fields = HashMap::from([(
            "total".to_string(),
            ComputedFieldEntry {
                module: "lookup_rollup".to_string(),
                definition_fingerprint: "definition".to_string(),
                input_fingerprint: Some("dependencies".to_string()),
                dependency_snapshot: ComputedDependencySnapshot::owner(
                    "clients/acme.md",
                    "client-content-hash",
                ),
                value_json: Some("4".to_string()),
                materialized_value_json: Some("4".to_string()),
                diagnostic: None,
            },
        )]);
        let writeback = apply_frontmatter_patch_with_intent(
            dir.path(),
            Path::new("invoice.md"),
            &parsed.content_hash,
            &BTreeMap::from([("total".to_string(), serde_json::json!(4))]),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &fields,
        )
        .unwrap();
        assert!(writeback.changed);
        assert!(computed_intent_path(dir.path()).is_file());

        // Simulate termination after Markdown rename but before Index::save().
        drop(index);
        let recovered = Index::open(&index_path).unwrap();
        assert_ne!(
            recovered.get_file("invoice.md").unwrap().content_hash,
            writeback.file.content_hash
        );
        assert!(recover_computed_intents(dir.path(), &recovered).unwrap());
        let recovered_file = recovered.get_file("invoice.md").unwrap();
        assert_eq!(recovered_file.content_hash, writeback.file.content_hash);
        assert_eq!(
            recovered_file.computed_fields["total"]
                .input_fingerprint
                .as_deref(),
            Some("dependencies")
        );
        let recovered_snapshot = &recovered_file.computed_fields["total"].dependency_snapshot;
        assert_eq!(
            recovered_snapshot.paths["clients/acme.md"]
                .content_hash
                .as_deref(),
            Some("client-content-hash")
        );

        // A second interruption after the index commit but before intent removal
        // is harmless: replay recognizes the post-write generation.
        recovered.save().unwrap();
        drop(recovered);
        let replayed = Index::open(&index_path).unwrap();
        assert!(recover_computed_intents(dir.path(), &replayed).unwrap());
        assert_eq!(
            replayed.get_computed_fields("invoice.md").unwrap()["total"].value_json,
            Some("4".to_string())
        );
        replayed.save().unwrap();
        finish_computed_intents(dir.path(), &replayed).unwrap();
        assert!(!computed_intent_path(dir.path()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn aborted_intent_never_claims_independently_written_identical_bytes() {
        use std::os::unix::fs::MetadataExt;

        let dir = TempDir::new().unwrap();
        let source_path = dir.path().join("invoice.md");
        let original = "---\nprice: 2\n---\nBody\n";
        write(&source_path, original);

        let index = Index::create(
            &dir.path().join("index"),
            &crate::index::types::EmbeddingConfig {
                provider: "test".to_string(),
                model: "test".to_string(),
                dimensions: 2,
            },
        )
        .unwrap();
        let parsed = parse_markdown_file(dir.path(), Path::new("invoice.md")).unwrap();
        index.upsert(&parsed, &[], &[]).unwrap();

        let fields = HashMap::from([(
            "total".to_string(),
            ComputedFieldEntry {
                module: "lookup_rollup".to_string(),
                definition_fingerprint: "definition".to_string(),
                input_fingerprint: Some("dependencies".to_string()),
                dependency_snapshot: ComputedDependencySnapshot::default(),
                value_json: Some("4".to_string()),
                materialized_value_json: Some("4".to_string()),
                diagnostic: None,
            },
        )]);
        let set = BTreeMap::from([("total".to_string(), serde_json::json!(4))]);
        let rendered =
            render_patch(original, Path::new("invoice.md"), &set, &BTreeSet::new()).unwrap();
        let guard = || {
            Err(Error::DependencyChanged {
                dependency: "clients/acme.md".to_string(),
            })
        };
        let result = apply_frontmatter_patch_with_intent_and_guard(
            dir.path(),
            Path::new("invoice.md"),
            &parsed.content_hash,
            &set,
            &BTreeSet::new(),
            ComputedWriteContext {
                owned_unset: &BTreeSet::new(),
                fields: &fields,
                pre_commit_guard: Some(&guard),
            },
        );
        assert!(matches!(result, Err(Error::DependencyChanged { .. })));
        assert_eq!(std::fs::read_to_string(&source_path).unwrap(), original);

        let intent = read_computed_intents(dir.path())
            .unwrap()
            .unwrap()
            .entries
            .remove("invoice.md")
            .unwrap();
        let planned_identity = intent.after_file_identity.unwrap();

        // A separate writer can legitimately produce the exact bytes that our
        // abandoned temporary held. Hash equality must not turn that ordinary
        // edit into module ownership.
        std::fs::write(&source_path, &rendered).unwrap();
        let independent = std::fs::metadata(&source_path).unwrap();
        assert_ne!(
            (planned_identity.device, planned_identity.inode),
            (independent.dev(), independent.ino())
        );

        assert!(recover_computed_intents(dir.path(), &index).unwrap());
        let stored = index.get_file("invoice.md").unwrap();
        assert_eq!(stored.content_hash, compute_content_hash(&rendered));
        assert!(stored.computed_fields.is_empty());
        assert_eq!(
            serde_json::from_str::<JsonValue>(stored.frontmatter.as_deref().unwrap()).unwrap()
                ["total"],
            JsonValue::from(4)
        );
    }

    #[cfg(unix)]
    #[test]
    fn captured_project_root_prevents_intent_redirection_after_root_retarget() {
        use std::os::unix::fs::symlink;

        let container = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let project = container.path().join("project");
        let relocated = container.path().join("project-relocated");
        std::fs::create_dir(&project).unwrap();

        let secure_root = SecureProjectRoot::open(&project).unwrap();
        std::fs::rename(&project, &relocated).unwrap();
        symlink(outside.path(), &project).unwrap();

        let log = ComputedWriteIntentLog {
            version: COMPUTED_INTENT_VERSION,
            entries: BTreeMap::new(),
        };
        write_computed_intents_with_root(&secure_root, &log).unwrap();

        assert!(relocated
            .join(".markdownvdb")
            .join(COMPUTED_INTENT_FILE)
            .is_file());
        assert!(!outside.path().join(".markdownvdb").exists());
        assert_eq!(
            read_computed_intents_with_root(&relocated, &secure_root)
                .unwrap()
                .unwrap()
                .version,
            COMPUTED_INTENT_VERSION
        );
    }

    #[cfg(unix)]
    #[test]
    fn fifo_record_is_rejected_without_blocking_or_creating_intent_state() {
        use std::os::unix::fs::FileTypeExt;

        let dir = TempDir::new().unwrap();
        let fifo = dir.path().join("record.md");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(std::fs::symlink_metadata(&fifo)
            .unwrap()
            .file_type()
            .is_fifo());

        let result = apply_frontmatter_patch(
            dir.path(),
            Path::new("record.md"),
            &compute_content_hash(""),
            &BTreeMap::from([("computed".to_string(), JsonValue::from(1))]),
            &BTreeSet::new(),
        );
        assert!(matches!(result, Err(Error::Config(message)) if message.contains("non-file")));
        assert!(!dir.path().join(".markdownvdb").exists());
        assert!(std::fs::symlink_metadata(fifo)
            .unwrap()
            .file_type()
            .is_fifo());
    }

    #[cfg(unix)]
    #[test]
    fn computed_intent_symlinks_never_read_write_or_delete_outside_the_project() {
        use std::os::unix::fs::symlink;

        let outside = TempDir::new().unwrap();
        let outside_intent = outside.path().join(COMPUTED_INTENT_FILE);
        let sentinel = br#"{"version":1,"entries":{}}"#;
        std::fs::write(&outside_intent, sentinel).unwrap();

        let make_index = |root: &Path| {
            Index::create(
                &root.join("safe-index"),
                &crate::index::types::EmbeddingConfig {
                    provider: "test".to_string(),
                    model: "test".to_string(),
                    dimensions: 2,
                },
            )
            .unwrap()
        };
        let log = ComputedWriteIntentLog {
            version: COMPUTED_INTENT_VERSION,
            entries: BTreeMap::new(),
        };

        let directory_link_project = TempDir::new().unwrap();
        symlink(
            outside.path(),
            directory_link_project.path().join(".markdownvdb"),
        )
        .unwrap();
        let directory_index = make_index(directory_link_project.path());
        assert!(matches!(
            read_computed_intents(directory_link_project.path()),
            Err(Error::Config(_))
        ));
        assert!(matches!(
            write_computed_intents(directory_link_project.path(), &log),
            Err(Error::Config(_))
        ));
        assert!(matches!(
            finish_computed_intents(directory_link_project.path(), &directory_index),
            Err(Error::Config(_))
        ));
        assert_eq!(std::fs::read(&outside_intent).unwrap(), sentinel);
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 1);

        let file_link_project = TempDir::new().unwrap();
        let state_dir = file_link_project.path().join(".markdownvdb");
        std::fs::create_dir(&state_dir).unwrap();
        symlink(&outside_intent, state_dir.join(COMPUTED_INTENT_FILE)).unwrap();
        let file_index = make_index(file_link_project.path());
        assert!(matches!(
            read_computed_intents(file_link_project.path()),
            Err(Error::Config(_))
        ));
        assert!(matches!(
            write_computed_intents(file_link_project.path(), &log),
            Err(Error::Config(_))
        ));
        assert!(matches!(
            finish_computed_intents(file_link_project.path(), &file_index),
            Err(Error::Config(_))
        ));
        assert!(state_dir.join(COMPUTED_INTENT_FILE).is_symlink());
        assert_eq!(std::fs::read(&outside_intent).unwrap(), sentinel);
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 1);
    }

    #[test]
    fn legacy_write_intent_entries_default_dependency_snapshots() {
        let entry: IntentComputedFieldEntry = serde_json::from_value(serde_json::json!({
            "module": "formula",
            "definition_fingerprint": "definition",
            "input_fingerprint": "inputs",
            "value_json": "4",
            "diagnostic": null
        }))
        .unwrap();

        assert!(entry.dependency_snapshot.paths.is_empty());
        assert!(entry.dependency_snapshot.incoming_scopes.is_empty());
        assert!(entry.materialized_value_json.is_none());

        let intent: ComputedWriteIntent = serde_json::from_value(serde_json::json!({
            "before_content_hash": "before",
            "after_content_hash": "after",
            "fields": {}
        }))
        .unwrap();
        assert!(intent.after_file_identity.is_none());
    }
}
