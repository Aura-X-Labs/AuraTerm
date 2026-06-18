//! Thin wrapper over the OS keychain for the opt-in "remember master password"
//! feature.
//!
//! Only a *single* secret is ever stored (the master password), unlike the old
//! per-credential keyring usage that caused repeated authorization prompts. The
//! real implementation is compiled only on macOS / Windows (native backends); on
//! every other platform the functions are no-ops and [`is_available`] returns
//! `false`, so the frontend hides the "remember" option instead of pulling in a
//! secret-service / DBus dependency on Linux.

#[cfg(any(target_os = "macos", target_os = "windows"))]
const SERVICE: &str = "AuraTerm";
#[cfg(any(target_os = "macos", target_os = "windows"))]
const ACCOUNT: &str = "master-password";

/// Whether OS keychain storage is supported on this platform.
pub fn is_available() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| format!("Keychain entry error: {e}"))
}

/// Store (or overwrite) the master password in the OS keychain.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn store_password(password: &str) -> Result<(), String> {
    entry()?
        .set_password(password)
        .map_err(|e| format!("Failed to store master password in keychain: {e}"))
}

/// Load the master password from the OS keychain. Returns `Ok(None)` when no
/// entry exists (e.g. the user never opted in).
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn load_password() -> Result<Option<String>, String> {
    match entry()?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Failed to read master password from keychain: {e}")),
    }
}

/// Remove the stored master password. Missing entry is treated as success.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn clear() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Failed to clear master password from keychain: {e}")),
    }
}

// ----- Fallback no-op implementation (Linux and any other platform) -----

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn store_password(_password: &str) -> Result<(), String> {
    Err("Keychain is not available on this platform".to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn load_password() -> Result<Option<String>, String> {
    Ok(None)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn clear() -> Result<(), String> {
    Ok(())
}
