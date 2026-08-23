# Rhai helpers

Reference for every Rhai function, type, and method `vynil-core` registers into a
[`rhai::Engine`](https://rhai.rs). For the Handlebars side see [handlebars_helpers.md](handlebars_helpers.md);
for the test doubles that mirror the network/cluster-facing APIs below, see [mocking.md](mocking.md).

## Cargo feature required

Everything in this document — the Rhai engine itself (`Script`, `engine.rs`) included — lives
behind the `rhai` Cargo feature. It's on by default, so nothing below changes for an existing
`vynil-core = "x"` dependency; it only matters if a consumer opts out with
`default-features = false` to get a Rhai-free build (e.g. Handlebars-only — see
[handlebars_helpers.md](handlebars_helpers.md)).

## How registration works

`vynil_core::engine::Script::new_bare(resolver_paths)` builds an `Engine` and auto-registers a
fixed set of modules. Not everything below is wired in automatically — three different tiers:

| Tier | Modules | Wired in by `Script::new_bare` |
|---|---|---|
| Always (given `rhai`) | core utilities, `chrono`, `hashes` (crc32 only), `semver`, `yaml`, `glob` | Yes, unconditionally |
| Cargo-feature-gated, auto-wired | `fs`, `password`, `crypto` (`hashes`'s bcrypt/argon + `key`), `oci`, `shell` | Yes, when the matching Cargo feature is enabled |
| Never auto-wired | `http`, `k8s` (all three sub-modules), `s3` | **No** — call the `*_rhai_register` function yourself on `script.engine` |

The third tier is opt-in regardless of Cargo features — `http`/`k8s`/`s3` each pull in `rhai`
themselves (their core types are built as Rhai-facing APIs, not a plain-Rust API with a Rhai
wrapper on top), so enabling one of those three features is enough to make the type available;
it's registration into a *given* `Engine` that stays a manual call. This crate never assumes a
consumer wants live network or cluster access baked into every script engine, so those are opt-in
calls:

```rust
let mut script = vynil_core::Script::new_bare(vec!["scripts/".into()]);
vynil_core::http::http_rhai_register(&mut script.engine);
#[cfg(feature = "k8s")]
{
    vynil_core::k8s::k8sgeneric_rhai_register(&mut script.engine);
    vynil_core::k8s::k8sraw_rhai_register(&mut script.engine);
    vynil_core::k8s::k8sworkload_rhai_register(&mut script.engine);
}
#[cfg(feature = "s3")]
vynil_core::s3::s3_rhai_register(&mut script.engine);
```

`k8s` additionally needs runtime context before any script can use it — see the "Feature `k8s`"
section further down in this document.

### Why this tier is opt-in

Beyond just avoiding unwanted network/cluster access by default, leaving `http`/`k8s`/`s3` out of
`new_bare` is what makes them mockable at the call site instead of at the crate level. Each of
these three modules has a matching mock module (`http_mock`, `k8s_mock`, `oci_mock` — see
[mocking.md](mocking.md)) that registers the **exact same Rhai type and function names**
(`RestClient`, `K8sGeneric`, …) against in-memory fixtures instead of a live endpoint. Because
registration is an explicit call rather than something baked into `Script::new_bare`, a consumer
can build two engines from the very same `.rhai` script source — one wired to the real
`*_rhai_register` for production, one wired to the matching `*_mock_rhai_register` for a unit
test — and run that script against both without touching a real cluster, registry, or HTTP
endpoint in the test. If registration were automatic, that swap wouldn't be possible without
threading a runtime flag through `Script::new_bare` itself.

## Cargo feature reference

| Feature | Unlocks | Auto-wired into `new_bare`? |
|---|---|---|
| `rhai` *(default)* | this entire document — core utilities, chrono, hashes' `crc32_hash`, semver, yaml, glob | always |
| `fs` | filesystem access (`file_read`, …) | yes |
| `password` | `gen_password`, `gen_password_alphanum` | yes |
| `crypto` *(default)* | hashes' `bcrypt_hash`/`Argon`, `key`'s `gen_private_key` | yes |
| `shell` | `shell_run`, `shell_output` | yes |
| `oci` *(implies `rhai`)* | `Registry` OCI client | yes |
| `k8s` *(implies `rhai`)* | `DynamicObject`, `K8sObject`, `K8sGeneric`, `K8sRaw`, `K8sDeploy`, `K8sDaemonSet`, `K8sStatefulSet`, `K8sJob` | **no** — manual |
| `http` *(default, implies `rhai`)* | `RestClient` (HTTP) | **no** — manual |
| `s3` *(implies `rhai`)* | `s3_get_yaml`, `s3_list_keys` | **no** — manual |

---

## 1. Core utilities (always available)

Registered by `core_common_rhai_register`, no feature beyond `rhai` itself required.

| Signature | Returns | Notes |
|---|---|---|
| `sha256(text: string)` | `string` | Hex digest |
| `log_debug(text: string)` / `log_info` / `log_warn` / `log_error` | `()` | Forwards to `tracing` |
| `url_encode(text: string)` | `string` | Percent-encodes for use in a query string |
| `get_env(name: string)` | `string` | Empty string if unset |
| `to_decimal(octal: string)` | `int` | Parses the string as base-8; `0` (and a warning) if it isn't valid octal |
| `base64_encode(text: string)` | `string` | Standard base64 |
| `base64_decode(text: string)` | `string` | Errors if the input isn't valid base64/UTF-8 |
| `json_encode(value)` | `string` | Serializes any Rhai value to JSON |
| `json_encode_escape(value)` | `string` | Same, then wraps the result in a second layer of JSON-string escaping (`format!("{:?}", …)`) — for embedding JSON inside JSON |
| `json_decode(text: string)` | dynamic | Errors on invalid JSON |
| `basename(path: string)` | `string` | |
| `dirname(path: string)` | `string` | |

### Script-level helpers (`add_common`)

Also always present — these are Rhai source injected via `add_code`, not native functions:

| Signature | Behavior |
|---|---|
| `assert(cond, message)` | Throws `message` if `cond` is falsy |
| `import_run(name, instance, context, args)` / `import_run(name, instance, context)` / `import_run(name, args)` | `import`s the module `name` and calls its `run(...)` function with the matching arity. If the module or the function doesn't exist, logs a debug message and returns instead of throwing; any other error is rethrown. |
| `import_template(name, instance, context, args)` / `import_template(name, instance, context)` / `import_template(name, args)` | Same as `import_run`, but calls `template(...)` first, falling back to `run(...)` if the module has no `template` function |

---

## 2. `chrono`

| Signature | Returns | Notes |
|---|---|---|
| `date_now()` | `DateTimeHandler` | Current local time |
| `<DateTimeHandler>.format(fmt: string)` | `string` | `fmt` is a [chrono strftime pattern](https://docs.rs/chrono/latest/chrono/format/strftime/index.html) |

## 3. `hashes`

`crc32_hash` needs only `rhai`. The other three need `crypto` too (default on, like `rhai`).

| Signature | Returns | Notes |
|---|---|---|
| `crc32_hash(text: string)` | `int` | CRC-32 |
| `bcrypt_hash(text: string)` | `string` | bcrypt, `DEFAULT_COST` — needs `crypto` |
| `new_argon()` | `Argon` | Generates a fresh random salt for this instance — needs `crypto` |
| `<Argon>.hash(password: string)` | `string` | Argon2 PHC string (salt + hash), using the salt captured at `new_argon()` time — needs `crypto` |

## 4. `key`

Needs `crypto` (default on, like `rhai`) in addition to `rhai` — the whole module is gated by it.

| Signature | Returns | Notes |
|---|---|---|
| `gen_private_key(algo: string)` | `string` | PKCS8 PEM. `algo`: `"rsa"` (4096 bits) or `"ed25519"` (bit count ignored) |
| `gen_private_key(algo: string, bits: int)` | `string` | Same, explicit RSA bit size |

## 5. `semver`

| Signature | Returns | Notes |
|---|---|---|
| `semver_from(text: string)` | `Semver` | Accepts an optional leading `v`, which is remembered and reproduced by `to_string` |
| `<Semver>.inc_major()` | `()` | Bumps major, resets minor/patch, clears prerelease |
| `<Semver>.inc_minor()` | `()` | Bumps minor, resets patch, clears prerelease |
| `<Semver>.inc_patch()` | `()` | Bumps patch — unless already on a prerelease, in which case it just clears the prerelease tag without bumping patch |
| `<Semver>.inc_beta()` | `()` | From stable: bumps patch and sets `beta.1`. From an existing `beta.N`: increments `N` (patch unchanged) |
| `<Semver>.inc_alpha()` | `()` | Same as `inc_beta`, for the `alpha.N` prerelease |
| `a == b`, `a != b`, `a < b`, `a > b`, `a <= b`, `a >= b` | `bool` | Standard semver ordering (prerelease < stable) |
| `to_string(sv)` | `string` | |

## 6. `yaml`

| Signature | Returns | Notes |
|---|---|---|
| `yaml_encode(value)` | `string` | |
| `yaml_decode(text: string)` | dynamic | |
| `yaml_decode_multi(text: string)` | `array` of dynamic | Splits on `---` document markers. **Quirk:** any input of 5 characters or less short-circuits to an empty array without attempting to parse. |

## 7. `glob`

| Signature | Returns | Notes |
|---|---|---|
| `glob(text: string, pattern: string)` | `bool` | Shell-style wildcard match (`wildmatch` — `*` and `?`) |

---

## 8. Feature `fs` — filesystem access

Gated at the function-definition level (`fs_rhai_register` only exists in the binary when this
feature is on), auto-wired into `new_bare`.

> Off by default deliberately: a consumer embedding untrusted or multi-tenant scripts may not
> want to grant filesystem access on the host running them.

| Signature | Returns |
|---|---|
| `file_read(path: string)` | `string` |
| `file_write(path: string, content: string)` | `()` |
| `file_copy(source: string, dest: string)` | `()` |
| `create_dir(path: string)` | `()` — recursive (`create_dir_all`) |
| `read_dir(path: string)` | `array` of path strings |
| `is_file(path: string)` | `bool` |
| `is_dir(path: string)` | `bool` |

## 9. Feature `password`

Off by default so consumers with their own password-generation semantics (e.g. a weight-based
one exposed through a public API) don't get a name collision. Auto-wired when enabled.

| Signature | Returns | Notes |
|---|---|---|
| `gen_password(len: int)` | `string` | At least 1 lowercase, 1 uppercase, 1 digit, 1 symbol |
| `gen_password(len: int, spec: map)` | `string` | `spec` keys `lower`/`upper`/`digits`/`symbols` set the **minimum** count per class (default `1` for any key omitted or negative) |
| `gen_password_alphanum(len: int)` | `string` | Same as `gen_password(len)` but 0 symbols |

Errors (Rhai exception) if the sum of class minimums exceeds `len`, or if every class is
disabled. Symbol set is shell/quoting-safe: `! # % * + - . : = ? @ _`.

## 10. Feature `shell`

| Signature | Returns | Notes |
|---|---|---|
| `shell_run(command: string)` | `int` | Runs via `sh -c`; stdout/stderr are inherited (go straight to the process's own streams); returns the exit code |
| `shell_output(command: string)` | `string` | Runs via `sh -c`, captures stdout; **errors** if the exit code is non-zero *or* if anything was written to stderr (even on exit 0) |

## 11. Feature `oci` (auto-wired)

`Registry` — a client for an OCI/Docker registry, backed by `oci-client`.

| Signature | Returns | Notes |
|---|---|---|
| `new_registry(registry: string, username: string, password: string)` | `Registry` | Anonymous auth if either credential is empty |
| `<Registry>.push_image(source_dir: string, repository: string, tag: string, annotations: map)` | `string` (digest) | Tars + gzips `source_dir` as a single layer and pushes an OCI image manifest |
| `<Registry>.sign_image(repository: string, tag: string, digest: string, key_path: string)` | `()` | No-op if `key_path` is empty; otherwise shells out to the `cosign` binary |
| `<Registry>.list_tags(repository: string)` | `array` of strings | |
| `<Registry>.get_manifest(repository: string, tag: string)` | dynamic | |

Standalone:

| Signature | Returns | Notes |
|---|---|---|
| `get_auth_from_file(path: string, registry: string)` | `map` `{user, pass}` | Reads a Docker `config.json`-style file; empty strings if the registry/entry isn't found |

---

## 12. HTTP client — `http::http_rhai_register` (call manually)

No Cargo feature gates this (`reqwest` is a base dependency), but it is **not** called by
`Script::new_bare` — register it yourself:

```rust
vynil_core::http::http_rhai_register(&mut script.engine);
```

`RestClient` — a reqwest-backed client with optional server CA / mTLS identity.

| Signature | Notes |
|---|---|
| `new_http_client(baseurl: string)` / `new_client(baseurl: string)` | Both construct a `RestClient` |
| `<RestClient>.set_baseurl(base: string)` | |
| `<RestClient>.set_server_ca(pem: string)` | Enables rustls + custom root CA |
| `<RestClient>.set_mtls_cert_key(cert_pem: string, key_pem: string)` | Client identity for mTLS |
| `<RestClient>.headers_reset()` | Clears all headers |
| `<RestClient>.add_header(key: string, value: string)` | |
| `<RestClient>.add_header_json()` | Adds `Content-Type: application/json; charset=utf-8` and `Accept: application/json`, only if not already set |
| `<RestClient>.add_header_bearer(token: string)` | Sets `Authorization: Bearer <token>` |
| `<RestClient>.add_header_basic(username: string, password: string)` | Sets `Authorization: Basic <base64>` |

Request methods — each pair (`x` / `http_x`) is the same function registered under two names:

| Signature | Returns |
|---|---|
| `.head(path)` | `map` `{code, headers}` |
| `.get(path)` / `.http_get(path)` | `map` `{code, headers, body, json}` |
| `.post(path, body)` / `.http_post(path, body)` | `map` `{code, headers, body, json}` |
| `.put(path, body)` / `.http_put(path, body)` | same |
| `.patch(path, body)` / `.http_patch(path, body)` | same |
| `.delete(path)` / `.http_delete(path)` | same |
| `.delete_with_body(path, body)` / `.http_delete_with_body(path, body)` | same |
| `.post_form(path, params: map)` | same — sends `params` as `application/x-www-form-urlencoded`, and drops any `Content-Type` header you'd set manually so the form encoder can set its own |

`body` for `post`/`put`/`patch`/`delete_with_body` may be a Rhai string (sent verbatim) or any
other value (JSON-serialized).

> **Status codes are never treated as errors.** These methods only raise a Rhai exception on a
> transport-level failure (DNS, TLS, connection refused, …). A 404 or 500 comes back as a normal
> `Ok` result with `code` set accordingly — check `.code` yourself. This differs from the plain
> Rust API (`body_get`/`json_get`/…), which does return an `Err` on non-2xx.

Standalone functions:

| Signature | Returns | Notes |
|---|---|---|
| `http_get_yaml(url: string, auth_type: string, credential: string)` | dynamic | One-shot GET-and-parse-as-YAML, own client (not tied to a `RestClient`). `auth_type`: `""`, `"bearer"`, or `"basic"` (in which case `credential` is `"user:pass"`, base64-encoded for you). Errors on non-2xx. |
| `headers_get(headers: array, name: string)` | value or `()` | Case-insensitive lookup into the opaque `headers` array returned as part of a request result (it has no Rhai-visible indexing/iteration of its own) |
| `headers_has(headers: array, name: string)` | `bool` | Case-insensitive presence check |

---

## 13. Feature `k8s` — `k8sgeneric_rhai_register`, `k8sraw_rhai_register`, `k8sworkload_rhai_register` (call manually)

Not auto-wired even with the feature on — register the pieces you need:

```rust
vynil_core::k8s::k8sgeneric_rhai_register(&mut script.engine);
vynil_core::k8s::k8sraw_rhai_register(&mut script.engine);
vynil_core::k8s::k8sworkload_rhai_register(&mut script.engine);
```

### Runtime context (required before any call)

Unlike everything else in this document, the `k8s` module needs process-wide context injected
once at startup, *before* any script runs — these are `OnceLock`s, not part of the `Engine`:

| Function | Purpose |
|---|---|
| `vynil_core::set_client_name(f)` | Identity used as the Server-Side-Apply field manager (and the HTTP User-Agent) |
| `vynil_core::k8s::set_get_client(f)` | Supplies the `kube::Client` |
| `vynil_core::k8s::set_get_labels(f)` | Labels auto-injected into objects created/patched/applied through `K8sGeneric` |
| `vynil_core::k8s::set_get_owner(f)` / `set_get_owner_ns(f)` | Owner reference auto-injected for namespaced objects created in the owner's own namespace |
| `vynil_core::k8s::context_is_wired()` | Returns `true` once all of the above (+ client name) are set |

Calling any `k8s` function before this wiring is done will panic (client-name/client lookups use
`.expect(...)`).

### `DynamicObject`

| Signature | Returns |
|---|---|
| `<DynamicObject>.data` (get) | dynamic — the raw object body |

### `K8sGeneric` — dynamically-discovered resource client

Resolves `name` (kind or plural) against the cluster's API discovery cache to find the right
group/version/kind and scope.

| Signature | Returns | Notes |
|---|---|---|
| `k8s_resource(name: string)` | `K8sGeneric` | Cluster-scoped (or the resource's actual scope if it's not cluster-scoped and no namespace is given) |
| `k8s_resource(name: string, ns: string)` | `K8sGeneric` | Namespaced |
| `k8s_resource(api_version: string, name: string, ns: string)` | `K8sGeneric` | Disambiguates by explicit `group/version`, for resources whose kind/plural exists in multiple API groups |
| `<K8sGeneric>.scope` (get) | `string` | `"cluster"` or `"namespace"` |
| `<K8sGeneric>.exist` (get) | `bool` | Whether discovery actually found the resource |
| `<K8sGeneric>.list()` | dynamic | Full object list |
| `<K8sGeneric>.list(labels: string)` | dynamic | Label-selector-filtered list |
| `<K8sGeneric>.list_meta()` | dynamic | Metadata-only list (cheaper) |
| `<K8sGeneric>.get(name: string)` | dynamic | Full object |
| `<K8sGeneric>.get_meta(name: string)` | dynamic | Metadata-only |
| `<K8sGeneric>.get_obj(name: string)` | `K8sObject` | Handle for `delete`/`wait_*` operations |
| `<K8sGeneric>.delete(name: string)` | `()` | Foreground deletion |
| `<K8sGeneric>.create(data)` | dynamic | Injects labels/owner-reference (see context above) before creating |
| `<K8sGeneric>.replace(name: string, data)` | dynamic | Same injection, then `PUT` |
| `<K8sGeneric>.patch(name: string, data)` | dynamic | Server-Side-Apply, forced |
| `<K8sGeneric>.apply(name: string, data)` | dynamic | Server-Side-Apply, forced; for `Job` objects specifically, an "immutable field" error is swallowed (returns the existing object) if the Job has already completed |
| `update_k8s_crd_cache()` | `()` | Refreshes the process-wide API-discovery cache (60s timeout; keeps the old cache on timeout) |

### `K8sObject` — handle returned by `get_obj`

| Signature | Returns | Notes |
|---|---|---|
| `<K8sObject>.kind` (get) | `string` | Kind as reported by the API server for this object |
| `<K8sObject>.original_kind` (get) | `string` | Kind the `K8sGeneric` was originally resolved for |
| `<K8sObject>.metadata` (get) | dynamic | |
| `<K8sObject>.delete()` | `()` | Foreground deletion |
| `<K8sObject>.wait_deleted(timeout: int)` | `()` | Waits (seconds) until the object's UID is gone |
| `<K8sObject>.wait_condition(condition: string, timeout: int)` | `()` | Waits until `status.conditions[].type == condition && status == "True"` |
| `<K8sObject>.wait_status(prop: string, timeout: int)` | `()` | Waits until `status.<prop>` is boolean `true` |
| `<K8sObject>.wait_status_prop(prop: string, timeout: int)` | `()` | Waits until `status.<prop>` merely exists (non-null) |
| `<K8sObject>.wait_status_string(prop: string, value: string, timeout: int)` | `()` | Waits until `status.<prop> == value` |

### `K8sRaw` — unstructured client for cluster-level endpoints

| Signature | Returns | Notes |
|---|---|---|
| `new_k8s_raw()` | `K8sRaw` | |
| `<K8sRaw>.get_url(url: string)` | dynamic | Raw authenticated GET against the API server, relative to its base URL |
| `<K8sRaw>.get_cluster_version()` | dynamic | `GET /version` |
| `<K8sRaw>.get_api_resources()` | dynamic | `GET /apis`, requesting the `APIGroupDiscoveryList` format |

### Workload helpers — `K8sDeploy` / `K8sDaemonSet` / `K8sStatefulSet` / `K8sJob`

Typed convenience wrappers (not going through discovery) for the four common workload kinds. Each
exposes the same shape:

| Signature | Returns | Notes |
|---|---|---|
| `get_deployment(ns: string, name: string)` → `K8sDeploy` | | |
| `get_deamonset(ns: string, name: string)` → `K8sDaemonSet` | | |
| `get_statefulset(ns: string, name: string)` → `K8sStatefulSet` | | |
| `get_job(ns: string, name: string)` → `K8sJob` | | |
| `<T>.metadata` / `<T>.spec` / `<T>.status` (get) | dynamic | |
| `<K8sDeploy/K8sDaemonSet/K8sStatefulSet>.wait_available(timeout: int)` | `()` | Deploy: `status.conditions[type=Available].status == "True"`. DaemonSet: `desired_number_scheduled == number_available`. StatefulSet: `spec.replicas == status.available_replicas` |
| `<K8sJob>.wait_done(timeout: int)` | `()` | Uses `kube`'s built-in `is_job_completed` condition |

---

## 14. Feature `s3` — `s3::s3_rhai_register` (call manually)

Not auto-wired even with the feature on:

```rust
#[cfg(feature = "s3")]
vynil_core::s3::s3_rhai_register(&mut script.engine);
```

Backed by `object_store`'s S3-compatible client (AWS S3 or any S3-compatible endpoint, e.g. MinIO).

| Signature | Returns | Notes |
|---|---|---|
| `s3_get_yaml(bucket, region, prefix, endpoint, access_key, secret_key, key)` | dynamic | Fetches `<prefix><key>` and parses it as YAML. Empty `access_key` skips explicit credentials (falls back to the store's default credential chain). Empty `endpoint` targets AWS S3; a non-empty `endpoint` is used as a custom endpoint with `allow_http` enabled (for local/self-hosted S3-compatible stores) |
| `s3_list_keys(bucket, region, prefix, endpoint, access_key, secret_key)` | `array` of strings | Lists all object keys under `prefix` |

All parameters are `string`.
