//! Lightweight leveled logging macros.
//!
//! The transport modules (SSH especially) carry verbose diagnostic traces that
//! used to be raw `eprintln!`/`println!` calls. Those reach stderr in *release*
//! builds too — noisy, and the SSH ones even narrate authentication progress.
//!
//! These macros gate the noise by build profile instead of pulling in a full
//! logging backend:
//!
//! - [`debug_log!`] — diagnostic traces. Compiled out entirely in release
//!   builds (`#[cfg(debug_assertions)]`), so a shipped binary stays quiet and
//!   pays no formatting cost.
//! - [`warn_log!`] — genuine warnings/errors worth surfacing in any build
//!   (e.g. a corrupt credential store, failure to restore window bounds).
//!
//! Both are `#[macro_export]`, so call them crate-wide as `crate::debug_log!`
//! / `crate::warn_log!`.

/// Diagnostic trace, emitted to stderr only in debug builds.
///
/// In release the message is compiled out, but the arguments are still
/// referenced via a discarded `format_args!` (zero runtime cost) so variables
/// that exist only to be logged don't trip `unused_variables` warnings.
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {{
        #[cfg(debug_assertions)]
        {
            eprintln!($($arg)*);
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = format_args!($($arg)*);
        }
    }};
}

/// Warning/error, emitted to stderr in every build.
#[macro_export]
macro_rules! warn_log {
    ($($arg:tt)*) => {{
        eprintln!($($arg)*);
    }};
}
