//! Rhai scripting engine.
//!
//! `Script` is the entry point. [`Script::new_bare`] builds a [`rhai::Engine`] preloaded with
//! generic helpers (base64/json, sha256, url/basenames, yaml, semver, chrono, …) and optional
//! feature-gated ones (`fs`, `shell`, `password`, `crypto`, `k8s`, `oci`, `s3`, `http`).
//!
//! The engine also injects `assert` and `import_run` / `import_template` shims so scripts can
//! optionally import other modules without failing when they are absent.

#[cfg(feature = "password")] use crate::password::password_rhai_register;
#[cfg(feature = "shell")] use crate::shell::shell_rhai_register;
use crate::{
    Error::{self, RhaiError},
    Result, RhaiRes,
    chrono::chrono_rhai_register,
    glob::glob_rhai_register,
    hashes::hashes_rhai_register,
    rhai_err,
    semver::semver_rhai_register,
    yaml::yaml_rhai_register,
};
#[cfg(feature = "crypto")]
use crate::{hashes::crypto_hashes_rhai_register, key::key_rhai_register};
use base64::{Engine as _, engine::general_purpose::STANDARD};
pub use rhai::{
    AST, ASTNode, Array, Dynamic, Engine, Expr, ImmutableString, Map, Module, ParseError, Scope, Stmt,
    module_resolvers::{FileModuleResolver, ModuleResolversCollection},
    serde::to_dynamic,
};
use std::path::{Path, PathBuf};
use url::form_urlencoded;

/// Base64 (standard alphabet) decode of `input` into a UTF-8 string.
///
/// # Errors
///
/// Returns [`Error::Base64DecodeError`] if `input` is not valid base64, and [`Error::UTF8`]
/// if the decoded bytes are not valid UTF-8.
pub fn base64_decode(input: &str) -> Result<String> {
    let bytes = STANDARD.decode(input).map_err(Error::Base64DecodeError)?;
    String::from_utf8(bytes).map_err(Error::UTF8)
}

/// Percent-encode `arg` for use in a URL query string.
#[must_use]
pub fn url_encode(arg: &str) -> String {
    form_urlencoded::byte_serialize(arg.as_bytes()).collect::<String>()
}

fn core_common_rhai_register(engine: &mut Engine) {
    engine
        .register_fn("sha256", |v: String| sha256::digest(v))
        .register_fn("log_debug", |s: ImmutableString| tracing::debug!("{s}"))
        .register_fn("log_info", |s: ImmutableString| tracing::info!("{s}"))
        .register_fn("log_warn", |s: ImmutableString| tracing::warn!("{s}"))
        .register_fn("log_error", |s: ImmutableString| tracing::error!("{s}"))
        .register_fn("url_encode", url_encode)
        .register_fn("sleep", |seconds: i64| {
            if let Ok(seconds) = u64::try_from(seconds)
                && seconds > 0
            {
                std::thread::sleep(std::time::Duration::from_secs(seconds));
            }
        })
        .register_fn("get_env", |var: ImmutableString| -> RhaiRes<ImmutableString> {
            match std::env::var_os(var.to_string()) {
                // variable absente → chaîne vide (les défauts sont un cas d'usage)
                None => Ok(String::new().into()),
                // présente mais non UTF-8 → erreur explicite, jamais le repli silencieux
                Some(value) => value.into_string().map(Into::into).map_err(|_| {
                    rhai_err(Error::Other(format!(
                        "get_env received a non-UTF-8 value for variable {var:?}"
                    )))
                }),
            }
        })
        .register_fn("to_decimal", |val: ImmutableString| -> RhaiRes<u32> {
            // toute chaîne non octale est une erreur, jamais un `0` indiscernable du `0` légitime
            u32::from_str_radix(val.as_str(), 8).map_err(|_| {
                rhai_err(Error::Other(format!(
                    "to_decimal received a non-valid parameter: {val:?}"
                )))
            })
        })
        .register_fn(
            "base64_decode",
            |val: ImmutableString| -> RhaiRes<ImmutableString> {
                base64_decode(val.as_str())
                    .map_err(rhai_err)
                    .map(std::convert::Into::into)
            },
        )
        .register_fn("base64_encode", |val: ImmutableString| -> ImmutableString {
            STANDARD.encode(val.to_string()).into()
        })
        .register_fn("json_encode", |val: Dynamic| -> RhaiRes<ImmutableString> {
            serde_json::to_string(&val)
                .map_err(|e| rhai_err(Error::SerializationError(e)))
                .map(std::convert::Into::into)
        })
        .register_fn("json_encode_escape", |val: Dynamic| -> RhaiRes<ImmutableString> {
            let str = serde_json::to_string(&val).map_err(|e| rhai_err(Error::SerializationError(e)))?;
            Ok(format!("{str:?}").into())
        })
        .register_fn("json_decode", |val: ImmutableString| -> RhaiRes<Dynamic> {
            serde_json::from_str(val.as_ref()).map_err(|e| rhai_err(Error::SerializationError(e)))
        });
    engine
        .register_fn("basename", |name: String| -> ImmutableString {
            Path::new(&name)
                .file_name()
                .unwrap_or_default()
                .to_str()
                .unwrap_or_default()
                .into()
        })
        .register_fn("dirname", |name: String| -> ImmutableString {
            Path::new(&name)
                .parent()
                .unwrap_or(Path::new(""))
                .to_str()
                .unwrap_or_default()
                .into()
        });
}

