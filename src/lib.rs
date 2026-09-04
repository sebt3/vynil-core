//! # vynil-core
//!
//! Generic Rust toolbox combining a [Rhai] scripting engine and a
//! [Handlebars] templating engine, with optional Kubernetes / OCI / S3 / HTTP handlers.
//!
//! Extracted from the [vynil](https://github.com/sebt3/vynil) workspace so it can be reused
//! by other projects (`kuberest`, `kydah`, …) without pulling in vynil's business abstractions
//! (CRDs, package model, instance controllers).
//!
//! > **Status:** API is not yet stable — expect breaking changes before `1.0`.
//!
//! # Features
//!
//! | Feature | Default | What it adds |
//! |---------|---------|--------------|
//! | `rhai` | ✅ | [`engine::Script`] engine + every other module's Rhai bindings |
//! | `hbs` | ✅ | [`hbs::HandleBars`] engine |
//! | `hbs-scripting` | ✅ | `register_helper_dir` / `rhai_register_helper_dir` (`handlebars/script_helper`). Implies `hbs` + `rhai`. Keep it separate because `script_helper` pulls `smartstring` which breaks `String + &String` in some graphs (see `hbs` docs) |
//! | `http` | ✅ | [`http::RestClient`] (reqwest) + [`http_mock::RestClientMock`]. Implies `rhai` |
//! | `crypto` | ✅ | `argon_hash` / `bcrypt_hash` / `gen_private_key` helpers (Handlebars + Rhai) |
//! | `k8s` | ❌ | Generic K8s handlers ([`k8s::K8sGeneric`], [`k8s::K8sObject`], …) + mocks. Implies `rhai` |
//! | `oci` | ❌ | [`oci::Registry`] + OCI mock. Implies `rhai` |
//! | `s3` | ❌ | S3 helpers ([`s3::s3_get_yaml`], [`s3::s3_list_keys`]). Implies `rhai` |
//! | `fs` | ❌ | Filesystem access from Rhai (`file_read`, `file_write`, …) |
//! | `shell` | ❌ | Shell execution (`shell::run` / `shell::get_out` + Rhai `shell_run`) |
//! | `password` | ❌ | `gen_password` / `gen_password_alphanum` (opt-in to avoid name collisions) |
//!
//! ```toml
//! # default: Rhai + Handlebars + HTTP + crypto
//! vynil-core = "0.7"
//! # Kubernetes project
//! vynil-core = { version = "0.7", features = ["k8s"] }
//! # Handlebars only, no Rhai/smartstring in the graph
//! vynil-core = { version = "0.7", default-features = false, features = ["hbs", "crypto"] }
//! ```
//!
//! # Quick start
//!
//! ```rust,no_run
//! vynil_core::set_client_name(|| "my-app.example.com".to_string());
//!
//! // Rhai
//! let mut script = vynil_core::engine::Script::new_bare(vec!["scripts/".into()]);
//! script.engine.register_fn("my_fn", |s: String| s.len() as i64);
//! // script.run_file(&std::path::PathBuf::from("scripts/run.rhai"))?;
//!
//! // Handlebars
//! let mut hbs = vynil_core::hbs::HandleBars::new();
//! let out = hbs.render("Hello {{ name }}!", &serde_json::json!({"name": "world"})).unwrap();
//! assert_eq!(out, "Hello world!");
//! # Ok::<(), vynil_core::Error>(())
//! ```
//!
//! # Client identity
//!
//! `vynil-core` does not assume an identity. Call `set_client_name` once at startup
//! before any HTTP or Kubernetes call, otherwise those calls panic with an actionable message.
//!
//! ```rust
//! vynil_core::set_client_name(|| "my-app.example.com".to_string());
//! assert!(vynil_core::client_name_is_set());
//! ```
//!
//! # Rhai helpers (injected by [`engine::Script::new_bare`])
//!
//! Common: `sha256`, `log_debug/info/warn/error`, `url_encode`, `get_env`, `to_decimal`,
//! `base64_encode/decode`, `json_encode/decode`, `basename`, `dirname`.
//! Additional, feature-gated: `yaml_encode/decode`, `semver_from` + `inc_*`, `glob`,
//! `date_now`/`format`, `crc32_hash`/`bcrypt_hash`/`argon`, `gen_private_key`,
//! `gen_password` (feature `password`), `file_*` (feature `fs`), `shell_*` (feature `shell`),
//! `Registry` / `s3_*` / `RestClient` / `k8s_*` when their feature is enabled.
//!
//! Scripts also get `assert` and `import_run` / `import_template` shims for optional imports.
//!
//! # Handlebars helpers (injected by [`hbs::HandleBars::new`])
//!
//! See [`hbs::CORE_HBS_HELPERS`] for the full list. Highlights: `base64_encode/decode`,
//! `url_encode`, `to_decimal`, `header_basic`, `crc32_hash`, `argon_hash`/`bcrypt_hash`/`gen_private_key`
//! (feature `crypto`), `gen_password*` (feature `password`), plus the `handlebars_misc_helpers`
//! set and the vendored `json_to_str` / `str_to_json` / `json_query` family.
//!
//! # Crate boundaries
//!
//! This crate stays generic: no dependency on `vynil`, `kuberest` or `kydah`, no default
//! client name, no CRDs or vynil-specific Handlebars helpers.
//!
//! [Rhai]: https://rhai.rs
//! [Handlebars]: https://handlebarsjs.com

