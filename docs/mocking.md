# Mocking

`vynil-core` ships three Rhai test doubles, one per network/cluster-facing module: HTTP, `k8s`,
and OCI. Each mock re-registers (mostly) the same Rhai type and function names as its real
counterpart, backed by in-memory fixtures instead of a live network call or cluster — so a script
written against the real API can run against a mock in a unit test with little or no change.
There is no Handlebars equivalent; mocking only applies to the Rhai side.

See [rhai_helpers.md](rhai_helpers.md) for the real APIs these mirror.

## Shared shape

None of the three mock registration functions are called automatically by
`Script::new_bare` — call them yourself, the same way you would the real `*_rhai_register`
functions, but passing fixture data instead of (or in addition to) an `Engine`:

```rust
let mut script = vynil_core::Script::new_bare(vec![]);

vynil_core::http_mock::httpmock_rhai_register(&mut script.engine, my_http_fixtures);

#[cfg(feature = "k8s")]
vynil_core::k8s_mock::k8s_mock_rhai_register(&mut script.engine, mocks.clone(), created.clone());

#[cfg(feature = "oci")]
vynil_core::oci_mock::oci_mock_rhai_register(&mut script.engine);
```

A given `Engine` should get *either* the real registration *or* the mock one for a given module,
never both — they claim the same Rhai type/function names and the second registration would
shadow the first.

---

## 1. `http_mock` — always compiled, no Cargo feature

Mirrors `RestClient` (see the "HTTP client" section of [rhai_helpers.md](rhai_helpers.md)) under
the **exact same Rhai type name**, `RestClient` — a script that only talks to `RestClient` is
source-compatible between the real and mocked engine.

```rust
pub fn httpmock_rhai_register(engine: &mut Engine, mocks: Vec<HttpMockItem>)
```

Fixtures, supplied up front instead of via context wiring:

```rust
pub struct HttpMockItem {
    pub path: String,
    pub method: HttpMethod,     // Get | Head | Delete | Patch | Post | Put
    pub return_obj: rhai::Map,  // returned verbatim as the call's result
}
```

### Registered surface

| Signature | Behavior |
|---|---|
| `new_http_client(base)` | Constructs a `RestClientMock` carrying a clone of the fixture list |
| `.set_baseurl(base)` | Stored but otherwise unused by matching (matching is by `path` only) |
| `.set_server_ca(pem)`, `.set_mtls_cert_key(cert, key)`, `.headers_reset()`, `.add_header(k, v)`, `.add_header_json()`, `.add_header_bearer(t)`, `.add_header_basic(u, p)` | No-ops — accepted for source compatibility, have no effect on matching or the returned value |
| `.head(path)` | Finds an `HttpMockItem` with `method == Head` and matching `path`; returns its `return_obj` |
| `.get(path)` / `.http_get(path)` | Same, `method == Get` |
| `.post(path, _body)` / `.http_post(path, _body)` | Same, `method == Post`; the body argument is accepted but ignored |
| `.post_form(path, _params)` / `.http_post_form(path, _params)` | Same, `method == Post` — matches the same fixtures as `.post`, since the real client also sends both as an HTTP `POST`; the form map is accepted but ignored |
| `.put(path, _body)` / `.http_put(path, _body)` | Same, `method == Put` |
| `.patch(path, _body)` / `.http_patch(path, _body)` | Same, `method == Patch` |
| `.delete(path)` / `.http_delete(path)` | Same, `method == Delete` |
| `.delete_with_body(path, _body)` / `.http_delete_with_body(path, _body)` | Same, `method == Delete` — matches the same fixtures as `.delete`; the body argument is accepted but ignored |
| `headers_get(headers, name)` / `headers_has(headers, name)` | The exact same standalone functions as the real client (`http::headers_get`/`headers_has`) — they only operate on the `headers` array shape of a result map, so they work unmodified against a mock result as long as your fixture's `return_obj` includes one |

No match → a Rhai error: `"Failed to find <METHOD> <path> in the Mock database"`.

### Gap vs. the real `RestClient`

- No `http_get_yaml` standalone function. Unlike the methods above, the real `http_get_yaml` isn't
  a `RestClient` method — it takes a full URL and fetches it with its own throwaway client,
  independent of any `RestClient`/fixture list. Mocking it meaningfully would need a fixture set
  keyed by URL rather than by `(method, path)` on a client instance, which is a small design
  decision rather than a mechanical addition — not done yet.