/// Filesystem access exposed to Rhai scripts: read/write/copy files, create and list
/// directories. Gated behind the `fs` feature since consumers embedding untrusted or
/// multi-tenant scripts may not want to grant filesystem access on the host running them.
#[cfg(feature = "fs")]
fn fs_rhai_register(engine: &mut Engine) {
    engine
        .register_fn("file_read", |name: String| -> RhaiRes<ImmutableString> {
            std::fs::read_to_string(name)
                .map_err(|e| rhai_err(Error::Stdio(e)))
                .map(std::convert::Into::into)
        })
        .register_fn("file_write", |name: String, content: String| -> RhaiRes<()> {
            std::fs::write(name, content).map_err(|e| rhai_err(Error::Stdio(e)))
        })
        .register_fn("file_copy", |source: String, dest: String| -> RhaiRes<()> {
            std::fs::copy(source, dest)
                .map_err(|e| rhai_err(Error::Stdio(e)))
                .map(|_| ())
        })
        .register_fn("create_dir", |name: String| -> RhaiRes<()> {
            std::fs::create_dir_all(name).map_err(|e| rhai_err(Error::Stdio(e)))
        })
        .register_fn("read_dir", |name: String| -> RhaiRes<rhai::Array> {
            let mut res = rhai::Array::new();
            for entry in std::fs::read_dir(name).map_err(|e| rhai_err(Error::Stdio(e)))? {
                let entry = entry.map_err(|e| rhai_err(Error::Stdio(e)))?;
                // UTF-8 strict comme `file_read` : un chemin non UTF-8 rend Error::UTF8,
                // jamais une chaîne vide glissée dans le tableau.
                res.push(
                    entry
                        .path()
                        .into_os_string()
                        .into_string()
                        .or_else(|os| String::from_utf8(os.into_encoded_bytes()))
                        .map_err(|e| rhai_err(Error::UTF8(e)))?
                        .into(),
                );
            }
            Ok(res)
        })
        .register_fn("is_file", |name: String| -> bool { Path::new(&name).is_file() })
        .register_fn("is_dir", |name: String| -> bool { Path::new(&name).is_dir() });
}

/// Rhai engine + evaluation scope.
///
/// Create with [`Script::new_bare`], register extra functions on `engine` if needed,
/// then evaluate files or snippets. See crate docs for the list of built-in helpers.
#[derive(Debug)]
pub struct Script {
    /// The Rhai engine (register extra `fn`s here before evaluating).
    pub engine: Engine,
    /// Persistent scope (variables set via [`Script::set_dynamic`]).
    pub ctx: Scope<'static>,
}
impl Script {
    /// Create a new engine with generic helpers registered and the module resolver REPLACED
    /// (not extended — no default resolver to fall back on) by one [`FileModuleResolver`] per
    /// entry of `resolver_path`, in vector order, inside a [`ModuleResolversCollection`].
    ///
    /// An empty `resolver_path` is fail-closed by contract: the collection stays empty, so
    /// every `import` from a script fails with a module-not-found error — no implicit disk
    /// access without an explicit directory from the consumer. The injected
    /// `import_run`/`import_template` shims catch that error and skip silently (a debug log
    /// only), so scripts going through the shims never fail on absent modules; a direct
    /// `import` outside them does fail. Evaluating plain helper calls (below) needs no
    /// resolver at all.
    ///
    /// ```rust
    /// let mut s = vynil_core::engine::Script::new_bare(vec![]);
    /// assert_eq!(s.eval("sha256(\"hello\")").unwrap().into_string().unwrap(),
    ///     "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    /// ```
    #[must_use]
    pub fn new_bare(resolver_path: Vec<String>) -> Script {
        let mut script = Script {
            engine: Engine::new(),
            ctx: Scope::new(),
        };

        let mut resolver = ModuleResolversCollection::new();
        for path in resolver_path {
            resolver.push(FileModuleResolver::new_with_path(path));
        }
        script.engine.set_module_resolver(resolver);
        script.engine.set_max_expr_depths(256, 128);
        script.engine.set_max_call_levels(512);
        core_common_rhai_register(&mut script.engine);
        #[cfg(feature = "fs")]
        fs_rhai_register(&mut script.engine);
        chrono_rhai_register(&mut script.engine);
        hashes_rhai_register(&mut script.engine);
        #[cfg(feature = "crypto")]
        {
            crypto_hashes_rhai_register(&mut script.engine);
            key_rhai_register(&mut script.engine);
        }
        #[cfg(feature = "password")]
        password_rhai_register(&mut script.engine);
        semver_rhai_register(&mut script.engine);
        yaml_rhai_register(&mut script.engine);
        glob_rhai_register(&mut script.engine);
        #[cfg(feature = "oci")]
        crate::oci::oci_rhai_register(&mut script.engine);
        #[cfg(feature = "shell")]
        shell_rhai_register(&mut script.engine);
        script.add_common();
        script
    }

