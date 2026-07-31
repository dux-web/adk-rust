//! Locating the `monty` worker binary.
//!
//! Python runs in `monty` worker subprocesses rather than in this process, so the
//! runtime needs a path to that binary before it can start. Resolution is
//! explicit and ordered, and a failure says exactly how to fix it — a missing
//! worker is the one setup problem every user of this crate will hit at least
//! once.
//!
//! There is deliberately **no fallback to in-process execution**. Falling back
//! would silently give up the crash isolation that running in a worker exists to
//! provide, and the caller would have no way to tell which mode they got.

use std::path::{Path, PathBuf};

/// The environment variable consulted when no path is configured.
pub const BINARY_ENV_VAR: &str = "ADK_MONTY_BINARY";

/// The worker binary's name on `PATH`.
pub const BINARY_NAME: &str = "monty";

/// The Monty version this crate speaks.
///
/// The pool and the worker exchange a versioned wire protocol, so the binary
/// should come from the same release as the linked `monty-pool`.
pub const MONTY_VERSION: &str = "0.0.19";

/// The install command that actually works.
///
/// `--locked` is not optional. Without it, cargo resolves `monty-runtime`'s
/// dependencies afresh and picks `get-size2 0.10.3` (→ `compact_str 0.10`), while
/// `ruff_python_ast 0.0.3` derives `GetSize` on `compact_str 0.9` fields — the
/// build fails inside a dependency with a trait-bound error that has nothing
/// obviously to do with what the user typed. Binary crates publish their
/// `Cargo.lock`, and `--locked` uses Monty's own known-good resolution.
pub const INSTALL_COMMAND: &str = "cargo install monty-runtime --version 0.0.19 --locked";

/// The worker binary could not be located.
#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    /// Nothing configured, and no `monty` on `PATH`.
    #[error(
        "the monty worker binary was not found.\n\
         \n\
         Python for the CodeAct runtime runs in a `monty` worker subprocess, so the \
         binary has to be available. Any one of these fixes it:\n\
         \n\
         1. Install it (note `--locked`, which is required):\n\
         \x20      {INSTALL_COMMAND}\n\
         2. Point at an existing binary:\n\
         \x20      export {BINARY_ENV_VAR}=/path/to/monty\n\
         3. Configure it in code:\n\
         \x20      MontyRuntime::builder().worker_binary(\"/path/to/monty\")\n\
         \n\
         Omitting `--locked` in step 1 fails inside a dependency \
         (`CompactString: GetSize`) for reasons unrelated to your project."
    )]
    NotFound,

    /// A path was given (explicitly or via the environment) but is unusable.
    #[error(
        "the monty worker binary at {path} is not usable: {reason}.\n\
         Install a working one with:\n\
         \x20      {INSTALL_COMMAND}"
    )]
    Unusable {
        /// The path that was tried.
        path: PathBuf,
        /// Why it could not be used.
        reason: String,
    },
}

/// How the worker binary should be found.
///
/// The default consults [`BINARY_ENV_VAR`] and then `PATH`; an explicit path
/// skips both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum WorkerBinary {
    /// Consult `ADK_MONTY_BINARY`, then `PATH`.
    #[default]
    Discover,
    /// Use exactly this path.
    Path(PathBuf),
}

impl WorkerBinary {
    /// Resolve to a usable path, or explain what to do about it.
    ///
    /// # Errors
    ///
    /// [`SetupError::NotFound`] when discovery finds nothing, or
    /// [`SetupError::Unusable`] when a configured path is missing or is not a
    /// file — a configured-but-wrong path is reported as such rather than
    /// silently falling through to discovery, because a caller who set a path
    /// meant it.
    pub fn resolve(&self) -> Result<PathBuf, SetupError> {
        match self {
            Self::Path(path) => check(path),
            Self::Discover => {
                if let Some(from_env) = std::env::var_os(BINARY_ENV_VAR)
                    .map(PathBuf::from)
                    .filter(|value| !value.as_os_str().is_empty())
                {
                    return check(&from_env);
                }
                find_on_path().ok_or(SetupError::NotFound)
            }
        }
    }
}

/// Whether a usable worker binary is available, and where.
///
/// Intended for a startup check or a `doctor`-style command, so an application
/// can fail fast with an actionable message instead of surfacing the problem
/// midway through an agent turn.
///
/// # Example
///
/// ```no_run
/// use adk_codeact_monty::setup::{WorkerBinary, probe};
///
/// match probe(&WorkerBinary::default()) {
///     Ok(path) => println!("monty worker: {}", path.display()),
///     // The error text names the install command, including `--locked`.
///     Err(err) => eprintln!("{err}"),
/// }
/// ```
pub fn probe(binary: &WorkerBinary) -> Result<PathBuf, SetupError> {
    binary.resolve()
}

/// Validate a configured path, preserving *why* it is unusable.
fn check(path: &Path) -> Result<PathBuf, SetupError> {
    let unusable = |reason: &str| SetupError::Unusable {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    };
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => Ok(path.to_path_buf()),
        Ok(_) => Err(unusable("not a file")),
        Err(err) => Err(unusable(&err.to_string())),
    }
}

/// Look for [`BINARY_NAME`] in the directories on `PATH`.
fn find_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(BINARY_NAME))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_path_is_used_as_given() {
        let file = std::env::current_exe().expect("the test binary exists");
        let resolved = WorkerBinary::Path(file.clone()).resolve().expect("a real file resolves");
        assert_eq!(resolved, file);
    }

    #[test]
    fn a_configured_path_that_is_wrong_is_reported_not_ignored() {
        // Falling through to discovery here would hide the caller's mistake and
        // could silently run a different binary than the one they named.
        let err = WorkerBinary::Path(PathBuf::from("/nonexistent/monty"))
            .resolve()
            .expect_err("a missing configured path must fail");
        match err {
            SetupError::Unusable { path, .. } => {
                assert_eq!(path, PathBuf::from("/nonexistent/monty"));
            }
            other => panic!("expected Unusable, got {other:?}"),
        }
        // A directory is not a worker either.
        let dir = std::env::temp_dir();
        assert!(matches!(
            WorkerBinary::Path(dir).resolve(),
            Err(SetupError::Unusable { reason, .. }) if reason == "not a file"
        ));
    }

    #[test]
    fn the_not_found_message_carries_every_fix_including_locked() {
        // This string is the one most users will ever read from this crate, and
        // `--locked` is what keeps them out of an upstream compile failure.
        let message = SetupError::NotFound.to_string();
        assert!(message.contains("--locked"), "{message}");
        assert!(message.contains("cargo install monty-runtime"), "{message}");
        assert!(message.contains(BINARY_ENV_VAR), "{message}");
        assert!(message.contains("worker_binary"), "{message}");
        assert!(message.contains("CompactString: GetSize"), "{message}");
    }

    #[test]
    fn a_real_worker_binary_resolves_when_one_is_installed() {
        // Verified against an actual `cargo install monty-runtime --locked`
        // artifact when present; skipped (not failed) otherwise, so a
        // contributor without the worker still gets an honest green run.
        let installed = std::path::Path::new("target/monty-tools/bin/monty");
        if !installed.is_file() {
            eprintln!(
                "skipped: no worker binary at {} — install it with `{INSTALL_COMMAND}`",
                installed.display()
            );
            return;
        }
        let resolved = WorkerBinary::Path(installed.to_path_buf())
            .resolve()
            .expect("an installed worker resolves");
        assert!(resolved.is_file());
    }

    #[test]
    fn discovery_is_the_default() {
        assert_eq!(WorkerBinary::default(), WorkerBinary::Discover);
    }
}
