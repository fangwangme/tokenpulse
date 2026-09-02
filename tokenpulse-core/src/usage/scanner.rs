use chrono::NaiveDate;
use rayon::prelude::*;
use std::path::PathBuf;
use walkdir::WalkDir;

pub fn discover_files(root: &PathBuf, extension: &str, since: Option<NaiveDate>) -> Vec<PathBuf> {
    discover_files_with_extensions(root, &[extension], since)
}

pub fn discover_files_with_extensions(
    root: &PathBuf,
    extensions: &[&str],
    since: Option<NaiveDate>,
) -> Vec<PathBuf> {
    let entries: Vec<walkdir::DirEntry> = WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| extensions.contains(&ext))
                .unwrap_or(false)
        })
        .collect();

    if let Some(since_date) = since {
        entries
            .into_par_iter()
            .filter(|e| {
                e.metadata()
                    .ok()
                    .and_then(|metadata| file_arrival_date(&metadata))
                    .is_some_and(|arrival| arrival >= since_date)
            })
            .map(|e| e.path().to_path_buf())
            .collect()
    } else {
        entries
            .into_iter()
            .map(|e| e.path().to_path_buf())
            .collect()
    }
}

/// The most recent date at which this file could have gained content we have not
/// read yet.
///
/// Modification time alone is not enough. Restoring a backup, migrating to
/// another machine, or any `rsync -a` / `cp -p` writes the file with its
/// *original* mtime, so a transcript can land on disk already older than the
/// incremental window and never be seen. Inode change time cannot be back-dated
/// that way — on the destination it is the moment the file was written there —
/// so the later of the two is what "recently arrived" actually means.
fn file_arrival_date(metadata: &std::fs::Metadata) -> Option<NaiveDate> {
    let modified = metadata
        .modified()
        .ok()
        .map(|time| chrono::DateTime::<chrono::Local>::from(time).date_naive());

    #[cfg(unix)]
    let changed = {
        use std::os::unix::fs::MetadataExt;
        chrono::DateTime::from_timestamp(metadata.ctime(), 0)
            .map(|time| time.with_timezone(&chrono::Local).date_naive())
    };
    // Windows has no ctime; creation time is the closest equivalent and is also
    // set to the copy time when a file is restored.
    #[cfg(not(unix))]
    let changed = metadata
        .created()
        .ok()
        .map(|time| chrono::DateTime::<chrono::Local>::from(time).date_naive());

    match (modified, changed) {
        (Some(modified), Some(changed)) => Some(modified.max(changed)),
        (modified, changed) => modified.or(changed),
    }
}

pub fn parse_files_parallel<F, T>(files: Vec<PathBuf>, parser: F) -> Vec<T>
where
    F: Fn(PathBuf) -> Vec<T> + Sync + Send,
    T: Send + Sync,
{
    files.into_par_iter().flat_map(parser).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_temp_dir() -> (tempfile::TempDir, PathBuf) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        (temp_dir, root)
    }

    #[test]
    fn test_discover_files_sees_a_restored_backup_with_an_old_mtime() {
        // Restoring a backup (Time Machine, `rsync -a`, a machine migration)
        // rewrites the file with its ORIGINAL modification time, so it can land
        // on disk already older than the incremental window. Inode change time
        // is the moment it was written here and cannot be back-dated, so the
        // file must still be discovered.
        let (_temp_dir, root) = create_temp_dir();
        let restored = root.join("restored-session.jsonl");
        fs::write(&restored, "{}").unwrap();

        let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 86_400);
        fs::File::options()
            .write(true)
            .open(&restored)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();

        let modified = chrono::DateTime::<chrono::Local>::from(
            fs::metadata(&restored).unwrap().modified().unwrap(),
        )
        .date_naive();
        let since = chrono::Local::now().date_naive() - chrono::Duration::days(7);
        assert!(modified < since, "mtime must be outside the window");

        let files = discover_files(&root, "jsonl", Some(since));
        assert_eq!(files.len(), 1, "restored file must still be discovered");
    }

    #[test]
    fn test_discover_files_empty_dir() {
        let (_temp_dir, root) = create_temp_dir();
        let files = discover_files(&root, "jsonl", None);
        assert!(files.is_empty());
    }

    #[test]
    fn test_discover_files_with_files() {
        let (_temp_dir, root) = create_temp_dir();

        fs::write(root.join("test.jsonl"), "{}").unwrap();
        fs::write(root.join("test2.jsonl"), "{}").unwrap();
        fs::write(root.join("test.txt"), "text").unwrap();

        let files = discover_files(&root, "jsonl", None);
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_discover_files_nested() {
        let (_temp_dir, root) = create_temp_dir();

        fs::create_dir_all(root.join("subdir")).unwrap();
        fs::write(root.join("test.jsonl"), "{}").unwrap();
        fs::write(root.join("subdir/test2.jsonl"), "{}").unwrap();

        let files = discover_files(&root, "jsonl", None);
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_parse_files_parallel() {
        let (_temp_dir, root) = create_temp_dir();

        fs::write(root.join("1.jsonl"), "{}").unwrap();
        fs::write(root.join("2.jsonl"), "{}").unwrap();
        fs::write(root.join("3.jsonl"), "{}").unwrap();

        let files = discover_files(&root, "jsonl", None);
        let results: Vec<i32> = parse_files_parallel(files, |_| vec![1, 2, 3]);

        // 3 files * 3 items = 9 items
        // sum: 1+2+3 per file * 3 files = 18
        assert_eq!(results.len(), 9);
        assert_eq!(results.iter().sum::<i32>(), 18);
    }
}
