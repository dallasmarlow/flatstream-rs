use std::fs;
use std::path::PathBuf;

pub fn list_with_suffix(dir: &str, suffix: &str) -> Option<Vec<PathBuf>> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    Some(
        entries
            .into_iter()
            .filter(|p| p.is_file() && p.to_string_lossy().ends_with(suffix))
            .collect(),
    )
}