#![cfg_attr(docsrs, feature(doc_cfg))]

use thiserror::Error;

/// Errors returned by `vynil-core` operations.
///
/// Variants behind feature gates are only available when that feature is enabled.
#[derive(Error, Debug)]
pub enum Error {
    /// JSON serialization / deserialization failure.
    #[error("SerializationError: {0}")]
    SerializationError(#[from] serde_json::Error),

    /// YAML parsing / serialisation failure. Payload is the underlying error string.
    #[error("YamlError: {0}")]
    YamlError(String),

    #[cfg(feature = "hbs")]
    #[cfg_attr(docsrs, doc(cfg(feature = "hbs")))]
    #[error("Registering template failed with error: {0}")]
    HbsTemplateError(#[from] handlebars::TemplateError),

    #[cfg(feature = "hbs")]
    #[cfg_attr(docsrs, doc(cfg(feature = "hbs")))]
    #[error("Renderer error: {0}")]
    HbsRenderError(#[from] handlebars::RenderError),

    #[cfg(feature = "rhai")]
    #[cfg_attr(docsrs, doc(cfg(feature = "rhai")))]
    #[error("Rhai script error: {0}")]
    RhaiError(#[from] Box<rhai::EvalAltResult>),

    #[cfg(feature = "http")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http")))]
    #[error("Reqwest error: {0}")]
    ReqwestError(#[from] reqwest::Error),

    /// JSON decoding of an HTTP body failed.
    #[error("Json decoding error: {0}")]
    JsonError(#[source] serde_json::Error),

    /// An HTTP call returned a non-success status.
    #[error("{0} query failed: {1}")]
    MethodFailed(String, u16, String),

    /// `RestClient::obj_*` was called with an unsupported method enum variant.
    #[error("Unsupported method")]
    UnsupportedMethod,

    /// Script file not found on disk.
    #[error("Missing script {0}")]
    MissingScript(std::path::PathBuf),

    /// UTF-8 conversion failure.
    #[error("UTF8 error {0}")]
    UTF8(#[from] std::string::FromUtf8Error),

    /// Semver parsing failure.
    #[error("Semver error {0}")]
    Semver(#[from] ::semver::Error),

    #[cfg(feature = "crypto")]
    #[cfg_attr(docsrs, doc(cfg(feature = "crypto")))]
    #[error("Argon2 password_hash error {0}")]
    Argon2hash(#[from] argon2::password_hash::Error),

    #[cfg(feature = "crypto")]
    #[cfg_attr(docsrs, doc(cfg(feature = "crypto")))]
    #[error("Bcrypt hash error {0}")]
    BcryptError(#[from] bcrypt::BcryptError),

    /// I/O error.
    #[error("Stdio error {0}")]
    Stdio(#[from] std::io::Error),

    /// Base64 decoding failure.
    #[error("Base64 decode error {0}")]
    Base64DecodeError(#[from] base64::DecodeError),

    /// Building a raw HTTP request failed.
    #[error("RAW api error {0}")]
    RawHTTP(#[from] ::http::Error),

    /// Integer parsing failure.
    #[error("ParseIntError {0}")]
    ParseInt(#[from] std::num::ParseIntError),

    #[cfg(feature = "crypto")]
    #[cfg_attr(docsrs, doc(cfg(feature = "crypto")))]
    #[error("KEY-OPENSSL-001 OpenSSL error {0}")]
    OpenSSL(#[from] openssl::error::ErrorStack),

    /// `gen_private_key` was called with an unknown algorithm.
    #[error("KEY-ALGO-001 Unsupported key algorithm: {0}")]
    UnsupportedKeyAlgorithm(String),

    /// Password generation spec was invalid.
    #[error("{0}")]
    PasswordSpec(String),

    /// Catch-all.
    #[error("Error: {0}")]
    Other(String),

    #[cfg(feature = "oci")]
    #[cfg_attr(docsrs, doc(cfg(feature = "oci")))]
    #[error("OCI jukebox error {0}")]
    OCIDistrib(#[from] oci_client::errors::OciDistributionError),

    #[cfg(feature = "oci")]
    #[cfg_attr(docsrs, doc(cfg(feature = "oci")))]
    #[error("OCI parse error {0}")]
    OCIParseError(#[from] oci_client::ParseError),

    #[cfg(feature = "k8s")]
    #[cfg_attr(docsrs, doc(cfg(feature = "k8s")))]
    #[error("K8s error: {0}")]
    KubeError(#[from] kube::Error),

    #[cfg(feature = "k8s")]
    #[cfg_attr(docsrs, doc(cfg(feature = "k8s")))]
    #[error("K8s wait error: {0}")]
    KubeWaitError(#[from] kube::runtime::wait::Error),

    #[cfg(feature = "k8s")]
    #[cfg_attr(docsrs, doc(cfg(feature = "k8s")))]
    #[error("Elapsed wait error: {0}")]
    Elapsed(#[from] tokio::time::error::Elapsed),

    #[cfg(feature = "k8s")]
    #[cfg_attr(docsrs, doc(cfg(feature = "k8s")))]
    #[error("Finalizer error: {0}")]
    FinalizerError(#[from] Box<kube::runtime::finalizer::Error<Error>>),
}

/// Crate result type. `E` defaults to [`enum@Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Result type used by Rhai-exposed functions. Alias for `Result<T, Box<EvalAltResult>>`.
#[cfg(feature = "rhai")]
#[cfg_attr(docsrs, doc(cfg(feature = "rhai")))]
pub type RhaiRes<T> = std::result::Result<T, Box<rhai::EvalAltResult>>;

/// Render an error together with its full `source()` chain, e.g.
/// `error sending request for url (...): dns error: failed to lookup address information: ...`.
///
/// Several error types this crate surfaces to Rhai — most notably `reqwest::Error` for a
/// connection-level failure (DNS, TLS, timeout, connection refused) — implement [`std::fmt::Display`]
/// on the outer error only, leaving the actual cause reachable solely through `Error::source()`
/// (i.e. visible in `{:?}` but not `{}`). Without walking the chain, a script (and whatever surfaces
/// its error, e.g. a JukeBox `Updated` condition) only ever sees an opaque
/// "error sending request for url (...)" with no indication of *why* the request failed.
/// A source already restating an ancestor's message verbatim (e.g. `Error::ReqwestError`'s
/// `#[error("Reqwest error: {0}")]` Display, which embeds its wrapped `reqwest::Error`'s own
/// Display) is skipped rather than appended again.
pub fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut acc = err.to_string();
    let mut source = err.source();
    while let Some(e) = source {
        let msg = e.to_string();
        if !acc.contains(&msg) {
            acc.push_str(": ");
            acc.push_str(&msg);
        }
        source = e.source();
    }
    acc
}

/// Convert a [`enum@Error`] into a Rhai `EvalAltResult`, including its full `source()` chain
/// (see [`error_chain`]) so the real cause of a connection-level failure isn't swallowed.
#[cfg(feature = "rhai")]
pub fn rhai_err(e: Error) -> Box<rhai::EvalAltResult> {
    error_chain(&e).into()
}

/// Convert a string into a Rhai `EvalAltResult`.
#[cfg(feature = "rhai")]
pub fn rhai_err_str(e: String) -> Box<rhai::EvalAltResult> {
    e.into()
}

/// Date/time helpers (`DateTimeHandler`).
pub mod chrono;
/// Global client identity (`User-Agent` / field-manager).
pub mod client_name;
/// Hash helpers (crc32, bcrypt, argon2).
pub mod hashes;
/// Password generation.
pub mod password;
/// Semver parsing and mutation.
pub mod semver;
/// YAML ↔ JSON helpers.
pub mod yaml;

#[cfg(feature = "crypto")]
#[cfg_attr(docsrs, doc(cfg(feature = "crypto")))]
/// Private key generation (RSA / ed25519 via OpenSSL).
pub mod key;

#[cfg(feature = "rhai")]
#[cfg_attr(docsrs, doc(cfg(feature = "rhai")))]
/// Rhai scripting engine ([`engine::Script`]) and its registered helpers.
pub mod engine;
#[cfg(feature = "rhai")]
#[cfg_attr(docsrs, doc(cfg(feature = "rhai")))]
/// Glob matching (`glob` Rhai helper).
pub mod glob;

#[cfg(feature = "hbs")]
#[cfg_attr(docsrs, doc(cfg(feature = "hbs")))]
/// Handlebars templating engine ([`hbs::HandleBars`]) and its helpers.
pub mod hbs;
#[cfg(feature = "hbs")] mod hbs_json;

#[cfg(feature = "http")]
#[cfg_attr(docsrs, doc(cfg(feature = "http")))]
/// HTTP client ([`http::RestClient`]).
pub mod http;
#[cfg(feature = "http")]
#[cfg_attr(docsrs, doc(cfg(feature = "http")))]
/// Mock HTTP client for tests ([`http_mock::RestClientMock`]).
pub mod http_mock;

#[cfg(feature = "oci")]
#[cfg_attr(docsrs, doc(cfg(feature = "oci")))]
/// OCI registry client ([`oci::Registry`]).
pub mod oci;
#[cfg(feature = "oci")]
#[cfg_attr(docsrs, doc(cfg(feature = "oci")))]
/// Mock OCI helpers.
pub mod oci_mock;

#[cfg(feature = "s3")]
#[cfg_attr(docsrs, doc(cfg(feature = "s3")))]
/// S3 helpers (`s3_get_yaml`, `s3_list_keys`).
pub mod s3;

#[cfg(feature = "k8s")]
#[cfg_attr(docsrs, doc(cfg(feature = "k8s")))]
/// Kubernetes handlers (`K8sGeneric`, `K8sObject`, …).
pub mod k8s;
#[cfg(feature = "k8s")]
#[cfg_attr(docsrs, doc(cfg(feature = "k8s")))]
/// Mock Kubernetes helpers.
pub mod k8s_mock;

#[cfg(feature = "shell")]
#[cfg_attr(docsrs, doc(cfg(feature = "shell")))]
/// Shell execution helpers.
pub mod shell;

pub use client_name::{client_name_is_set, get_client_name, set_client_name};
pub use semver::Semver;

#[cfg(feature = "rhai")]
#[cfg_attr(docsrs, doc(cfg(feature = "rhai")))]
pub use engine::Script;
#[cfg(feature = "hbs")]
#[cfg_attr(docsrs, doc(cfg(feature = "hbs")))]
pub use hbs::HandleBars;

#[cfg(feature = "k8s")]
#[cfg_attr(docsrs, doc(cfg(feature = "k8s")))]
pub use k8s::update_cache;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    #[derive(Debug)]
    struct Layered {
        msg: &'static str,
        source: Option<Box<Layered>>,
    }
    impl fmt::Display for Layered {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.msg)
        }
    }
    impl std::error::Error for Layered {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.source
                .as_deref()
                .map(|e| e as &(dyn std::error::Error + 'static))
        }
    }

    #[test]
    fn error_chain_walks_every_source() {
        let err = Layered {
            msg: "error sending request for url (https://gitlab.com/api/v4/projects)",
            source: Some(Box::new(Layered {
                msg: "dns error: failed to lookup address information",
                source: Some(Box::new(Layered {
                    msg: "Temporary failure in name resolution",
                    source: None,
                })),
            })),
        };
        assert_eq!(
            error_chain(&err),
            "error sending request for url (https://gitlab.com/api/v4/projects): \
             dns error: failed to lookup address information: \
             Temporary failure in name resolution"
        );
    }

    #[test]
    fn error_chain_collapses_a_duplicate_leading_source() {
        // Mirrors `Error::ReqwestError`, whose `#[error("Reqwest error: {0}")]` Display already
        // embeds its wrapped source's own message verbatim.
        let inner = Layered {
            msg: "error sending request for url (https://gitlab.com/api/v4/projects)",
            source: None,
        };
        let outer = Layered {
            msg: "Reqwest error: error sending request for url (https://gitlab.com/api/v4/projects)",
            source: Some(Box::new(Layered {
                msg: "error sending request for url (https://gitlab.com/api/v4/projects)",
                source: None,
            })),
        };
        assert_eq!(error_chain(&inner), inner.msg);
        assert_eq!(
            error_chain(&outer),
            outer.msg,
            "duplicate source line must be collapsed"
        );
    }

    #[test]
    fn error_chain_single_error_has_no_source() {
        let err = Layered {
            msg: "boom",
            source: None,
        };
        assert_eq!(error_chain(&err), "boom");
    }
}
