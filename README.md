# vynil-core

A Rust toolbox for bootstrapping projects that combine a **Rhai** scripting engine and a
**Handlebars** templating engine, with optional Kubernetes / OCI / S3 handlers.

`vynil-core` is the generic layer extracted from [vynil](https://github.com/sebt3/vynil) so it
can be reused by other projects (kuberest, kydah, …) without pulling in vynil's business
abstractions (CRDs, package model, instance controllers).

> **Status:** extracted from the vynil workspace and published independently. The public API is
> not yet stable — expect breaking changes before 1.0.

## What it provides

Always present (no feature needed):

- **Rhai engine** — `Script::new_bare(resolver_paths)` with the generic helpers (datetime, hashes,
  password, ed25519 keys, semver, glob, shell, serde-YAML, base64/json, file I/O).
- **Handlebars engine** — `HandleBars::new()` with the generic helpers (base64, bcrypt, argon,
  password, encoding, concat, plus the `handlebars_misc_helpers` set) and `engine_mut()` to extend it.
- **HTTP** — `RestClient` (reqwest) and its mock `RestClientMock`.

## Features

| Feature | Adds | Pulls in |
|---------|------|----------|
| *(default)* | Rhai + Handlebars + HTTP + generic helpers | rhai, handlebars, reqwest, … |
| `k8s` | generic K8s handlers + their mocks | `kube`, `k8s-openapi`, `futures` |
| `oci` | `Registry` + OCI mock | `oci-client` |
| `s3` | `S3` client | `object_store` |

```toml
# minimal (templates + scripts, no specialised network)
vynil-core = "0.5"

# Kubernetes project
vynil-core = { version = "0.5", features = ["k8s"] }
```

## Client identity

`vynil-core` performs HTTP calls (and, with the `k8s` feature, Server-Side-Apply calls) on
behalf of the consuming application. It does not assume an identity for you — call
`vynil_core::set_client_name(...)` once at startup, before issuing any request:

```rust
vynil_core::set_client_name(|| "my-app.example.com".to_string());
```

Any `RestClient` or `k8s` call made before this is configured will panic with a message
explaining what to do.

## Minimal example

```rust
let mut script = vynil_core::Script::new_bare(vec!["scripts/".into()]);
script.engine.register_fn("my_fn", my_fn);          // register your own helpers
script.run_file(&std::path::PathBuf::from("scripts/run.rhai"))?;

let mut hbs = vynil_core::HandleBars::new();
hbs.engine_mut().register_helper("my_helper", Box::new(my_helper));
let output = hbs.render(template_str, &data)?;
```

## What it does NOT include

The vynil-specific layer stays in vynil: CRDs, `VynilContext`, `VynilPackage`, the instance
macros, the order-preserving `YamlDoc`, and the context-aware Handlebars helpers
(`selector_from_ctx`, `labels_from_ctx`, `image_from_ctx`, …). Consumers add their own on top of
the `Script`/`HandleBars` engines.

## License

BSD 3-Clause (same as vynil).
