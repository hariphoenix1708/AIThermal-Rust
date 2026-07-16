use anyhow::Result;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::error;

// Lazy cache for discovered paths
lazy_static::lazy_static! {
    static ref PATH_CACHE: RwLock<HashMap<String, PathBuf>> = RwLock::new(HashMap::new());
}

#[derive(thiserror::Error, Debug)]
pub enum SysfsError {
    #[error("Node not found or not a file: {0}")]
    NotFound(String),
    #[error("Permission denied reading/writing node: {0}")]
    PermissionDenied(String),
    #[error("I/O error at node {0}: {1}")]
    Io(String, std::io::Error),
    #[error("Invalid value or parse error for node {0}: {1}")]
    Parse(String, String),
    #[error("No valid path available from candidates")]
    NoValidPath,
    #[error("Write verification failed for {0}. Expected {1}, got {2}")]
    VerificationFailed(String, String, String),
    #[error("Unsupported sysfs node format: {0}")]
    UnsupportedNode(String),
}

/// Safely read a sysfs node into a trimmed String.
pub fn read_string<P: AsRef<Path>>(path: P) -> Result<String, SysfsError> {
    let p = path.as_ref();
    if !exists(p) {
        return Err(SysfsError::NotFound(p.to_string_lossy().into_owned()));
    }

    let content = fs::read_to_string(p).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => {
            SysfsError::PermissionDenied(p.to_string_lossy().into_owned())
        }
        _ => SysfsError::Io(p.to_string_lossy().into_owned(), e),
    })?;

    Ok(content.trim().to_string())
}

/// Safely read a sysfs node into an i64.
pub fn read_i64<P: AsRef<Path>>(path: P) -> Result<i64, SysfsError> {
    let content = read_string(path.as_ref())?;
    content
        .parse::<i64>()
        .map_err(|e| SysfsError::Parse(path.as_ref().to_string_lossy().into_owned(), e.to_string()))
}

/// Write a string to a sysfs node safely, with verification.
pub fn write_string<P: AsRef<Path>, S: AsRef<str>>(path: P, value: S) -> Result<(), SysfsError> {
    let p = path.as_ref();
    let val_str = value.as_ref();

    if !exists(p) {
        return Err(SysfsError::NotFound(p.to_string_lossy().into_owned()));
    }

    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(p)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::PermissionDenied => {
                SysfsError::PermissionDenied(p.to_string_lossy().into_owned())
            }
            _ => SysfsError::Io(p.to_string_lossy().into_owned(), e),
        })?;

    file.write_all(val_str.as_bytes())
        .map_err(|e| SysfsError::Io(p.to_string_lossy().into_owned(), e))?;
    file.flush()
        .map_err(|e| SysfsError::Io(p.to_string_lossy().into_owned(), e))?;

    Ok(())
}

pub fn write_string_verified<P: AsRef<Path>, S: AsRef<str>>(
    path: P,
    value: S,
) -> Result<(), SysfsError> {
    let p = path.as_ref();
    let val_str = value.as_ref();

    write_string(p, val_str)?;

    let verify = read_string(p)?;
    if verify != val_str {
        return Err(SysfsError::VerificationFailed(
            p.to_string_lossy().into_owned(),
            val_str.to_string(),
            verify,
        ));
    }

    Ok(())
}

/// Write an i64 to a sysfs node safely.
pub fn write_i64<P: AsRef<Path>>(path: P, value: i64) -> Result<(), SysfsError> {
    write_string(path, value.to_string())
}

/// Check if a sysfs node exists and is a file.
pub fn exists<P: AsRef<Path>>(path: P) -> bool {
    let p = path.as_ref();
    p.exists() && p.is_file()
}

/// Find the first existing path from candidates and cache it for future use.
pub fn discover_paths<'a, I, P>(cache_key: &str, paths: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path> + 'a,
{
    // Check cache first
    if let Some(cached) = PATH_CACHE.read().get(cache_key)
        && exists(cached)
    {
        return Some(cached.clone());
    }

    // Discover
    let found = paths
        .into_iter()
        .map(|p| p.as_ref().to_path_buf())
        .find(|p| exists(p));

    // Cache result
    if let Some(ref path) = found {
        PATH_CACHE
            .write()
            .insert(cache_key.to_string(), path.clone());
    } else {
        error!("Could not discover any valid path for: {}", cache_key);
    }

    found
}

/// Find the first existing path from a given list of candidates and write to it.
pub fn write_first_available<'a, I, P, S>(paths: I, value: S) -> Result<(), SysfsError>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path> + 'a,
    S: AsRef<str>,
{
    let found = paths
        .into_iter()
        .map(|p| p.as_ref().to_path_buf())
        .find(|p| exists(p) && std::fs::OpenOptions::new().write(true).open(p).is_ok());

    if let Some(path) = found {
        write_string(&path, value)?;
        Ok(())
    } else {
        Err(SysfsError::NoValidPath)
    }
}

/// Read the first available path
pub fn read_first_available<'a, I, P>(paths: I) -> Result<String, SysfsError>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path> + 'a,
{
    let found = paths
        .into_iter()
        .map(|p| p.as_ref().to_path_buf())
        .find(|p| exists(p));

    if let Some(path) = found {
        read_string(&path)
    } else {
        Err(SysfsError::NoValidPath)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_read_write_sysfs() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_node");

        fs::write(&file_path, "").unwrap();

        // write
        write_string(&file_path, "12345").unwrap();

        // exists
        assert!(exists(&file_path));

        // read_string
        assert_eq!(read_string(&file_path).unwrap(), "12345");

        // read_i64
        assert_eq!(read_i64(&file_path).unwrap(), 12345);

        // write_i64
        write_i64(&file_path, 999).unwrap();
        assert_eq!(read_i64(&file_path).unwrap(), 999);

        // write_first_available
        let fake_path1 = dir.path().join("fake_node1");
        let fake_path2 = dir.path().join("fake_node2");

        let paths = vec![&fake_path1, &fake_path2, &file_path];

        // test discover cache
        let discovered = discover_paths("test_key", paths.clone());
        assert_eq!(discovered, Some(file_path.clone()));

        write_first_available(paths.clone(), "67890").unwrap();
        assert_eq!(read_first_available(paths).unwrap(), "67890");
    }

    #[test]
    fn test_not_found() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("missing");
        let res = read_string(&file_path);
        assert!(matches!(res, Err(SysfsError::NotFound(_))));
    }

    #[test]
    fn test_verification_failure() {
        // Normally write verification passes.
        // We simulate a failure by using a file that changes or isn't writable.
        // On Unix, writing to a read-only file should return PermissionDenied.
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("readonly");
        fs::write(&file_path, "123").unwrap();
        let mut perms = fs::metadata(&file_path).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&file_path, perms).unwrap();

        let res = write_string(&file_path, "456");
        assert!(matches!(res, Err(SysfsError::PermissionDenied(_))));
    }
}
