//! Shell execution helpers.
//!
//! `run` / `get_out` are the Rust APIs; `shell_run` / `shell_output` are the Rhai
//! bindings gated behind the `shell` feature (plus `rhai` for the bindings).

use crate::{Error, Result};
#[cfg(feature = "rhai")] use crate::{RhaiRes, rhai_err};
#[cfg(feature = "rhai")] use rhai::Engine;
use std::process::{Command, Output, Stdio};

/// Run `sh -c <command>` inheriting stdout/stderr. Returns the raw [`Output`].
///
/// # Errors
///
/// Returns [`Error::Stdio`] if the shell cannot be spawned.
pub fn run(command: String) -> Result<Output> {
    Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output()
        .map_err(Error::Stdio)
}

/// Rhai binding of [`run`]: runs the command and returns its exit code (`0` when not a numeric
/// exit status, e.g. killed by signal).
///
/// # Errors
///
/// Returns a Rhai error wrapping [`Error::Stdio`] if the shell cannot be spawned.
#[cfg(feature = "rhai")]
pub fn rhai_run(command: String) -> RhaiRes<i64> {
    let out = run(command).map_err(rhai_err)?;
    Ok(i64::from(out.status.code().unwrap_or(0)))
}

/// Run `sh -c <command>` capturing stdout/stderr. Returns the raw [`Output`].
///
/// # Errors
///
/// Returns [`Error::Stdio`] if the shell cannot be spawned.
pub fn get_out(command: String) -> Result<Output> {
    Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(Error::Stdio)
}

/// Rhai binding of [`get_out`]: returns captured stdout; fails on non-zero exit or non-empty
/// stderr (also logged at warn level).
///
/// # Errors
///
/// Returns a Rhai error wrapping [`Error::Stdio`] if the shell cannot be spawned, [`Error::UTF8`]
/// on non-UTF-8 output, or [`Error::Other`] on a failed command or stderr content.
#[cfg(feature = "rhai")]
pub fn rhai_get_stdout(command: String) -> RhaiRes<String> {
    let out = get_out(command).map_err(rhai_err)?;
    if !out.status.success() {
        Err(rhai_err(Error::Other(format!(
            "Command failed, rc={}",
            out.status.code().unwrap_or(-1)
        ))))
    } else if !out.stderr.is_empty() {
        let err = String::from_utf8(out.stderr).map_err(|e| rhai_err(Error::UTF8(e)))?;
        tracing::warn!(err);
        Err(rhai_err(Error::Other(format!("Command had stderr : {err}"))))
    } else {
        let output = String::from_utf8(out.stdout).map_err(|e| rhai_err(Error::UTF8(e)))?;
        Ok(output)
    }
}

/// Registers the shell Rhai helpers on a Rhai `engine`.
#[cfg(feature = "rhai")]
pub fn shell_rhai_register(engine: &mut Engine) {
    engine
        .register_fn("shell_run", rhai_run)
        .register_fn("shell_output", rhai_get_stdout);
}
