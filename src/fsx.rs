//! The filesystem reads the folder listing needs, the same inside the sandbox
//! and out.
//!
//! Inside it, a symlink with an absolute target is never followed (see
//! `sicompass_sdk::fs_links`), so `/home/u/code -> /data/code` would look like
//! a dead end. Every path is resolved through the host's `desktop.read-link`
//! first, which reaches the target directly with `/` granted. A repository's
//! own paths need none of this: git reports them already resolved.

use std::path::{Path, PathBuf};

/// `path` with every symlink along it replaced by its target.
pub fn resolve(path: &Path) -> PathBuf {
    #[cfg(target_arch = "wasm32")]
    {
        sicompass_sdk::fs_links::resolve_with(path, |p| {
            sicompass_pdk::desktop::read_link(&p.to_string_lossy())
                .ok()
                .map(PathBuf::from)
        })
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        path.to_path_buf()
    }
}

/// Whether `path` is a directory, following symlinks.
pub fn is_dir(path: &Path) -> bool {
    resolve(path).is_dir()
}

/// The names in the folder at `path`, in no particular order. Empty when it
/// cannot be read.
pub fn list_dir(path: &Path) -> Vec<String> {
    let dir = resolve(path);
    #[cfg(target_arch = "wasm32")]
    {
        // Not `read_dir`: in the sandbox it stops at an entry removed while
        // the folder was being read (see `sicompass_pdk::fs::list_dir`).
        sicompass_pdk::fs::list_dir(&dir).unwrap_or_default()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }
}
