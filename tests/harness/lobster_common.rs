use std::fs;

// Parse SHASUMS.txt into filename -> sha256 hex (lowercase)
pub fn parse_shasums(
    shasums_path: &str,
) -> Result<std::collections::HashMap<String, String>, String> {
    let text = fs::read_to_string(shasums_path)
        .map_err(|_| format!("Missing SHASUMS.txt at {}", shasums_path))?;
    let mut map = std::collections::HashMap::new();
    for (idx, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // format: <filename> <sha256>
        let mut parts = line.split_whitespace();
        let Some(fname) = parts.next() else {
            return Err(format!("Malformed SHASUMS.txt line {}: {}", idx + 1, line));
        };
        let Some(sum) = parts.next() else {
            return Err(format!("Malformed SHASUMS.txt line {}: {}", idx + 1, line));
        };
        map.insert(fname.to_string(), sum.to_lowercase());
    }
    Ok(map)
}

// Return base names (filename without extension) for ZIPs present in zips_dir,
// listed in SHASUMS.txt and matching sha256
pub fn find_verified_zip_file_bases(zips_dir: &str, shasums_path: &str) -> Vec<String> {
    use sha2::{Digest, Sha256};
    let allowed = parse_shasums(shasums_path).unwrap_or_else(|e| panic!("{}", e));
    let entries =
        fs::read_dir(zips_dir).unwrap_or_else(|_| panic!("Missing zips directory: {}", zips_dir));
    let mut file_bases = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("zip") {
            continue;
        }
        let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        match allowed.get(fname) {
            None => {
                eprintln!("Skipping present but unlisted file: {}", fname);
                continue;
            }
            Some(expected) => {
                // compute sha256
                let data = fs::read(&path).unwrap_or_else(|_| panic!("Failed to read {}", fname));
                let mut hasher = Sha256::new();
                hasher.update(&data);
                let actual = format!("{:x}", hasher.finalize());
                if actual != *expected {
                    eprintln!(
                        "Checksum mismatch for {}: expected {}, got {}. Skipping.",
                        fname, expected, actual
                    );
                    continue;
                }
            }
        }
        let base = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        file_bases.push(base);
    }
    file_bases.sort();
    file_bases
}
