//! Bounded live-file operations for one physical sandbox generation.
//!
//! The caller performs the generation fence before entering this module. Paths are absolute guest
//! paths rooted at `/workspace`; this module never falls back to a persisted manifest and never
//! materializes a target.

use std::io::{Read as _, Write as _};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use cap_std::ambient_authority;
use cap_std::fs::{Dir, Metadata, OpenOptions};
use globset::Glob;
use regex::bytes::Regex;
use sha2::{Digest as _, Sha256};

pub const LOGICAL_WORKSPACE: &str = "/workspace";
pub const MAX_LIVE_FILE_BYTES: usize = 16 * 1024 * 1024;
pub use environment_policy::MAX_OBJECT_BYTES as MAX_LIVE_OBJECT_BYTES;
pub const MAX_LOGICAL_PATH_BYTES: usize = 4096;
pub const MAX_SEARCH_ENTRIES: usize = 100_000;
pub const MAX_SEARCH_PATH_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_GREP_FILE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_GREP_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveFileKind {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveFileEntry {
    pub path: String,
    pub kind: LiveFileKind,
    pub bytes: u64,
    pub sha256: Option<String>,
    pub modified_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveFileContent {
    pub entry: LiveFileEntry,
    pub bytes: Vec<u8>,
}

/// A race-safe file descriptor opened beneath the workspace capability. Callers may stream it
/// without returning to an ambient pathname.
pub struct LiveFileReader {
    pub entry: LiveFileEntry,
    pub file: std::fs::File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveFilePage {
    pub entries: Vec<LiveFileEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Debug)]
pub struct LiveFiles {
    root: Dir,
    staging: Dir,
}

impl LiveFiles {
    pub fn new(root: impl AsRef<Path>, staging: impl AsRef<Path>) -> Result<Self, LiveFileError> {
        std::fs::create_dir_all(root.as_ref()).map_err(io_error)?;
        std::fs::create_dir_all(staging.as_ref()).map_err(io_error)?;
        let root_metadata = std::fs::symlink_metadata(root.as_ref()).map_err(io_error)?;
        let staging_metadata = std::fs::symlink_metadata(staging.as_ref()).map_err(io_error)?;
        if !root_metadata.is_dir()
            || root_metadata.file_type().is_symlink()
            || !staging_metadata.is_dir()
            || staging_metadata.file_type().is_symlink()
        {
            return Err(LiveFileError::UnsafeStaging);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            use std::os::unix::fs::PermissionsExt as _;
            // Staging remains supervisor-only, but its setgid bit assigns the workspace's shared
            // group to every staged inode before the atomic link/rename into `/workspace`.
            if root_metadata.dev() != staging_metadata.dev()
                || root_metadata.gid() != staging_metadata.gid()
            {
                return Err(LiveFileError::UnsafeStaging);
            }
            std::fs::set_permissions(staging.as_ref(), std::fs::Permissions::from_mode(0o2700))
                .map_err(io_error)?;
        }
        let root = Dir::open_ambient_dir(root.as_ref(), ambient_authority()).map_err(io_error)?;
        let staging =
            Dir::open_ambient_dir(staging.as_ref(), ambient_authority()).map_err(io_error)?;
        Ok(Self { root, staging })
    }

    pub fn try_clone(&self) -> Result<Self, LiveFileError> {
        Ok(Self {
            root: self.root.try_clone().map_err(io_error)?,
            staging: self.staging.try_clone().map_err(io_error)?,
        })
    }

    pub fn list(
        &self,
        logical_path: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<LiveFilePage, LiveFileError> {
        let relative = self.relative(logical_path)?;
        let directory = self
            .root
            .open_dir(cap_path(&relative))
            .map_err(map_directory)?;
        validate_cursor(cursor)?;
        let limit = limit.clamp(1, crate::page::MAX_PAGE);
        let mut entries = bounded_directory_entries(&directory, MAX_SEARCH_ENTRIES)?;
        entries.sort_by_key(cap_std::fs::DirEntry::file_name);
        let mut projected = Vec::with_capacity(limit + 1);
        for entry in entries {
            let mut child = relative.clone();
            child.push(entry.file_name());
            let logical = project_logical_path(&child)?;
            if cursor.is_some_and(|cursor| logical.as_str() <= cursor) {
                continue;
            }
            projected.push(self.entry_without_hash(&child, &logical)?);
            if projected.len() > limit {
                break;
            }
        }
        Ok(page(projected, limit))
    }

    pub fn stat(&self, logical_path: &str) -> Result<LiveFileEntry, LiveFileError> {
        let relative = self.relative(logical_path)?;
        self.entry_without_hash(&relative, logical_path)
    }

    pub fn read(
        &self,
        logical_path: &str,
        max_bytes: usize,
    ) -> Result<LiveFileContent, LiveFileError> {
        let max_bytes = max_bytes.min(MAX_LIVE_FILE_BYTES);
        if max_bytes == 0 {
            return Err(LiveFileError::TooLarge);
        }
        let relative = self.relative(logical_path)?;
        let file = self.root.open(&relative).map_err(map_not_found)?;
        let metadata = file.metadata().map_err(map_not_found)?;
        if !metadata.is_file() {
            return Err(LiveFileError::NotFile);
        }
        if metadata.len() > max_bytes as u64 {
            return Err(LiveFileError::TooLarge);
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(max_bytes.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() > max_bytes {
            return Err(LiveFileError::TooLarge);
        }
        let mut entry = regular_file_entry(logical_path, &metadata)?;
        entry.sha256 = Some(hex::encode(Sha256::digest(&bytes)));
        Ok(LiveFileContent { entry, bytes })
    }

    pub fn open_reader(&self, logical_path: &str) -> Result<LiveFileReader, LiveFileError> {
        let relative = self.relative(logical_path)?;
        let file = self.root.open(&relative).map_err(map_not_found)?;
        let metadata = file.metadata().map_err(map_not_found)?;
        let entry = regular_file_entry(logical_path, &metadata)?;
        Ok(LiveFileReader {
            entry,
            file: file.into_std(),
        })
    }

    pub fn open_directory(&self, logical_path: &str) -> Result<std::fs::File, LiveFileError> {
        let relative = self.relative(logical_path)?;
        self.root
            .open_dir(cap_path(&relative))
            .map(Dir::into_std_file)
            .map_err(map_directory)
    }

    pub fn write(
        &self,
        logical_path: &str,
        bytes: &[u8],
        overwrite: bool,
    ) -> Result<LiveFileEntry, LiveFileError> {
        if bytes.len() > MAX_LIVE_FILE_BYTES {
            return Err(LiveFileError::TooLarge);
        }
        let path = self.relative(logical_path)?;
        if path.as_os_str().is_empty() {
            return Err(LiveFileError::NotFile);
        }
        let parent = path.parent().ok_or(LiveFileError::OutsideScope)?;
        self.root.create_dir_all(parent).map_err(io_error)?;
        let temporary = temporary_path(&path);
        let result = (|| {
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            let mut file = self
                .staging
                .open_with(&temporary, &options)
                .map_err(io_error)?;
            file.write_all(bytes).map_err(io_error)?;
            file.sync_all().map_err(io_error)?;
            #[cfg(unix)]
            {
                use cap_std::fs::PermissionsExt as _;
                file.set_permissions(cap_std::fs::Permissions::from_mode(0o660))
                    .map_err(io_error)?;
            }
            drop(file);
            if !overwrite {
                // A hard-link install is atomic and has no replace semantics on every supported
                // platform. Both names are resolved beneath the same capability directory.
                self.staging
                    .hard_link(&temporary, &self.root, &path)
                    .map_err(map_already_exists)?;
                return self.staging.remove_file(&temporary).map_err(io_error);
            }
            self.staging
                .rename(&temporary, &self.root, &path)
                .map_err(io_error)
        })();
        if result.is_err() {
            let _ = self.staging.remove_file(&temporary);
        }
        result?;
        let mut entry = self.entry_without_hash(&path, logical_path)?;
        entry.sha256 = Some(hex::encode(Sha256::digest(bytes)));
        Ok(entry)
    }

    pub fn write_from_file(
        &self,
        logical_path: &str,
        source: &Path,
        expected_bytes: u64,
        expected_sha256: &str,
        overwrite: bool,
    ) -> Result<LiveFileEntry, LiveFileError> {
        if expected_bytes > MAX_LIVE_OBJECT_BYTES {
            return Err(LiveFileError::TooLarge);
        }
        let path = self.relative(logical_path)?;
        if path.as_os_str().is_empty() {
            return Err(LiveFileError::NotFile);
        }
        let parent = path.parent().ok_or(LiveFileError::OutsideScope)?;
        self.root.create_dir_all(parent).map_err(io_error)?;
        let temporary = temporary_path(&path);
        let result = (|| {
            let source = std::fs::File::open(source).map_err(io_error)?;
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            let mut destination = self
                .staging
                .open_with(&temporary, &options)
                .map_err(io_error)?;
            let copied = std::io::copy(
                &mut source.take(expected_bytes.saturating_add(1)),
                &mut destination,
            )
            .map_err(io_error)?;
            if copied != expected_bytes {
                return Err(LiveFileError::SourceChanged);
            }
            destination.sync_all().map_err(io_error)?;
            #[cfg(unix)]
            {
                use cap_std::fs::PermissionsExt as _;
                destination
                    .set_permissions(cap_std::fs::Permissions::from_mode(0o660))
                    .map_err(io_error)?;
            }
            drop(destination);
            if overwrite {
                self.staging
                    .rename(&temporary, &self.root, &path)
                    .map_err(io_error)
            } else {
                self.staging
                    .hard_link(&temporary, &self.root, &path)
                    .map_err(map_already_exists)?;
                self.staging.remove_file(&temporary).map_err(io_error)
            }
        })();
        if result.is_err() {
            let _ = self.staging.remove_file(&temporary);
        }
        result?;
        let mut entry = self.entry_without_hash(&path, logical_path)?;
        entry.sha256 = Some(expected_sha256.to_owned());
        Ok(entry)
    }

    pub fn find(
        &self,
        logical_path: &str,
        expression: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<LiveFilePage, LiveFileError> {
        let matcher = Glob::new(expression)
            .map_err(|error| LiveFileError::InvalidExpression(error.to_string()))?
            .compile_matcher();
        self.search(logical_path, cursor, limit, |relative, _, _| {
            Ok(matcher.is_match(relative))
        })
    }

    pub fn grep(
        &self,
        logical_path: &str,
        expression: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<LiveFilePage, LiveFileError> {
        let matcher = Regex::new(expression)
            .map_err(|error| LiveFileError::InvalidExpression(error.to_string()))?;
        let mut total = 0u64;
        self.search(logical_path, cursor, limit, |_, path, metadata| {
            if !metadata.is_file() || metadata.len() > MAX_GREP_FILE_BYTES {
                return Ok(false);
            }
            total = total.saturating_add(metadata.len());
            if total > MAX_GREP_TOTAL_BYTES {
                return Err(LiveFileError::SearchBoundExceeded);
            }
            let file = self.root.open(path).map_err(map_not_found)?;
            let mut bytes = Vec::with_capacity(metadata.len() as usize);
            file.take(metadata.len().saturating_add(1))
                .read_to_end(&mut bytes)
                .map_err(io_error)?;
            Ok(!bytes.contains(&0) && matcher.is_match(&bytes))
        })
    }

    fn search(
        &self,
        logical_path: &str,
        cursor: Option<&str>,
        limit: usize,
        mut matches: impl FnMut(&Path, &Path, &Metadata) -> Result<bool, LiveFileError>,
    ) -> Result<LiveFilePage, LiveFileError> {
        let base = self.relative(logical_path)?;
        self.root.open_dir(cap_path(&base)).map_err(map_directory)?;
        validate_cursor(cursor)?;
        let limit = limit.clamp(1, crate::page::MAX_PAGE);
        let mut projected = Vec::with_capacity(limit + 1);
        let mut pending = vec![base.clone()];
        let mut paths = Vec::new();
        let mut path_bytes = 0usize;
        while let Some(directory_path) = pending.pop() {
            if paths.len() >= MAX_SEARCH_ENTRIES {
                return Err(LiveFileError::SearchBoundExceeded);
            }
            let directory = self
                .root
                .open_dir(cap_path(&directory_path))
                .map_err(map_directory)?;
            let mut entries =
                bounded_directory_entries(&directory, MAX_SEARCH_ENTRIES - paths.len())?;
            entries.sort_by_key(cap_std::fs::DirEntry::file_name);
            for item in entries {
                let mut child = directory_path.clone();
                child.push(item.file_name());
                path_bytes = path_bytes
                    .checked_add(child.as_os_str().as_encoded_bytes().len())
                    .ok_or(LiveFileError::SearchBoundExceeded)?;
                if path_bytes > MAX_SEARCH_PATH_BYTES {
                    return Err(LiveFileError::SearchBoundExceeded);
                }
                let metadata = self.root.symlink_metadata(&child).map_err(map_not_found)?;
                if metadata.is_dir() && !metadata.file_type().is_symlink() {
                    pending.push(child.clone());
                }
                paths.push(child);
            }
        }
        paths.sort();
        for child in paths {
            let logical = project_logical_path(&child)?;
            if cursor.is_some_and(|cursor| logical.as_str() <= cursor) {
                continue;
            }
            let metadata = self.root.symlink_metadata(&child).map_err(map_not_found)?;
            let relative = child.strip_prefix(&base).unwrap_or(&child);
            if matches(relative, &child, &metadata)? {
                projected.push(self.entry_without_hash(&child, &logical)?);
                if projected.len() > limit {
                    return Ok(page(projected, limit));
                }
            }
        }
        Ok(page(projected, limit))
    }

    fn entry_without_hash(
        &self,
        path: &Path,
        logical_path: &str,
    ) -> Result<LiveFileEntry, LiveFileError> {
        let metadata = self
            .root
            .symlink_metadata(cap_path(path))
            .map_err(map_not_found)?;
        let kind = if metadata.file_type().is_symlink() {
            LiveFileKind::Symlink
        } else if metadata.is_dir() {
            LiveFileKind::Directory
        } else if metadata.is_file() {
            LiveFileKind::File
        } else {
            return Err(LiveFileError::UnsupportedFileType);
        };
        let bytes = match kind {
            LiveFileKind::File => metadata.len(),
            LiveFileKind::Symlink => self
                .root
                .read_link_contents(path)
                .map_err(io_error)?
                .as_os_str()
                .as_encoded_bytes()
                .len() as u64,
            LiveFileKind::Directory => 0,
        };
        let modified_at_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.into_std().duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_millis() as u64);
        Ok(LiveFileEntry {
            path: logical_path.to_owned(),
            kind,
            bytes,
            sha256: None,
            modified_at_ms,
        })
    }

    fn relative(&self, logical_path: &str) -> Result<PathBuf, LiveFileError> {
        if logical_path.is_empty() || logical_path.len() > 4096 || logical_path.contains('\0') {
            return Err(LiveFileError::InvalidPath);
        }
        let relative = if logical_path == LOGICAL_WORKSPACE {
            ""
        } else if let Some(relative) = logical_path.strip_prefix("/workspace/") {
            relative
        } else {
            return Err(LiveFileError::OutsideScope);
        };
        let mut candidate = PathBuf::new();
        for segment in relative.split('/').filter(|segment| !segment.is_empty()) {
            // Logical paths always use `/`, even when conformance tests run on Windows. Requiring
            // one native Normal component also rejects drive prefixes and `\\` traversal there.
            let mut components = Path::new(segment).components();
            if segment.contains('\\')
                || !matches!(components.next(), Some(Component::Normal(_)))
                || components.next().is_some()
                || matches!(segment, "." | "..")
            {
                return Err(LiveFileError::OutsideScope);
            }
            candidate.push(segment);
        }
        if !relative.is_empty() && relative.split('/').any(str::is_empty) {
            return Err(LiveFileError::InvalidPath);
        }
        Ok(candidate)
    }
}

fn bounded_directory_entries(
    directory: &Dir,
    max: usize,
) -> Result<Vec<cap_std::fs::DirEntry>, LiveFileError> {
    let mut entries = Vec::with_capacity(max.min(1_024));
    for entry in directory.entries().map_err(io_error)? {
        if entries.len() >= max {
            return Err(LiveFileError::SearchBoundExceeded);
        }
        entries.push(entry.map_err(io_error)?);
    }
    Ok(entries)
}

fn project_logical_path(relative: &Path) -> Result<String, LiveFileError> {
    if relative.is_absolute() {
        return Err(LiveFileError::OutsideScope);
    }
    let relative = relative.to_string_lossy().replace('\\', "/");
    Ok(if relative.is_empty() {
        LOGICAL_WORKSPACE.into()
    } else {
        format!("{LOGICAL_WORKSPACE}/{relative}")
    })
}

fn cap_path(relative: &Path) -> &Path {
    if relative.as_os_str().is_empty() {
        Path::new(".")
    } else {
        relative
    }
}

impl crate::page::PageIdentity for LiveFileEntry {
    fn page_identity(&self) -> &str {
        &self.path
    }
}

fn page(entries: Vec<LiveFileEntry>, limit: usize) -> LiveFilePage {
    let paged = crate::page::page(entries, limit);
    LiveFilePage {
        entries: paged.items,
        next_cursor: paged.next_cursor,
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    PathBuf::from(format!(
        ".{name}.environment-write-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}

fn regular_file_entry(
    logical_path: &str,
    metadata: &Metadata,
) -> Result<LiveFileEntry, LiveFileError> {
    if !metadata.is_file() {
        return Err(LiveFileError::NotFile);
    }
    Ok(LiveFileEntry {
        path: logical_path.to_owned(),
        kind: LiveFileKind::File,
        bytes: metadata.len(),
        sha256: None,
        modified_at_ms: metadata
            .modified()
            .ok()
            .and_then(|time| time.into_std().duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_millis() as u64),
    })
}

fn validate_cursor(cursor: Option<&str>) -> Result<(), LiveFileError> {
    if cursor.is_some_and(|cursor| {
        cursor.len() > MAX_LOGICAL_PATH_BYTES
            || !(cursor == LOGICAL_WORKSPACE || cursor.starts_with("/workspace/"))
    }) {
        Err(LiveFileError::InvalidCursor)
    } else {
        Ok(())
    }
}

fn map_not_found(error: std::io::Error) -> LiveFileError {
    if error.kind() == std::io::ErrorKind::NotFound {
        LiveFileError::NotFound
    } else {
        io_error(error)
    }
}

fn map_directory(error: std::io::Error) -> LiveFileError {
    match error.kind() {
        std::io::ErrorKind::NotFound => LiveFileError::NotFound,
        std::io::ErrorKind::NotADirectory => LiveFileError::NotDirectory,
        _ => io_error(error),
    }
}

fn map_already_exists(error: std::io::Error) -> LiveFileError {
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        LiveFileError::AlreadyExists
    } else {
        io_error(error)
    }
}

fn io_error(error: std::io::Error) -> LiveFileError {
    LiveFileError::Io(error.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LiveFileError {
    #[error("path is invalid")]
    InvalidPath,
    #[error("path resolves outside /workspace")]
    OutsideScope,
    #[error("path does not exist")]
    NotFound,
    #[error("path is not a regular file")]
    NotFile,
    #[error("path is not a directory")]
    NotDirectory,
    #[error("file already exists and overwrite is false")]
    AlreadyExists,
    #[error("file exceeds the live-file inline bound")]
    TooLarge,
    #[error("file type is unsupported")]
    UnsupportedFileType,
    #[error("staged object changed while it was copied")]
    SourceChanged,
    #[error("search expression is invalid: {0}")]
    InvalidExpression(String),
    #[error("search exceeded its bounded scan budget")]
    SearchBoundExceeded,
    #[error("pagination cursor is invalid")]
    InvalidCursor,
    #[error("live-file I/O failed: {0}")]
    Io(String),
    #[error("live-file staging directory is not a private real directory")]
    UnsafeStaging,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_files_are_bounded_sorted_and_never_leave_workspace() {
        let directory = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        let files = LiveFiles::new(directory.path(), staging.path()).unwrap();
        files.write("/workspace/b.txt", b"TODO b", false).unwrap();
        files.write("/workspace/a.txt", b"hello", false).unwrap();
        files
            .write("/workspace/nested/c.rs", b"// TODO c", false)
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
            let root = std::fs::metadata(directory.path()).unwrap();
            let staging_metadata = std::fs::metadata(staging.path()).unwrap();
            let published = std::fs::metadata(directory.path().join("a.txt")).unwrap();
            assert_eq!(staging_metadata.permissions().mode() & 0o7777, 0o2700);
            assert_eq!(published.permissions().mode() & 0o777, 0o660);
            assert_eq!(published.gid(), root.gid());
        }
        assert_eq!(std::fs::read_dir(staging.path()).unwrap().count(), 0);
        assert!(std::fs::read_dir(directory.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("environment-write")
        }));
        let first = files.list("/workspace", None, 1).unwrap();
        assert_eq!(first.entries[0].path, "/workspace/a.txt");
        let second = files
            .list("/workspace", first.next_cursor.as_deref(), 10)
            .unwrap();
        assert_eq!(
            second
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["/workspace/b.txt", "/workspace/nested"]
        );
        assert_eq!(
            files
                .find("/workspace", "**/*.rs", None, 10)
                .unwrap()
                .entries[0]
                .path,
            "/workspace/nested/c.rs"
        );
        assert_eq!(
            files
                .grep("/workspace", "TODO", None, 10)
                .unwrap()
                .entries
                .len(),
            2
        );
        assert_eq!(files.read("/workspace/a.txt", 32).unwrap().bytes, b"hello");
        assert_eq!(
            files.read("/workspace/a.txt", 4),
            Err(LiveFileError::TooLarge)
        );
        assert_eq!(files.stat("/etc/passwd"), Err(LiveFileError::OutsideScope));
        assert_eq!(
            files.write("/workspace/../escape", b"bad", false),
            Err(LiveFileError::OutsideScope)
        );
    }

    #[test]
    fn directory_collection_fails_before_growing_beyond_its_memory_bound() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["a", "b", "c"] {
            std::fs::write(directory.path().join(name), b"x").unwrap();
        }
        let directory = Dir::open_ambient_dir(directory.path(), ambient_authority()).unwrap();
        assert_eq!(
            bounded_directory_entries(&directory, 2).unwrap_err(),
            LiveFileError::SearchBoundExceeded
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected_but_the_link_itself_can_be_listed() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), b"nope").unwrap();
        symlink(outside.path(), workspace.path().join("outside")).unwrap();
        let files = LiveFiles::new(workspace.path(), staging.path()).unwrap();
        assert_eq!(
            files.stat("/workspace/outside").unwrap().kind,
            LiveFileKind::Symlink
        );
        let read = files.read("/workspace/outside/secret", 32);
        assert!(matches!(
            read,
            Err(LiveFileError::OutsideScope | LiveFileError::Io(_))
        ));

        let write = files.write("/workspace/outside/new", b"bad", false);
        assert!(matches!(
            write,
            Err(LiveFileError::OutsideScope | LiveFileError::Io(_))
        ));
        assert_eq!(
            std::fs::read(outside.path().join("secret")).unwrap(),
            b"nope"
        );
        assert!(!outside.path().join("new").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn concurrent_ancestor_symlink_swap_cannot_escape_the_capability_directory() {
        use std::os::unix::fs::symlink;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let workspace = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("pivot")).unwrap();
        std::fs::write(outside.path().join("secret"), b"outside").unwrap();
        let files = LiveFiles::new(workspace.path(), staging.path()).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let attacker_stop = stop.clone();
        let pivot = workspace.path().join("pivot");
        let held = workspace.path().join("pivot-held");
        let outside_path = outside.path().to_path_buf();
        let attacker = std::thread::spawn(move || {
            while !attacker_stop.load(Ordering::Relaxed) {
                if std::fs::rename(&pivot, &held).is_ok() {
                    if symlink(&outside_path, &pivot).is_ok() {
                        std::thread::yield_now();
                        let _ = std::fs::remove_file(&pivot);
                    }
                    let _ = std::fs::rename(&held, &pivot);
                }
            }
        });

        for _ in 0..2_000 {
            if let Ok(content) = files.read("/workspace/pivot/secret", 32) {
                assert_ne!(
                    content.bytes, b"outside",
                    "read escaped the capability root"
                );
            }
            let _ = files.write("/workspace/pivot/pwn", b"bad", true);
            assert!(
                !outside.path().join("pwn").exists(),
                "write escaped the capability root"
            );
        }
        stop.store(true, Ordering::Relaxed);
        attacker.join().unwrap();
        assert!(!outside.path().join("pwn").exists());
    }
}
