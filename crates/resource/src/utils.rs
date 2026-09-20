use std::path::{Path, PathBuf};

/// Left-pad an ID with zeros to `len` digits (e.g., 42 → "00000042" for len=9).
pub fn get_padding_zero(id: u32, len: usize) -> String {
    let s = id.to_string();
    let pad = len.saturating_sub(s.len());
    let mut buf = String::with_capacity(len);
    for _ in 0..pad {
        buf.push('0');
    }
    buf.push_str(&s);
    buf
}

/// Locate the `data/` directory: tries `<root>/data`, then `<root>/../data`.
pub fn get_data_path(root_path: &Path) -> PathBuf {
    let current = root_path.join("data");
    if current.exists() {
        return current;
    }
    let parent = root_path.join("../data");
    if parent.exists() {
        return parent;
    }
    root_path.to_path_buf()
}

/// Create a directory recursively if it doesn't exist; return its PathBuf.
pub fn create_folder(root: &Path, relative: &str) -> PathBuf {
    let target = root.join(relative);
    if !target.exists() {
        std::fs::create_dir_all(&target).ok();
    }
    target
}

/// Map bucket: `id / 100000000` determines which `MapN/` subdirectory holds the file.
pub fn map_bucket(id: u32) -> u32 {
    id / 100_000_000
}
