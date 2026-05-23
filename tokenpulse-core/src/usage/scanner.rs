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
                if let Ok(metadata) = e.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        let modified_date =
                            chrono::DateTime::<chrono::Local>::from(modified).date_naive();
                        return modified_date >= since_date;
                    }
                }
                false
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
