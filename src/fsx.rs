//! The filesystem reads the folder listing needs.
//!
//! Plain `std::fs`, which follows symlinks, so `/home/u/code -> /data/code`
//! lists the target's folders.

use std::path::Path;

/// Whether `path` is a directory, following symlinks.
pub fn is_dir(path: &Path) -> bool {
    path.is_dir()
}

/// The names in the folder at `path`, in no particular order. Empty when it
/// cannot be read. An entry that cannot be read (removed while the folder was
/// being listed, say) is skipped rather than ending the listing.
pub fn list_dir(path: &Path) -> Vec<String> {
    std::fs::read_dir(path)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}
