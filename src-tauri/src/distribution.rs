//! What the channel this binary ships through allows.
//!
//! The Mac App Store build (`--features app-store`, produced by
//! `scripts/package-appstore.sh`) runs inside the App Sandbox. A shell spawned
//! there inherits the sandbox: `HOME` is the app's container, so the user's
//! dotfiles, `~/.ssh` and anything Homebrew installed are out of reach. Rather
//! than ship a shell that cannot do what a terminal user expects, that build
//! leaves local shells out; the direct downloads keep them.
//!
//! The frontend reads the same choice at build time from
//! `VITE_AURATERM_APP_STORE` (see `src/distribution.ts`); the packaging script
//! sets both, and the checks here hold even if the two ever disagree.

use std::path::{Path, PathBuf};

pub const APP_STORE: bool = cfg!(feature = "app-store");

/// Whether this build may open a local shell: `start_pty`, the native
/// "New Session ▸ Local Shell" item and Live Share's open-shell target.
pub const LOCAL_SHELL: bool = !APP_STORE;

pub const LOCAL_SHELL_UNAVAILABLE: &str =
    "Local shells are not available in the Mac App Store edition of AuraTerm.";

/// Where AuraTerm puts files it writes for the user (logs, downloads), as a
/// `~/` path. The sandbox only lets the App Store build write to Downloads,
/// besides what the user picks in a dialog, so its default lives there; inside
/// the container `~/Downloads` is the system's link to the real folder.
pub const FILES_DIR_TILDE: &str = if APP_STORE { "~/Downloads/AuraTerm" } else { "~/AuraTerm" };

pub fn files_dir(home: &Path) -> PathBuf {
    home.join(FILES_DIR_TILDE.trim_start_matches("~/"))
}

pub fn default_log_dir_tilde() -> String {
    format!("{FILES_DIR_TILDE}/logs")
}

pub fn default_download_dir_tilde() -> String {
    format!("{FILES_DIR_TILDE}/downloads")
}

/// The home directory a file dialog should open in. Inside the sandbox `HOME`
/// is the container, so `~/.ssh` would never be found there; the account's own
/// home comes from the password database, and the system dialog may browse it.
pub fn dialog_home_dir(home: PathBuf) -> PathBuf {
    #[cfg(all(unix, feature = "app-store"))]
    {
        // SAFETY: getpwuid returns null or a pointer to static storage that
        // stays valid until the next getpw* call; it is copied out at once.
        unsafe {
            let passwd = libc::getpwuid(libc::getuid());
            if !passwd.is_null() && !(*passwd).pw_dir.is_null() {
                let dir = std::ffi::CStr::from_ptr((*passwd).pw_dir);
                if let Ok(dir) = dir.to_str() {
                    if !dir.is_empty() {
                        return PathBuf::from(dir);
                    }
                }
            }
        }
    }
    home
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_paths_sit_under_the_files_dir() {
        assert_eq!(default_log_dir_tilde(), format!("{FILES_DIR_TILDE}/logs"));
        assert_eq!(default_download_dir_tilde(), format!("{FILES_DIR_TILDE}/downloads"));
        let home = Path::new("/Users/someone");
        let expected = if APP_STORE { "/Users/someone/Downloads/AuraTerm" } else { "/Users/someone/AuraTerm" };
        assert_eq!(files_dir(home), PathBuf::from(expected));
    }

    #[test]
    fn local_shell_follows_the_channel() {
        assert_eq!(LOCAL_SHELL, !APP_STORE);
    }
}