    /// Inject `assert` and `import_run`/`import_template` shims (called by `new_bare`).
    ///
    /// The shims are one long Rhai source each by design (they must be compiled as a single
    /// global module), which keeps this function verbose.
    #[allow(clippy::too_many_lines)] // shims Rhai `import_run`/`import_template` volontairement en un seul bloc (vyvil-core.sdd)
    pub fn add_common(&mut self) {
        // `add_common` n'a pas de canal d'erreur (engine.sdd) : un échec de shim est le
        // `tracing::error!` que `add_code` émet lui-même, d'où le `Result` abandonné.
        let _ = self.add_code("fn assert(cond, mess) {if (!cond){throw mess}}");
        let _ = self.add_code(
            "fn import_run(name, instance, context, args) {\n\
            try {\n\
                import name as imp;\n\
                return imp::run(instance, context, args);\n\
            } catch(e) {\n\
                if type_of(e) == \"map\" && \"error\" in e && e.error == \"ErrorModuleNotFound\" {\n\
                    log_debug(`No ${name} module, skipping.`);\n\
                } else if type_of(e) == \"map\" && \"error\" in e && e.error == \"ErrorFunctionNotFound\" {\n\
                    log_debug(`No ${name}::run function, skipping.`);\n\
                } else {\n\
                    throw ;\n\
                }\n\
            }\n\
        }",
        );
        let _ = self.add_code(
            "fn import_template(name, instance, context, args) {\n\
            try {\n\
                import name as imp;\n\
                return imp::template(instance, context, args);\n\
            } catch(e) {\n\
                if type_of(e) == \"map\" && \"error\" in e && e.error == \"ErrorModuleNotFound\" {\n\
                    log_debug(`No ${name} module, skipping.`);\n\
                } else if type_of(e) == \"map\" && \"error\" in e && e.error == \"ErrorFunctionNotFound\" {\n\
                    try {\n\
                        import name as imp;\n\
                        return imp::run(instance, context, args);\n\
                    } catch(e) {\n\
                        if type_of(e) == \"map\" && \"error\" in e && e.error == \"ErrorFunctionNotFound\" {\n\
                            log_debug(`No ${name}::run function, skipping.`);\n\
                        } else {\n\
                            throw;\n\
                        }\n\
                    }\n\
                } else {\n\
                    throw;\n\
                }\n\
            }\n\
        }",
        );
        let _ = self.add_code(
            "fn import_run(name, instance, context) {\n\
            try {\n\
                import name as imp;\n\
                return imp::run(instance, context);\n\
            } catch(e) {\n\
                if type_of(e) == \"map\" && \"error\" in e && e.error == \"ErrorModuleNotFound\" {\n\
                    log_debug(`No ${name} module, skipping.`);\n\
                } else if type_of(e) == \"map\" && \"error\" in e && e.error == \"ErrorFunctionNotFound\" {\n\
                    log_debug(`No ${name}::run function, skipping.`);\n\
                } else {\n\
                    throw;\n\
                }\n\
            }\n\
        }",
        );
        let _ = self.add_code(
            "fn import_template(name, instance, context) {\n\
            try {\n\
                import name as imp;\n\
                return imp::template(instance, context);\n\
            } catch(e) {\n\
                if type_of(e) == \"map\" && \"error\" in e && e.error == \"ErrorModuleNotFound\" {\n\
                    log_debug(`No ${name} module, skipping.`);\n\
                } else if type_of(e) == \"map\" && \"error\" in e && e.error == \"ErrorFunctionNotFound\" {\n\
                    try {\n\
                        import name as imp;\n\
                        return imp::run(instance, context);\n\
                    } catch(e) {\n\
                        if type_of(e) == \"map\" && \"error\" in e && e.error == \"ErrorFunctionNotFound\" {\n\
                            log_debug(`No ${name}::run function, skipping.`);\n\
                        } else {\n\
                            throw;\n\
                        }\n\
                    }\n\
                } else {\n\
                    throw;\n\
                }\n\
            }\n\
        }",
        );
        let _ = self.add_code(
            "fn import_run(name, args) {\n\
            try {\n\
                import name as imp;\n\
                return imp::run(args);\n\
            } catch(e) {\n\
                if type_of(e) == \"map\" && \"error\" in e && e.error == \"ErrorModuleNotFound\" {\n\
                    log_debug(`No ${name} module, skipping.`);\n\
                } else if type_of(e) == \"map\" && \"error\" in e && e.error == \"ErrorFunctionNotFound\" {\n\
                    log_debug(`No ${name}::run function, skipping.`);\n\
                } else {\n\
                    throw;\n\
                }\n\
            }\n\
        }",
        );
        let _ = self.add_code(
            "fn import_template(name, args) {\n\
            try {\n\
                import name as imp;\n\
                return imp::template(args);\n\
            } catch(e) {\n\
                if type_of(e) == \"map\" && \"error\" in e && e.error == \"ErrorModuleNotFound\" {\n\
                    log_debug(`No ${name} module, skipping.`);\n\
                } else if type_of(e) == \"map\" && \"error\" in e && e.error == \"ErrorFunctionNotFound\" {\n\
                    try {\n\
                        import name as imp;\n\
                        return imp::run(args);\n\
                    } catch(e) {\n\
                        if type_of(e) == \"map\" && \"error\" in e && e.error == \"ErrorFunctionNotFound\" {\n\
                            log_debug(`No ${name}::run function, skipping.`);\n\
                        } else {\n\
                            throw;\n\
                        }\n\
                    }\n\
                } else {\n\
                    throw;\n\
                }\n\
            }\n\
        }",
        );
    }

