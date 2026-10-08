//! Shell execution helpers.
//!
//! `run` / `get_out` are the Rust APIs; `shell_run` / `shell_output` are the Rhai
//! bindings gated behind the `shell` feature (plus `rhai` for the bindings).

use crate::{Error, Result};
#[cfg(feature = "rhai")] use crate::{RhaiRes, rhai_err};
#[cfg(feature = "rhai")] use rhai::Engine;
use std::process::{Command, Output, Stdio};

/// Verdict de terminaison partagé par les deux collages Rhai : le code d'exit élargi quand
/// il existe, sinon — sous unix — `128 + signal` (convention du shell : `kill -TERM` rend
/// `143`), sinon `-1`. Une mort par signal n'est ainsi jamais confondue avec un succès `0`.
#[cfg(feature = "rhai")]
fn exit_verdict(status: std::process::ExitStatus) -> i64 {
    if let Some(code) = status.code() {
        i64::from(code)
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt as _;
            status
                .signal()
                .and_then(|sig| i64::from(sig).checked_add(128))
                .unwrap_or(-1)
        }
        #[cfg(not(unix))]
        {
            -1
        }
    }
}

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

/// Rhai binding of [`run`]: runs the command and returns its exit code as an integer —
/// `128 + signal` when killed by a signal (unix), `-1` when neither is available; a signal
/// death is never rendered as a success `0`. No non-zero code produces an error here.
///
/// # Errors
///
/// Returns a Rhai error wrapping [`Error::Stdio`] if the shell cannot be spawned.
#[cfg(feature = "rhai")]
pub fn rhai_run(command: String) -> RhaiRes<i64> {
    let out = run(command).map_err(rhai_err)?;
    Ok(exit_verdict(out.status))
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

/// Rhai binding of [`get_out`]: returns captured stdout; fails on non-zero exit — with the
/// exit verdict and, when stderr is non-empty, a `from_utf8_lossy` excerpt of its first 512
/// bytes trimmed of trailing whitespace — or on non-empty stderr at exit 0 (also logged at
/// warn level).
///
/// # Errors
///
/// Returns a Rhai error wrapping [`Error::Stdio`] if the shell cannot be spawned, [`Error::UTF8`]
/// on non-UTF-8 output, or [`Error::Other`] on a failed command or stderr content.
#[cfg(feature = "rhai")]
pub fn rhai_get_stdout(command: String) -> RhaiRes<String> {
    let out = get_out(command).map_err(rhai_err)?;
    if !out.status.success() {
        let mut msg = format!("Command failed, rc={}", exit_verdict(out.status));
        if !out.stderr.is_empty() {
            let excerpt = String::from_utf8_lossy(&out.stderr[..out.stderr.len().min(512)]);
            msg.push_str(": ");
            msg.push_str(excerpt.trim_end());
        }
        return Err(rhai_err(Error::Other(msg)));
    }
    if !out.stderr.is_empty() {
        let err = String::from_utf8(out.stderr).map_err(|e| rhai_err(Error::UTF8(e)))?;
        tracing::warn!(err);
        return Err(rhai_err(Error::Other(format!("Command had stderr : {err}"))));
    }
    let output = String::from_utf8(out.stdout).map_err(|e| rhai_err(Error::UTF8(e)))?;
    Ok(output)
}

/// Registers the shell Rhai helpers on a Rhai `engine`.
#[cfg(feature = "rhai")]
pub fn shell_rhai_register(engine: &mut Engine) {
    engine
        .register_fn("shell_run", rhai_run)
        .register_fn("shell_output", rhai_get_stdout);
}

/// Scenarios de `shell.sdd` sur le verdict de signal et l'extrait de stderr borné à 512
/// octets (tâche « Unifier le verdict de signal »). Gating : le module `shell` est déjà
/// gated `shell` dans ./lib.rs et les items `rhai_*` gated `rhai` ; les cinq verrous
/// passent tous par un engine enregistré par `crate::shell::shell_rhai_register`, la table
/// des portes impose donc `all(test, feature = "shell", feature = "rhai")`, épelé en dur
/// comme dans ./hashes.rs. Les deux verrous de signal portent `#[cfg(unix)]` : la branche
/// non-unix (le `-1` de repli) est hors harnais par construction, le Scenario le dit.
#[cfg(all(test, feature = "shell", feature = "rhai"))]
mod tests {
    use crate::shell::shell_rhai_register;
    use rhai::Engine;

    fn engine() -> Engine {
        let mut e = Engine::new();
        shell_rhai_register(&mut e);
        e
    }

    /// Texte porté par l'`EvalAltResult` d'une évaluation en échec, extrait de son
    /// enveloppe rhai (`ErrorRuntime(Dynamic)`, variante fabriquée par `crate::rhai_err`) :
    /// seul le texte porté est contractuel, pas le décorateur de la runtime.
    fn script_error_text(e: &Engine, script: &str) -> String {
        let err = e.eval::<String>(script).expect_err("the script call must fail");
        match err.as_ref() {
            rhai::EvalAltResult::ErrorRuntime(d, _) => d.as_immutable_string_ref().unwrap().to_string(),
            other => panic!("expected the error carried by the registered function, got: {other}"),
        }
    }

    // ── Scenario « la mort par signal rend 128 + signal des deux côtés » ──

    /// Verrou 1, voie `shell_run` : `kill -TERM $$` n'est jamais un succès `0` ; le helper
    /// substitue `128 + 15` quand `ExitStatus::code` rend `None`. Rouge à l'état d'avant :
    /// `rhai_run` rend `unwrap_or(0)`, soit `0`, la confusion exacte que le `Must` interdit.
    #[test]
    #[cfg(unix)]
    fn signal_via_shell_run_renders_143() {
        let got: i64 = engine()
            .eval(r#"shell_run("kill -TERM $$")"#)
            .expect("shell_run must not fail on a signal death");
        assert_eq!(got, 143, "signal TERM must render 128 + 15, never 0");
    }

    /// Verrou 2, voie `shell_output` : même verdict, en échec portant exactement
    /// `Error: Command failed, rc=143` (préfixe `Error: ` de `Error::Other` inclus, aucun
    /// extrait puisque le stderr est vide). Rouge à l'état d'avant : `unwrap_or(-1)` rend
    /// `rc=-1`.
    #[test]
    #[cfg(unix)]
    fn signal_via_shell_output_fails_with_rc_143() {
        let text = script_error_text(&engine(), r#"shell_output("kill -TERM $$")"#);
        assert_eq!(text, "Error: Command failed, rc=143");
    }

    // ── Scenario « shell_output échoue sur tout code non nul avec son rc rendu » ──

    /// Verrou 3 : code non nul avec stderr → `Command failed, rc=2: alerte`, extrait de
    /// stderr débarrassé de sa nouvelle de fin ; le stdout `sortie` n'apparaît nulle part.
    #[test]
    fn shell_output_nonzero_rc_renders_stderr_excerpt_not_stdout() {
        let text = script_error_text(
            &engine(),
            r#"shell_output("echo sortie && echo alerte >&2 && exit 2")"#,
        );
        assert_eq!(text, "Error: Command failed, rc=2: alerte");
        assert!(
            !text.contains("sortie"),
            "the stdout must never surface in the message, got: {text}"
        );
    }

    /// Verrou 4 : sans stderr, pas de suffixe `: <extrait>` — exactement
    /// `Error: Command failed, rc=2`. Ancre de non-régression : vert à l'état d'avant sur
    /// un code numérique, verrouillé pour que le nouveau collage ne le déplace pas.
    #[test]
    fn shell_output_nonzero_rc_without_stderr_renders_bare_rc() {
        let text = script_error_text(&engine(), r#"shell_output("exit 2")"#);
        assert_eq!(text, "Error: Command failed, rc=2");
    }

    /// Verrou 5 : extrait borné à exactement 512 octets. Source déterministe ASCII
    /// (`printf 'x%.0s' $(seq 1 600)` → 600 octets `x` sur stderr, sans nouvelle de fin) :
    /// à l'octet près, octets et caractères coïncident (ASCII), la troncature est donc
    /// verrouillée à la borne des 512 octets et non « à peu près ». Rouge à l'état
    /// d'avant : aucun extrait de stderr n'est rendu du tout.
    #[test]
    fn shell_output_stderr_excerpt_capped_at_512_bytes() {
        let text = script_error_text(
            &engine(),
            r#"shell_output("printf 'x%.0s' $(seq 1 600) >&2; exit 1")"#,
        );
        let expected = format!("Error: Command failed, rc=1: {}", "x".repeat(512));
        assert_eq!(
            text,
            expected,
            "excerpt must be exactly 512 bytes of the stderr, got len {}",
            text.len()
        );
    }
}
