//! Per-user data directory shared by the daemon and the UIs (state.json, history.jsonl, ...).
use std::path::PathBuf;

pub fn data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/Buds"))
    }
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA").map(|h| PathBuf::from(h).join("Buds"))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map(|d| d.join("buds"))
    }
}
