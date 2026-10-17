/**
 * What the channel this build ships through allows; the frontend half of
 * `src-tauri/src/distribution.rs`.
 *
 * The Mac App Store build runs in the App Sandbox, where a local shell would
 * see the app's container instead of the user's home, so that build leaves
 * local shells out and keeps its files under ~/Downloads, the one user folder
 * it may write to without a dialog. `scripts/package-appstore.sh` sets
 * `VITE_AURATERM_APP_STORE=1` for the frontend build alongside the Rust
 * `app-store` feature; the backend refuses a local shell on its own as well.
 */
export const isAppStoreBuild = import.meta.env.VITE_AURATERM_APP_STORE === "1";

export const localShellAvailable = !isAppStoreBuild;

/** Where logs and downloads go by default, as a `~/` path. */
export const defaultFilesDir = isAppStoreBuild ? "~/Downloads/AuraTerm" : "~/AuraTerm";