- `return_obj` is exactly the map you configured — the real client's `{code, headers, body,
  json}` shape is a convention you must reproduce yourself in the fixture if your script expects
  it (e.g. set `return_obj = #{code: 200, json: #{...}}`).

---

## 2. `k8s_mock` — feature `k8s`

Reuses the *same* `register_k8s_object!` / `register_k8s_generic!` / `register_k8s_raw!` macros
that `k8s.rs` uses for the real client, instantiated against mock structs but registered under
the **exact same Rhai type names**: `K8sObject`, `K8sGeneric`, `K8sRaw`, `K8sDeploy`,
`K8sDaemonSet`, `K8sStatefulSet`, `K8sJob`. A script written against the real `k8s` module runs
unmodified against the mock.

```rust
pub fn k8s_mock_rhai_register(
    engine: &mut Engine,
    mocks: Arc<Mutex<Vec<Dynamic>>>,    // fixture objects
    created: Arc<Mutex<Vec<Dynamic>>>,  // write-log for assertions
)
```

Unlike the real module, **no runtime context wiring is required**: `k8s_mock` never touches
`set_client_name`/`set_get_client`/the discovery cache, so it's safe to use in a plain unit test
with no cluster, no `set_client_name` call, nothing.

### Fixtures

`mocks` is a flat list of full object maps, each expected to carry `kind`, `metadata.name`, and
(for namespaced kinds) `metadata.namespace` — the same shape a real `kubectl get -o json` object
would have. `K8sGeneric`/`K8sObject`/workload lookups filter this list by kind + name(+
namespace).

### Behavior notes

| Operation | Mock behavior |
|---|---|
| `k8s_resource(...)`, `get_deployment`/`get_deamonset`/`get_statefulset`/`get_job` | Constructs the mock handle immediately (no discovery round-trip); doesn't fail if nothing matches yet — failure happens on the subsequent `get`/lookup |
| `.get(name)` / `.get_meta(name)` / `.list()` / `.list_labels(...)` / `.list_meta()` | Searches `mocks` by kind (+ name/namespace where relevant) |
| `.create(data)` / `.replace(name, data)` / `.patch(name, data)` / `.apply(name, data)` | Deep-merges `data` onto any existing matching entry (by kind + name + namespace) and appends the merged result to `created` — inspect `created` after running a script to assert what would have been sent to the cluster |
| `.delete(name)`, `.wait_*(...)` | No-ops that always succeed |
| `<K8sObject>.wait_condition`/`wait_status`/`wait_status_prop`/`wait_status_string`/`wait_for` | Always immediately satisfied |
| `<K8sGeneric>.exist` | Always `true` |
| `update_k8s_crd_cache()` | Overridden to a no-op — the real macro-registered version would try to reach a live cluster to refresh discovery, which would panic without a wired client |
| `<K8sRaw>.get_url`/`get_cluster_version`/`get_api_resources` | Always return `{}` |
| `K8sDeploy`/`K8sDaemonSet`/`K8sStatefulSet`/`K8sJob` | All backed by one shared `K8sWorkloadMock` struct; `.metadata`/`.spec`/`.status` read the matching sub-key straight out of the fixture object (`Dynamic::UNIT` if absent); `wait_available`/`wait_done` are no-ops |

---

## 3. `oci_mock` — feature `oci`

```rust
pub fn oci_mock_rhai_register(engine: &mut Engine)
```

No fixtures — every method returns a fixed canned value regardless of its arguments.

> **Caveat:** this is the one mock that does **not** reuse the real type name. The real client
> registers as `Registry`; `oci_mock` registers as `OciRegistryMock`. The constructor function
> name (`new_registry`) is identical, so scripts that only do
> `let r = new_registry(reg, user, pass);` and call methods on `r` are unaffected — but any script
> branching on the type name explicitly would need to account for this.

| Signature | Always returns |
|---|---|
| `new_registry(registry, user, pass)` | `OciRegistryMock` (all three arguments ignored) |
| `.list_tags(repository)` | `[]` |
| `.get_manifest(repository, tag)` | `#{ "annotations": #{} }` |
| `.push_image(dir, repository, tag, annotations)` | `"sha256:mock-digest-for-testing"` |
| `.sign_image(repository, tag, digest, key_path)` | `()` (always succeeds, never actually shells out to `cosign`) |