    /// Compile `code` and register its public functions as global Rhai modules.
    ///
    /// The module is evaluated on a CLONE of the current scope (a frozen snapshot, by
    /// contract): a later [`Script::set_dynamic`] does not feed modules already globalised.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RhaiError`] when compilation or module evaluation fails; the failed
    /// operation is also logged via `tracing::error!` with a `Compiling`/`Evaluating` label
    /// naming the step that actually failed.
    pub fn add_code(&mut self, code: &str) -> Result<()> {
        match self.engine.compile(code) {
            Ok(ast) => match Module::eval_ast_as_new(self.ctx.clone(), &ast, &self.engine) {
                Ok(module) => {
                    self.engine.register_global_module(module.into());
                    Ok(())
                }
                Err(e) => {
                    tracing::error!("Evaluating {code} failed with : {e:}");
                    Err(RhaiError(e))
                }
            },
            Err(e) => {
                tracing::error!("Compiling {code} failed with : {e:}");
                Err(RhaiError(e.into()))
            }
        }
    }

    /// Push a JSON value into the persistent Rhai scope under `name`.
    ///
    /// The JSON round-trip into a [`Dynamic`] cannot fail for a valid [`serde_json::Value`];
    /// a failure is logged and the scope is left untouched rather than propagated (this API
    /// has no error channel).
    pub fn set_dynamic(&mut self, name: &str, val: &serde_json::Value) {
        let converted = serde_json::to_string(val)
            .map_err(Error::from)
            .and_then(|json| serde_json::from_str::<Dynamic>(&json).map_err(Error::from));
        match converted {
            Ok(value) => {
                self.ctx.set_or_push(name, value);
            }
            Err(e) => {
                tracing::error!("cannot convert {name} to a Rhai Dynamic: {e}");
            }
        }
    }

    /// Evaluate the Rhai file at `file` inside the persistent scope.
    ///
    /// # Errors
    ///
    /// Returns [`Error::MissingScript`] if `file` is not a file, [`Error::Other`] if its path
    /// is not valid UTF-8, [`Error::RhaiError`] on a compile or evaluation failure.
    pub fn run_file(&mut self, file: &PathBuf) -> Result<Dynamic, Error> {
        if Path::new(&file).is_file() {
            let str = file
                .as_os_str()
                .to_str()
                .ok_or_else(|| Error::Other(format!("{}: path is not valid UTF-8", file.display())))?;
            match self.engine.compile_file(str.into()) {
                Ok(ast) => self
                    .engine
                    .eval_ast_with_scope::<Dynamic>(&mut self.ctx, &ast)
                    .map_err(Error::RhaiError),
                Err(e) => Err(Error::RhaiError(e)),
            }
        } else {
            Err(Error::MissingScript(file.clone()))
        }
    }

    /// Evaluate a Rhai snippet and return its [`Dynamic`] result.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RhaiError`] on a parse or evaluation failure.
    pub fn eval(&mut self, script: &str) -> Result<Dynamic, Error> {
        self.engine
            .eval_with_scope::<Dynamic>(&mut self.ctx, script)
            .map_err(RhaiError)
    }

    /// Evaluate a Rhai snippet expected to return `bool`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RhaiError`] on a parse/evaluation failure or when the result is not a
    /// `bool`.
    pub fn eval_truth(&mut self, script: &str) -> Result<bool, Error> {
        tracing::debug!("START: eval_truth({})", script);
        let r = self
            .engine
            .eval_with_scope::<bool>(&mut self.ctx, script)
            .map_err(RhaiError);
        tracing::debug!("END: eval_truth({})", script);
        r
    }

    /// Evaluate a Rhai snippet expected to return a `Map`, serialised to a JSON string.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RhaiError`] on a parse/evaluation failure or when the result is not a
    /// `Map`, and [`Error::SerializationError`] if the map cannot be serialised.
    pub fn eval_map_string(&mut self, script: &str) -> Result<String, Error> {
        tracing::debug!("START: eval_map_string({})", script);
        let m = self
            .engine
            .eval_with_scope::<Map>(&mut self.ctx, script)
            .map_err(RhaiError)?;
        tracing::debug!("END: eval_map_string({})", script);
        serde_json::to_string(&m).map_err(Error::SerializationError)
    }

    /// Evaluate a Rhai snippet expected to return a `Map`, as `serde_json::Value`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RhaiError`] on a parse/evaluation failure or when the result is not a
    /// `Map`, and [`Error::SerializationError`] if the map cannot be converted.
    pub fn eval_map_json(&mut self, script: &str) -> Result<serde_json::Value, Error> {
        let m = self
            .engine
            .eval_with_scope::<Map>(&mut self.ctx, script)
            .map_err(RhaiError)?;
        serde_json::to_value(&m).map_err(Error::SerializationError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

    fn make_script() -> Script {
        Script::new_bare(vec![])
    }

    // ── Harnais de capture des faits `tracing` (même patron que le verrou de src/hbs.rs) ──
    // Couche de capture minimale, confinée au fil courant via `with_default` (jamais
    // `set_global_default`) : ne retient que le texte formaté du champ `message`, pour
    // assertionner texte et nombre des `tracing::error!` contractés par `engine.sdd`.
    struct MessageCapture(Arc<Mutex<Vec<String>>>);

    impl<S: tracing::Subscriber> Layer<S> for MessageCapture {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            let mut visitor = MessageOnly(Vec::new());
            event.record(&mut visitor);
            self.0.lock().unwrap().extend(visitor.0);
        }
    }

    struct MessageOnly(Vec<String>);

    impl Visit for MessageOnly {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.0.push(format!("{value:?}"));
            }
        }
    }

    /// Évalue `run` sur `script` sous le subscriber de capture confiné au fil courant et
    /// retourne sa sortie avec les textes d'événement capturés, dans l'ordre d'émission.
    fn capture_logs<T>(script: &mut Script, run: impl FnOnce(&mut Script) -> T) -> (T, Vec<String>) {
        let captured: Arc<Mutex<Vec<String>>> = Arc::default();
        let subscriber =
            tracing_subscriber::registry::Registry::default().with(MessageCapture(Arc::clone(&captured)));
        let out = tracing::subscriber::with_default(subscriber, || run(script));
        (out, captured.lock().unwrap().clone())
    }

    // ── Fixture des shims d'import : un répertoire de résolution avec trois modules Rhai ──
    /// Écrit sous la face publique `new_bare(vec![dir])` un répertoire contenant :
    /// - `full.rhai` : `run` et `template` aux trois arités (valeurs constantes figeant
    ///   l'arité appelée),
    /// - `runonly.rhai` : `run` seulement (face fallback de `import_template`),
    /// - `bad.rhai` : `run` qui `throw "interne"` (face relance de l'erreur interne).
    fn resolver_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("full.rhai"),
            r#"
