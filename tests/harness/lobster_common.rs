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

pub fn find_lobster_pairs(root: &str) -> Vec<(PathBuf, PathBuf, PathBuf)> {
    let msgs = list_with_suffix(root, "-message.bin").unwrap_or_default();
    let obs = list_with_suffix(root, "-orderbook.bin").unwrap_or_default();

    use std::collections::HashMap;
    let mut map: HashMap<String, (Option<PathBuf>, Option<PathBuf>)> = HashMap::new();

    for p in msgs {
        let fname = p.file_name().unwrap().to_string_lossy().to_string();
        let stem = fname
            .strip_suffix("-message.bin")
            .unwrap_or(&fname)
            .to_string();
        map.entry(stem).or_default().0 = Some(p);
    }
    for p in obs {
        let fname = p.file_name().unwrap().to_string_lossy().to_string();
        let stem = fname
            .strip_suffix("-orderbook.bin")
            .unwrap_or(&fname)
            .to_string();
        map.entry(stem).or_default().1 = Some(p);
    }

    let mut out = Vec::new();
    for (stem, (m, o)) in map.into_iter() {
        if let (Some(mp), Some(op)) = (m, o) {
            let sidecar = PathBuf::from(format!("{}/{}-counts.txt", root, stem));
            out.push((mp, op, sidecar));
        }
    }
    out
}


