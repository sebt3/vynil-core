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
        .register_fn("get_env", |var: ImmutableString| -> String {
            std::env::var(var.to_string()).unwrap_or_default()
        })
        .register_fn("to_decimal", |val: ImmutableString| -> RhaiRes<u32> {
            Ok(u32::from_str_radix(val.as_str(), 8).unwrap_or_else(|_| {
                tracing::warn!("to_decimal received a non-valid parameter: {:?}", val);
                0
            }))
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
                res.push(entry.path().to_str().unwrap_or_default().into());
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
    /// Create a new engine with generic helpers registered and `resolver_path` added to the
    /// module resolver. `resolver_path` is a list of directories searched by `import` statements.
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
        self.add_code("fn assert(cond, mess) {if (!cond){throw mess}}");
        self.add_code(
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
        self.add_code(
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
        self.add_code(
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
        self.add_code(
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
        self.add_code(
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
        self.add_code(
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
    /// Errors are logged via `tracing::error!` and otherwise ignored.
    pub fn add_code(&mut self, code: &str) {
        match self.engine.compile(code) {
            Ok(ast) => match Module::eval_ast_as_new(self.ctx.clone(), &ast, &self.engine) {
                Ok(module) => {
                    self.engine.register_global_module(module.into());
                }
                Err(e) => {
                    tracing::error!("Parsing {code} failed with: {e:}");
                }
            },
            Err(e) => {
                tracing::error!("Loading {code} failed with: {e:}");
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

    fn make_script() -> Script {
        Script::new_bare(vec![])
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

    #[test]
    fn test_yaml_decode_multi_short_string_returns_empty() {
        let mut s = make_script();
        let result = s.eval(r#"yaml_decode_multi("ab").len()"#).unwrap();
        assert_eq!(result.cast::<i64>(), 0);
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
}