fn run(instance, context, args) { return "run/3"; }
fn run(instance, context) { return "run/2"; }
fn run(args) { return "run/1"; }
fn template(instance, context, args) { return "tpl/3"; }
fn template(instance, context) { return "tpl/2"; }
fn template(args) { return "tpl/1"; }
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("runonly.rhai"),
            r#"
fn run(instance, context, args) { return "fallback/3"; }
fn run(instance, context) { return "fallback/2"; }
fn run(args) { return "fallback/1"; }
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("bad.rhai"),
            r#"fn run(instance, context, args) { throw "interne"; }"#,
        )
        .unwrap();
        dir
    }

    // ── yaml_decode / yaml_encode ─────────────────────────────────────────────

    #[test]
    fn test_yaml_decode_string_value() {
        let mut s = make_script();
        let result = s.eval(r#"yaml_decode("key: hello")["key"]"#).unwrap();
        assert_eq!(result.to_string(), "hello");
    }

    #[test]
    fn test_yaml_decode_integer_value() {
        let mut s = make_script();
        let result = s.eval(r#"yaml_decode("count: 42")["count"]"#).unwrap();
        assert_eq!(result.cast::<i64>(), 42);
    }

    #[test]
    fn test_yaml_decode_boolean_value() {
        let mut s = make_script();
        let result = s.eval(r#"yaml_decode("enabled: true")["enabled"]"#).unwrap();
        assert!(result.cast::<bool>());
    }

    #[test]
    fn test_yaml_decode_nested_access() {
        let mut s = make_script();
        let result = s.eval(r#"yaml_decode("a:\n  b: nested")["a"]["b"]"#).unwrap();
        assert_eq!(result.to_string(), "nested");
    }

    #[test]
    fn test_yaml_decode_array_access() {
        let mut s = make_script();
        let result = s
            .eval(r#"yaml_decode("items:\n  - first\n  - second")["items"][1]"#)
            .unwrap();
        assert_eq!(result.to_string(), "second");
    }

    #[test]
    fn test_yaml_encode_produces_yaml() {
        let mut s = make_script();
        let result = s.eval(r#"yaml_encode(#{"key": "value"})"#).unwrap();
        let yaml_str = result.to_string();
        assert!(yaml_str.contains("key:"));
        assert!(yaml_str.contains("value"));
    }

    #[test]
    fn test_yaml_encode_decode_roundtrip() {
        let mut s = make_script();
        let result = s
            .eval(
                r#"
            let m = #{"name": "test", "count": 3};
            let encoded = yaml_encode(m);
            let decoded = yaml_decode(encoded);
            decoded["name"]
        "#,
            )
            .unwrap();
        assert_eq!(result.to_string(), "test");
    }

    // ── yaml_decode_multi ─────────────────────────────────────────────────────

    #[test]
    fn test_yaml_decode_multi_single_document() {
        let mut s = make_script();
        let result = s.eval(r#"yaml_decode_multi("key: val\n").len()"#).unwrap();
        assert_eq!(result.cast::<i64>(), 1);
    }

    #[test]
    fn test_yaml_decode_multi_two_documents() {
        let mut s = make_script();
        let result = s
            .eval(r#"yaml_decode_multi("key: a\n---\nkey: b\n").len()"#)
            .unwrap();
        assert_eq!(result.cast::<i64>(), 2);
    }

    #[test]
    fn test_yaml_decode_multi_document_values() {
        let mut s = make_script();
        let result = s
            .eval(
                r#"
            let docs = yaml_decode_multi("key: first\n---\nkey: second\n");
            docs[1]["key"]
        "#,
            )
            .unwrap();
        assert_eq!(result.to_string(), "second");
    }

    // ── json_encode / json_decode ─────────────────────────────────────────────

    #[test]
    fn test_json_encode_decode_roundtrip() {
        let mut s = make_script();
        let result = s
            .eval(
                r#"
            let encoded = json_encode(#{"a": "hello", "b": 42});
            let decoded = json_decode(encoded);
            decoded["a"]
        "#,
            )
            .unwrap();
        assert_eq!(result.to_string(), "hello");
    }

    #[test]
    fn test_json_decode_invalid_returns_error() {
        let mut s = make_script();
        assert!(s.eval(r#"json_decode("not json")"#).is_err());
    }

    // ── base64_encode / base64_decode ─────────────────────────────────────────

    #[test]
    fn test_base64_encode_decode_roundtrip() {
        let mut s = make_script();
        let result = s
            .eval(
                r#"
            let encoded = base64_encode("hello world");
            base64_decode(encoded)
        "#,
            )
            .unwrap();
        assert_eq!(result.to_string(), "hello world");
    }

    #[test]
    fn test_base64_encode_known_value() {
        let mut s = make_script();
        let result = s.eval(r#"base64_encode("hello")"#).unwrap();
        assert_eq!(result.to_string(), "aGVsbG8=");
    }

    // ── Semver from Rhai ──────────────────────────────────────────────────────

    #[test]
    fn test_semver_parse_and_to_string() {
        let mut s = make_script();
        let result = s.eval(r#"to_string(semver_from("1.2.3"))"#).unwrap();
        assert_eq!(result.to_string(), "1.2.3");
    }

    #[test]
    fn test_semver_comparison_operators() {
        let mut s = make_script();
        assert!(
            s.eval(r#"semver_from("1.0.0") < semver_from("2.0.0")"#)
                .unwrap()
                .cast::<bool>()
        );
        assert!(
            s.eval(r#"semver_from("2.0.0") > semver_from("1.0.0")"#)
                .unwrap()
                .cast::<bool>()
        );
        assert!(
            s.eval(r#"semver_from("1.0.0") == semver_from("1.0.0")"#)
                .unwrap()
                .cast::<bool>()
        );
    }

    #[test]
    fn test_semver_inc_minor() {
        let mut s = make_script();
        let result = s
            .eval(
                r#"
            let v = semver_from("1.2.3");
            inc_minor(v);
            to_string(v)
        "#,
            )
            .unwrap();
        assert_eq!(result.to_string(), "1.3.0");
    }

    // ── Utility functions ─────────────────────────────────────────────────────

    #[test]
    fn test_sha256_known_hash() {
        let mut s = make_script();
        let result = s.eval(r#"sha256("hello")"#).unwrap();
        assert_eq!(
            result.to_string(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn test_to_decimal_octal() {
        let mut s = make_script();
        let result = s.eval(r#"to_decimal("755")"#).unwrap();
        assert_eq!(result.cast::<u32>(), 493);
    }

    #[test]
    fn test_url_encode() {
        let mut s = make_script();
        let result = s.eval(r#"url_encode("hello world")"#).unwrap();
        assert_eq!(result.to_string(), "hello+world");
    }

    #[test]
    fn test_sleep_non_positive_returns_immediately() {
        let mut s = make_script();
        let start = std::time::Instant::now();
        assert!(s.eval("sleep(0); sleep(-3)").is_ok());
        assert!(start.elapsed() < std::time::Duration::from_millis(250));
    }

    #[test]
    fn test_sleep_positive_blocks_for_the_requested_duration() {
        let mut s = make_script();
        let start = std::time::Instant::now();
        assert!(s.eval("sleep(1)").is_ok());
        assert!(start.elapsed() >= std::time::Duration::from_millis(900));
    }

    // ── get_env : absente reste chaîne vide, non-UTF-8 rend une erreur (Scenario utilitaires) ──

    #[test]
    fn test_get_env_absent_variable_returns_empty_string() {
        let mut s = make_script();
        let result = s.eval(r#"get_env("VYVIL_ABSOLUTE_ABSENT_7F3C")"#).unwrap();
        assert_eq!(result.to_string(), "");
    }

    /// Contrat `get_env` : une variable PRÉSENTE mais non UTF-8 rend une erreur (jamais la
    /// chaîne vide, indiscernable de l'absence). La variable doit exister dans le processus
    /// sans passer par `std::env::set_var` — `unsafe` en édition 2024, hors d'atteinte sous
    /// `unsafe_code = "forbid"` — donc le père la réinjecte via `Command::env` (safe, accepte
    /// une `&OsStr` d'octets invalides) dans un
    /// fils qui rejoue ce seul test ; le marqueur `VYVIL_CHILD_ASSERTED` sur stdout prouve
    /// que le fils a bien exécuté ses assertions (et non filtré zéro test).
    #[cfg(unix)]
    #[test]
    fn test_get_env_non_utf8_value_is_error() {
        use std::os::unix::ffi::OsStrExt;
        const VAR: &str = "VYVIL_ENGINE_TEST_NOT_UTF8";
        const MARKER: &str = "VYVIL_ENGINE_TEST_CHILD";
        if std::env::var_os(MARKER).is_some() {
            let mut s = make_script();
            let r = s.eval(&format!("get_env(\"{VAR}\")"));
            let text = format!("{}", r.unwrap_err());
            assert!(
                text.contains("get_env received a non-UTF-8 value"),
                "expected the typed non-UTF-8 error, got {text}"
            );
            println!("VYVIL_CHILD_ASSERTED");
            return;
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("engine::tests::test_get_env_non_utf8_value_is_error")
            .arg("--nocapture")
            .env(MARKER, "1")
            .env(VAR, std::ffi::OsStr::from_bytes(&[0xff_u8]))
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success() && stdout.contains("VYVIL_CHILD_ASSERTED"),
            "the child must have run its assertions and succeeded: status {:?}, stdout {stdout}",
            out.status
        );
    }

    // ── to_decimal : toute chaîne non octale rend une erreur (jamais un 0 indiscernable) ──

    #[test]
    fn test_to_decimal_non_octal_is_error_and_zero_still_zero() {
        let mut s = make_script();
        let err = s.eval(r#"to_decimal("8")"#).unwrap_err();
        assert!(
            matches!(err, Error::RhaiError(_)),
            "expected RhaiError, got {err:?}"
        );
        assert!(
            format!("{err}").contains("to_decimal received a non-valid parameter"),
            "expected the module's wording in the error, got {err}"
        );
        assert_eq!(s.eval(r#"to_decimal("0")"#).unwrap().cast::<u32>(), 0);
    }

    // ── read_dir sous `fs` : UTF-8 strict comme `file_read` (jamais de chaîne vide glissée) ──

    #[cfg(feature = "fs")]
    #[test]
    fn test_read_dir_lists_full_paths_of_utf8_entries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        std::fs::write(dir.path().join("b.txt"), "y").unwrap();
        let mut s = make_script();
        let entries = s
            .eval(&format!("read_dir(\"{}\")", dir.path().display()))
            .unwrap()
            .cast::<rhai::Array>();
        assert_eq!(entries.len(), 2);
        for entry in &entries {
            assert!(
                entry
                    .to_string()
                    .starts_with(&dir.path().to_string_lossy().into_owned()),
                "each entry must be the full path, got {entry}"
            );
        }
    }

    #[cfg(all(feature = "fs", unix))]
    #[test]
    fn test_read_dir_non_utf8_entry_is_explicit_utf8_error() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::File::create(dir.path().join(std::ffi::OsStr::from_bytes(&[0xff_u8]))).unwrap();
        std::fs::write(dir.path().join("ok.txt"), "x").unwrap();
        let mut s = make_script();
        let err = s
            .eval(&format!("read_dir(\"{}\")", dir.path().display()))
            .unwrap_err();
        let text = format!("{err}");
        assert!(
            text.contains("UTF8 error"),
            "expected the Error::UTF8 wording, got {text}"
        );
    }

    // ── Les sept shims de `add_common` (face publique, Scenario add_code et shims) ──

    /// Shim 1/7 — `assert(cond, mess)` à l'unique arité 2 : jette la VALEUR `mess` telle
    /// quelle (pas forcée en chaîne), et une arité 1 est fonction-inconnue (rhai 1.25
    /// n'embarque pas d'`assert` natif).
    #[test]
    fn test_assert_shim_throws_value_and_locks_arity_two() {
        let mut s = make_script();
        let err = s.eval(r#"assert(false, "boom")"#).unwrap_err();
        assert!(
            format!("{err}").contains("boom"),
            "thrown value must carry the message, got {err}"
        );
        assert_eq!(
            s.eval(r"let out = 0; try { assert(false, 42); } catch (e) { out = e; } out")
                .unwrap()
                .cast::<i64>(),
            42,
            "the catch must see the thrown value as-is, not stringified"
        );
        assert!(s.eval(r#"assert(true, "ok")"#).is_ok());
        assert!(
            s.eval("assert(true)").is_err(),
            "single-arg assert must be an unknown function"
        );
    }

    /// Shim 2/7 — `import_run(name, instance, context, args)` rend la valeur d'`imp::run`
    /// telle quelle.
    #[test]
    fn test_import_run_arity4_returns_module_run_value() {
        let dir = resolver_dir();
        let mut s = Script::new_bare(vec![dir.path().to_string_lossy().into_owned()]);
        let result = s.eval(r#"import_run("full", #{}, #{}, [])"#).unwrap();
        assert_eq!(result.to_string(), "run/3");
    }

    /// Shim 3/7 — `import_run(name, instance, context)` → `imp::run(instance, context)`.
    #[test]
    fn test_import_run_arity3_returns_module_run_value() {
        let dir = resolver_dir();
        let mut s = Script::new_bare(vec![dir.path().to_string_lossy().into_owned()]);
        let result = s.eval(r#"import_run("full", #{}, #{})"#).unwrap();
        assert_eq!(result.to_string(), "run/2");
    }

    /// Shim 4/7 — `import_run(name, args)` → `imp::run(args)`.
    #[test]
    fn test_import_run_arity2_returns_module_run_value() {
        let dir = resolver_dir();
        let mut s = Script::new_bare(vec![dir.path().to_string_lossy().into_owned()]);
        let result = s.eval(r#"import_run("full", [])"#).unwrap();
        assert_eq!(result.to_string(), "run/1");
    }

    /// Shim 5/7 — `import_template(name, instance, context, args)` → `imp::template(...)`
    /// d'abord.
    #[test]
    fn test_import_template_arity4_returns_template_value() {
        let dir = resolver_dir();
        let mut s = Script::new_bare(vec![dir.path().to_string_lossy().into_owned()]);
        let result = s.eval(r#"import_template("full", #{}, #{}, [])"#).unwrap();
        assert_eq!(result.to_string(), "tpl/3");
    }

    /// Shim 6/7 — `import_template(name, instance, context)` → `imp::template(instance, context)`.
    #[test]
    fn test_import_template_arity3_returns_template_value() {
        let dir = resolver_dir();
        let mut s = Script::new_bare(vec![dir.path().to_string_lossy().into_owned()]);
        let result = s.eval(r#"import_template("full", #{}, #{})"#).unwrap();
        assert_eq!(result.to_string(), "tpl/2");
    }

    /// Shim 7/7 — `import_template(name, args)` → `imp::template(args)`.
    #[test]
    fn test_import_template_arity2_returns_template_value() {
        let dir = resolver_dir();
        let mut s = Script::new_bare(vec![dir.path().to_string_lossy().into_owned()]);
        let result = s.eval(r#"import_template("full", [])"#).unwrap();
        assert_eq!(result.to_string(), "tpl/1");
    }

    /// Face fallback de `import_template` (les trois arités) : module pourvu de `run` mais
    /// sans `template` → `ErrorFunctionNotFound` rattrapé, re-import puis `imp::run(...)`
    /// aux mêmes arguments — c'est la valeur de `run` qui sort.
    #[test]
    fn test_import_template_falls_back_to_run_when_template_missing() {
        let dir = resolver_dir();
        let mut s = Script::new_bare(vec![dir.path().to_string_lossy().into_owned()]);
        assert_eq!(
            s.eval(r#"import_template("runonly", #{}, #{}, [])"#)
                .unwrap()
                .to_string(),
            "fallback/3"
        );
        assert_eq!(
            s.eval(r#"import_template("runonly", #{}, #{})"#)
                .unwrap()
                .to_string(),
            "fallback/2"
        );
        assert_eq!(
            s.eval(r#"import_template("runonly", [])"#).unwrap().to_string(),
            "fallback/1"
        );
    }

    /// Scenario « resolver vide ferme les imports et les shims masquent » : sur
    /// `new_bare(vec![])`, `import_run`/`import_template` ne produisent JAMAIS d'erreur
    /// (skip silencieux), tandis qu'un `import` direct hors shim échoue en `RhaiError`.
    #[test]
    fn test_empty_resolver_shim_skips_and_direct_import_fails() {
        let mut s = make_script();
        assert!(s.eval(r#"import_run("absent", #{}, #{}, [])"#).is_ok());
        assert!(s.eval(r#"import_template("absent", #{}, #{}, [])"#).is_ok());
        assert!(s.eval(r#"import_run("absent", [])"#).is_ok());
        let err = s.eval(r#"import "absent" as x; x::run(1)"#).unwrap_err();
        assert!(
            matches!(err, Error::RhaiError(_)),
            "direct import must fail in RhaiError, got {err:?}"
        );
    }

    /// Le skip ne couvre QUE `ErrorModuleNotFound`/`ErrorFunctionNotFound` : un module dont
    /// `run` lance lui-même voit son erreur relancée (`throw`) et remonte en `RhaiError`.
    #[test]
    fn test_shim_rethrows_module_internal_error() {
        let dir = resolver_dir();
        let mut s = Script::new_bare(vec![dir.path().to_string_lossy().into_owned()]);
        let err = s.eval(r#"import_run("bad", #{}, #{}, [])"#).unwrap_err();
        assert!(
            format!("{err}").contains("interne"),
            "internal error must be rethrown, got {err}"
        );
    }

    // ── add_code : erreurs rendues avec le libellé de l'opération réellement en échec ──

    /// Code syntaxiquement invalide → `Error::RhaiError` rendu ET `tracing::error!`
    /// « Compiling ... failed with : ... » (pas « Evaluating » — libellés corrigés) ; le
    /// Script évalue normalement ensuite.
    #[test]
    fn test_add_code_compile_failure_is_rhai_error_logged_compiling() {
        let mut s = make_script();
        let (res, logs) = capture_logs(&mut s, |sc| sc.add_code("fn broken( {"));
        let err = res.unwrap_err();
        assert!(
            matches!(err, Error::RhaiError(_)),
            "expected RhaiError, got {err:?}"
        );
        assert!(
            logs.iter()
                .any(|l| l.starts_with("Compiling ") && l.contains(" failed with : ")),
            "expected a « Compiling ... failed with : » error, got {logs:?}"
        );
        assert!(
            !logs.iter().any(|l| l.starts_with("Evaluating ")),
            "the evaluating label must not appear on a compile failure, got {logs:?}"
        );
        assert_eq!(
            s.eval("1 + 1").unwrap().cast::<i64>(),
            2,
            "the script must keep evaluating"
        );
    }

    /// Code qui compile mais dont l'évaluation du module lève → `Error::RhaiError` rendu et
    /// libellé « Evaluating ... failed with : ... ».
    #[test]
    fn test_add_code_eval_failure_is_rhai_error_logged_evaluating() {
        let mut s = make_script();
        let (res, logs) = capture_logs(&mut s, |sc| sc.add_code("throw \"eval-boom\""));
        let err = res.unwrap_err();
        assert!(
            matches!(err, Error::RhaiError(_)),
            "expected RhaiError, got {err:?}"
        );
        assert!(
            logs.iter()
                .any(|l| l.starts_with("Evaluating ") && l.contains(" failed with : ")),
            "expected a « Evaluating ... failed with : » error, got {logs:?}"
        );
        assert!(
            !logs.iter().any(|l| l.starts_with("Compiling ")),
            "the compiling label must not appear on an eval failure, got {logs:?}"
        );
    }

    /// Rappel manuel d'`add_common` : les sept shims se recompilent et se réenregistrent
    /// sans erreur (aucun `tracing::error!`), et restent utilisables ensuite.
    #[test]
    fn test_add_common_recall_recompiles_shims_without_error() {
        let mut s = make_script();
        let ((), logs) = capture_logs(&mut s, Script::add_common);
        assert!(
            logs.is_empty(),
            "an add_common recall must emit no error log, got {logs:?}"
        );
        assert_eq!(
            s.eval(r"let out = 0; try { assert(false, 7); } catch (e) { out = e; } out")
                .unwrap()
                .cast::<i64>(),
            7,
            "the assert shim must still work after a recall"
        );
        assert!(
            s.eval(r#"import_run("absent", #{}, #{}, [])"#).is_ok(),
            "the import_run shim must still work after a recall"
        );
    }
}
