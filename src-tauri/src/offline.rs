//! Closed-network asset seeding used by the Windows installer.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SeedReport {
    pub copied: usize,
    pub skipped: usize,
}

/// Parse the optional maintenance CLI argument without starting Tauri.
pub fn seed_resources_argument(args: &[String]) -> Result<Option<PathBuf>, String> {
    let mut source = None;
    let mut i = 1;
    while i < args.len() {
        if args[i] != "--seed-resources" {
            i += 1;
            continue;
        }
        if source.is_some() {
            return Err("--seed-resources may be specified only once".into());
        }
        let value = args
            .get(i + 1)
            .filter(|value| !value.is_empty() && !value.starts_with('-'))
            .ok_or_else(|| "--seed-resources requires a source directory".to_string())?;
        source = Some(PathBuf::from(value));
        i += 2;
    }
    Ok(source)
}

/// Copy `source_dir/ai/**` into `data_root/ai/**`, never replacing a file.
/// The source layout is deliberately checked here so an installer typo cannot
/// seed arbitrary files beside the application's data directory.
pub fn seed_resources(source_dir: &Path, data_root: &Path) -> io::Result<SeedReport> {
    let source_ai_path = source_dir.join("ai");
    let source_meta = fs::symlink_metadata(&source_ai_path)?;
    if !source_meta.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "offline asset source must contain an ai directory: {}",
                source_ai_path.display()
            ),
        ));
    }
    let source_ai = fs::canonicalize(&source_ai_path)?;

    let destination_ai_path = data_root.join("ai");
    let destination_ai = canonicalize_for_check(&destination_ai_path)?;
    if source_ai == destination_ai
        || source_ai.starts_with(&destination_ai)
        || destination_ai.starts_with(&source_ai)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "offline asset source and destination overlap: {} -> {}",
                source_ai.display(),
                destination_ai.display()
            ),
        ));
    }

    fs::create_dir_all(&destination_ai_path)?;
    let mut report = SeedReport::default();
    copy_tree(&source_ai_path, &destination_ai_path, &mut report)?;
    Ok(report)
}

/// Canonicalize a path before it exists by canonicalizing its nearest existing
/// ancestor and appending the missing suffix. This keeps overlap checks safe
/// without creating the destination before the check.
fn canonicalize_for_check(path: &Path) -> io::Result<PathBuf> {
    let mut missing = Vec::new();
    let mut probe = path;
    loop {
        match fs::canonicalize(probe) {
            Ok(mut canonical) => {
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let Some(name) = probe.file_name() else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("cannot canonicalize destination: {}", path.display()),
                    ));
                };
                missing.push(PathBuf::from(name));
                let parent = probe.parent().unwrap_or_else(|| Path::new("."));
                probe = if parent.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    parent
                };
            }
            Err(error) => return Err(error),
        }
    }
}

fn copy_tree(source: &Path, destination: &Path, report: &mut SeedReport) -> io::Result<()> {
    let mut entries = fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let source_path = entry.path();
        let file_type = entry.file_type()?;
        let destination_path = destination.join(entry.file_name());
        if file_type.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "symlink is not a supported offline asset: {}",
                    source_path.display()
                ),
            ));
        }
        if file_type.is_dir() {
            match fs::symlink_metadata(&destination_path) {
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!(
                            "destination is a file, expected a directory: {}",
                            destination_path.display()
                        ),
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(&destination_path)?;
                }
                Err(error) => return Err(error),
            }
            copy_tree(&source_path, &destination_path, report)?;
        } else if file_type.is_file() {
            copy_file_if_missing(&source_path, &destination_path, report)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported offline asset: {}", source_path.display()),
            ));
        }
    }
    Ok(())
}

fn copy_file_if_missing(
    source: &Path,
    destination: &Path,
    report: &mut SeedReport,
) -> io::Result<()> {
    match fs::symlink_metadata(destination) {
        Ok(meta) if meta.is_file() => {
            report.skipped += 1;
            return Ok(());
        }
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("destination is not a file: {}", destination.display()),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let Some(parent) = destination.parent() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("destination has no parent: {}", destination.display()),
        ));
    };
    fs::create_dir_all(parent)?;

    let mut input = fs::File::open(source)?;
    let mut output = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            report.skipped += 1;
            return Ok(());
        }
        Err(error) => return Err(error),
    };

    if let Err(error) = copy_and_flush(&mut input, &mut output) {
        drop(output);
        let _ = fs::remove_file(destination);
        return Err(error);
    }
    report.copied += 1;
    Ok(())
}

fn copy_and_flush(input: &mut fs::File, output: &mut fs::File) -> io::Result<()> {
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
    }
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("hindsight-offline-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn parses_seed_argument_and_rejects_missing_value() {
        let args = vec![
            "hindsight.exe".into(),
            "--seed-resources".into(),
            "assets".into(),
        ];
        assert_eq!(
            seed_resources_argument(&args).unwrap(),
            Some(PathBuf::from("assets"))
        );
        assert!(seed_resources_argument(&args[..2]).is_err());
    }

    #[test]
    fn missing_source_is_a_seeding_error_for_nonzero_cli_exit() {
        let root = test_root("missing");
        let error = seed_resources(&root.join("offline-assets"), &root.join("data")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn nested_seed_never_overwrites_existing_files() {
        let root = test_root("copy");
        let source = root.join("offline-assets");
        let data = root.join("data");
        fs::create_dir_all(source.join("ai/models/nested")).unwrap();
        fs::write(source.join("ai/models/nested/new.gguf"), b"new").unwrap();
        fs::write(
            source.join("ai/models/nested/existing.gguf"),
            b"replacement",
        )
        .unwrap();
        fs::create_dir_all(data.join("ai/models/nested")).unwrap();
        fs::write(data.join("ai/models/nested/existing.gguf"), b"user-data").unwrap();

        let report = seed_resources(&source, &data).unwrap();
        assert_eq!(
            report,
            SeedReport {
                copied: 1,
                skipped: 1
            }
        );
        assert_eq!(
            fs::read(data.join("ai/models/nested/new.gguf")).unwrap(),
            b"new"
        );
        assert_eq!(
            fs::read(data.join("ai/models/nested/existing.gguf")).unwrap(),
            b"user-data"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_equal_and_ancestor_destination_before_copying() {
        let root = test_root("overlap");
        let source = root.join("offline-assets");
        fs::create_dir_all(source.join("ai/nested")).unwrap();
        fs::write(source.join("ai/nested/model.gguf"), b"model").unwrap();

        let equal = seed_resources(&source, &source).unwrap_err();
        assert_eq!(equal.kind(), io::ErrorKind::InvalidInput);

        // data_root/ai is a descendant of source_dir/ai; the destination does
        // not exist yet, so this also covers canonicalizing a missing suffix.
        let descendant = seed_resources(&source, &source.join("ai")).unwrap_err();
        assert_eq!(descendant.kind(), io::ErrorKind::InvalidInput);
        assert!(!source.join("ai/ai").exists());

        let _ = fs::remove_dir_all(root);
    }
}
