//! Kubernetes handlers.
//!
//! Feature `k8s` only (implies `rhai`). Provides `K8sGeneric` (CRUD via SSA + discovery cache),
//! `K8sObject` (per-object helpers), `K8sRaw` (raw API discovery) and typed workload helpers
//! (`K8sDeploy`, `K8sJob`, …). The API is Rhai-facing throughout (methods return [`crate::RhaiRes`]).
//!
//! The discovery cache is wired via `OnceLock` function pointers injected by the consumer (see
//! `context_is_wired` / `set_get_client` …) — the crate itself does not assume a kubeconfig.

use std::sync::{LazyLock, OnceLock};

use crate::{Error, Result, RhaiRes, rhai_err, rhai_err_str};
use k8s_openapi::api::{
    apps::v1::{DaemonSet, Deployment, StatefulSet},
    batch::v1::Job,
};
use kube::{
    Client, ResourceExt,
    api::{
        Api, DeleteParams, DynamicObject, ListParams, ObjectList, PartialObjectMeta, Patch, PatchParams,
        PostParams,
    },
    discovery::{ApiCapabilities, ApiResource, Discovery, Scope},
    runtime::wait::{Condition, await_condition, conditions},
};
use rhai::{Dynamic, Engine, serde::to_dynamic};
use serde_json::json;
use tokio::sync::RwLock;

// ── Context function pointers (initialized by common at startup) ─────────────

/// Injected kube [`Client`] factory, set by the consumer via [`set_get_client`].
pub static GET_CLIENT: OnceLock<Box<dyn Fn() -> Client + Send + Sync>> = OnceLock::new();
/// Injected common-labels accessor, set by the consumer via [`set_get_labels`].
pub static GET_LABELS: OnceLock<Box<dyn Fn() -> Option<serde_json::Value> + Send + Sync>> = OnceLock::new();
/// Injected owner-reference accessor, set by the consumer via [`set_get_owner`].
pub static GET_OWNER: OnceLock<Box<dyn Fn() -> Option<serde_json::Value> + Send + Sync>> = OnceLock::new();
/// Injected owner-namespace accessor, set by the consumer via [`set_get_owner_ns`].
pub static GET_OWNER_NS: OnceLock<Box<dyn Fn() -> Option<String> + Send + Sync>> = OnceLock::new();

/// Injects the kube-client factory; first call wins, later calls are ignored.
pub fn set_get_client(f: Box<dyn Fn() -> Client + Send + Sync>) {
    GET_CLIENT.set(f).ok();
}
/// Injects the common-labels accessor; first call wins, later calls are ignored.
pub fn set_get_labels(f: Box<dyn Fn() -> Option<serde_json::Value> + Send + Sync>) {
    GET_LABELS.set(f).ok();
}
/// Injects the owner-reference accessor; first call wins, later calls are ignored.
pub fn set_get_owner(f: Box<dyn Fn() -> Option<serde_json::Value> + Send + Sync>) {
    GET_OWNER.set(f).ok();
}
/// Injects the owner-namespace accessor; first call wins, later calls are ignored.
pub fn set_get_owner_ns(f: Box<dyn Fn() -> Option<String> + Send + Sync>) {
    GET_OWNER_NS.set(f).ok();
}

/// True if the client name (`crate::set_client_name`) and the 4 context accessors have been
/// injected (see `common::context::wire_core_k8s`).
pub fn context_is_wired() -> bool {
    GET_CLIENT.get().is_some()
        && crate::client_name_is_set()
        && GET_LABELS.get().is_some()
        && GET_OWNER.get().is_some()
        && GET_OWNER_NS.get().is_some()
}

fn call_get_labels() -> Option<serde_json::Value> {
    GET_LABELS.get().and_then(|f| f())
}
fn call_get_owner() -> Option<serde_json::Value> {
    GET_OWNER.get().and_then(|f| f())
}
fn call_get_owner_ns() -> Option<String> {
    GET_OWNER_NS.get().and_then(|f| f())
}

/// Builds the shared kube client from the injected [`GET_CLIENT`] factory.
///
/// # Panics
///
/// Panics if the k8s context was not wired (see [`context_is_wired`]).
#[allow(clippy::expect_used)] // panic by design: no error channel in the CLIENT/RAW_CLIENT static initializers (vyvil-core.sdd)
fn build_client() -> Client {
    let f = GET_CLIENT.get().expect("k8s context not initialized");
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(async move { f() }))
}

/// Wait timeout as a [`std::time::Duration`]; a negative timeout clamps to zero (immediate timeout).
fn timeout_duration(timeout: i64) -> std::time::Duration {
    std::time::Duration::from_secs(u64::try_from(timeout).unwrap_or(0))
}

// ── k8sgeneric ───────────────────────────────────────────────────────────────

/// Shared kube client, built from the injected factory on first access.
///
/// Panics on first access if the k8s context was not wired (see [`context_is_wired`]).
pub static CLIENT: LazyLock<Client> = LazyLock::new(build_client);

fn aggregated_apiservice_group(spec: &serde_json::Value) -> Option<String> {
    spec.get("service").filter(|s| !s.is_null())?;
    spec.get("group")
        .and_then(|g| g.as_str())
        .filter(|g| !g.is_empty())
        .map(std::string::ToString::to_string)
}

async fn excluded_apiservice_groups() -> Vec<String> {
    let ar = ApiResource {
        group: "apiregistration.k8s.io".to_string(),
        version: "v1".to_string(),
        api_version: "apiregistration.k8s.io/v1".to_string(),
        kind: "APIService".to_string(),
        plural: "apiservices".to_string(),
    };
    let api: Api<DynamicObject> = Api::all_with(CLIENT.clone(), &ar);
    match api.list(&ListParams::default()).await {
        Ok(list) => list
            .items
            .iter()
            .filter_map(|obj| aggregated_apiservice_group(obj.data.get("spec")?))
            .collect(),
        Err(e) => {
            tracing::warn!("E_DISCOVERY_WARN: cannot list APIServices ({e}), proceeding without exclusions");
            vec![]
        }
    }
}

async fn async_populate_cache() -> Result<Discovery> {
    let excluded = excluded_apiservice_groups().await;
    let excluded_refs: Vec<&str> = excluded.iter().map(std::string::String::as_str).collect();
    Discovery::new(CLIENT.clone())
        .exclude(&excluded_refs)
        .run()
        .await
        .map_err(Error::KubeError)
}

#[allow(clippy::expect_used)] // panic by design: no error channel in the CACHE static initializer (vyvil-core.sdd)
fn populate_cache() -> Discovery {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current()
            .block_on(async_populate_cache())
            .expect("create discovery (excluding api-services)")
    })
}

/// Discovery cache, populated from the cluster on first access.
///
/// Panics on first access if discovery fails; use [`update_cache`] for a graceful refresh.
pub static CACHE: LazyLock<RwLock<Discovery>> = LazyLock::new(|| RwLock::new(populate_cache()));

/// Refreshes the discovery cache from the cluster, keeping the previous one on timeout or failure.
///
/// Exposed to Rhai as `update_k8s_crd_cache`.
pub fn update_cache() {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async move {
            match tokio::time::timeout(std::time::Duration::from_mins(1), async_populate_cache()).await {
                Ok(Ok(discovery)) => {
                    *CACHE.write().await = discovery;
                }
                Ok(Err(e)) => {
                    tracing::warn!("E_DISCOVERY_WARN: update_k8s_crd_cache failed ({e}), keeping old cache");
                }
                Err(_) => {
                    tracing::warn!(
                        "E_DISCOVERY_TIMEOUT: update_k8s_crd_cache exceeded 30s, keeping old cache"
                    );
                }
            }
        });
    });
}

/// A single live Kubernetes object: its API handle plus the metadata fetched at creation.
#[derive(Clone, Debug)]
pub struct K8sObject {
    /// API handle used for every operation on this object.
    pub api: Api<DynamicObject>,
    /// Partial metadata fetched when the object was obtained (via `get_obj`).
    pub obj: PartialObjectMeta,
    /// Kind recorded on the [`K8sGeneric`] this object came from.
    pub kind: String,
}
impl K8sObject {
    /// Deletes the object with foreground propagation.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::KubeError`] if the API call fails.
    pub fn rhai_delete(&mut self) -> RhaiRes<()> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                self.api
                    .delete(&self.obj.name_any(), &DeleteParams::foreground())
                    .await
                    .map_err(Error::KubeError)
                    .map(|_| ())
            })
        })
        .map_err(rhai_err)
    }

    /// Waits until this object's uid is observed as deleted, up to `timeout` seconds.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if the object has no uid, if `timeout` elapses ([`Error::Elapsed`])
    /// or if the watch fails ([`Error::KubeWaitError`]).
    pub fn rhai_wait_deleted(&mut self, timeout: i64) -> RhaiRes<()> {
        let name = self.obj.name_any();
        let uid = self
            .obj
            .uid()
            .ok_or_else(|| rhai_err_str(format!("cannot wait for deletion of {name}: uid is missing")))?;
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let cond = await_condition(self.api.clone(), &name, conditions::is_deleted(&uid));
                tokio::time::timeout(timeout_duration(timeout), cond)
                    .await
                    .map_err(Error::Elapsed)
            })
        })
        .map_err(rhai_err)?
        .map_err(|e| rhai_err(Error::KubeWaitError(e)))
        .map(|_| ())
    }

    /// This object's metadata rendered as a Rhai value.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::SerializationError`] if the metadata cannot be
    /// converted to a Rhai value.
    pub fn get_metadata(&mut self) -> RhaiRes<Dynamic> {
        let v = serde_json::to_value(self.obj.metadata.clone())
            .map_err(|e| rhai_err(Error::SerializationError(e)))?;
        to_dynamic(v)
    }

    /// Runtime kind read from the object's own type fields (empty when absent).
    pub fn get_kind(&mut self) -> String {
        if let Some(t) = self.obj.types.clone() {
            t.kind
        } else {
            String::new()
        }
    }

    /// Kind recorded on the [`K8sGeneric`] this object was fetched from.
    pub fn original_kind(&mut self) -> String {
        self.kind.clone()
    }

    /// Condition matching when `status.conditions` contains `cond` with `status: "True"`.
    #[must_use]
    pub fn is_condition(cond: String) -> impl Condition<DynamicObject> {
        move |obj: Option<&DynamicObject>| {
            let Some(conditions) = obj
                .and_then(|o| o.data.get("status"))
                .and_then(|s| s.get("conditions"))
                .and_then(|c| c.as_array())
            else {
                return false;
            };
            conditions.iter().any(|c| {
                c.get("type").and_then(|t| t.as_str()) == Some(cond.as_str())
                    && c.get("status").and_then(|s| s.as_str()) == Some("True")
            })
        }
    }

    /// Waits up to `timeout` seconds for `condition` to become `True` on this object.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if `timeout` elapses ([`Error::Elapsed`]) or the watch fails
    /// ([`Error::KubeWaitError`]).
    pub fn wait_condition(&mut self, condition: String, timeout: i64) -> RhaiRes<()> {
        let name = self.obj.name_any();
        let cond = await_condition(self.api.clone(), &name, Self::is_condition(condition));
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                tokio::time::timeout(timeout_duration(timeout), cond)
                    .await
                    .map_err(Error::Elapsed)
            })
        })
        .map_err(rhai_err)?
        .map_err(Error::KubeWaitError)
        .map_err(rhai_err)?;
        Ok(())
    }

    /// Condition matching when `status.<prop>` is the boolean `true`.
    #[must_use]
    pub fn is_status(prop: String) -> impl Condition<DynamicObject> {
        move |obj: Option<&DynamicObject>| {
            obj.and_then(|o| o.data.get("status"))
                .and_then(|s| s.get(prop.as_str()))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        }
    }

    /// Condition matching when `status.<prop>` is present and not null.
    #[must_use]
    pub fn have_status(prop: String) -> impl Condition<DynamicObject> {
        move |obj: Option<&DynamicObject>| {
            obj.and_then(|o| o.data.get("status"))
                .and_then(|s| s.get(prop.as_str()))
                .is_some_and(|v| !v.is_null())
        }
    }

    /// Condition matching when `status.<prop>` equals the string `value`.
    #[must_use]
    pub fn have_status_value(prop: String, value: String) -> impl Condition<DynamicObject> {
        move |obj: Option<&DynamicObject>| {
            obj.and_then(|o| o.data.get("status"))
                .and_then(|s| s.get(prop.as_str()))
                .and_then(|v| v.as_str())
                .is_some_and(|v| v == value.as_str())
        }
    }

    /// Waits up to `timeout` seconds for `status.<prop>` to become the boolean `true`.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if `timeout` elapses ([`Error::Elapsed`]) or the watch fails
    /// ([`Error::KubeWaitError`]).
    pub fn wait_status(&mut self, prop: String, timeout: i64) -> RhaiRes<()> {
        let name = self.obj.name_any();
        tracing::debug!("wait_status({}) for {} {}", &prop, self.kind, name);
        let cond = await_condition(self.api.clone(), &name, Self::is_status(prop));
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                tokio::time::timeout(timeout_duration(timeout), cond)
                    .await
                    .map_err(Error::Elapsed)
            })
        })
        .map_err(rhai_err)?
        .map_err(Error::KubeWaitError)
        .map_err(rhai_err)?;
        Ok(())
    }

    /// Waits up to `timeout` seconds for `status.<prop>` to appear (non-null).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if `timeout` elapses ([`Error::Elapsed`]) or the watch fails
    /// ([`Error::KubeWaitError`]).
    pub fn wait_status_prop(&mut self, prop: String, timeout: i64) -> RhaiRes<()> {
        let name = self.obj.name_any();
        tracing::debug!("wait_status({}) for {} {}", &prop, self.kind, name);
        let cond = await_condition(self.api.clone(), &name, Self::have_status(prop));
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                tokio::time::timeout(timeout_duration(timeout), cond)
                    .await
                    .map_err(Error::Elapsed)
            })
        })
        .map_err(rhai_err)?
        .map_err(Error::KubeWaitError)
        .map_err(rhai_err)?;
        Ok(())
    }

    /// Waits up to `timeout` seconds for `status.<prop>` to equal the string `value`.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if `timeout` elapses ([`Error::Elapsed`]) or the watch fails
    /// ([`Error::KubeWaitError`]).
    pub fn wait_status_string(&mut self, prop: String, value: String, timeout: i64) -> RhaiRes<()> {
        let name = self.obj.name_any();
        tracing::debug!("wait_status({}) for {} {}", &prop, self.kind, name);
        let cond = await_condition(self.api.clone(), &name, Self::have_status_value(prop, value));
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                tokio::time::timeout(timeout_duration(timeout), cond)
                    .await
                    .map_err(Error::Elapsed)
            })
        })
        .map_err(rhai_err)?
        .map_err(Error::KubeWaitError)
        .map_err(rhai_err)?;
        Ok(())
    }

    /// Wait until a caller-supplied Rhai predicate returns `true` for this object.
    ///
    /// The predicate is called with the object rendered as a map (`metadata` / `spec` /
    /// `status` / …, exactly the shape `<K8sGeneric>.get(name)` returns) and must return a
    /// boolean. It is re-evaluated on every watch event until it returns `true` or `timeout`
    /// seconds elapse. Unlike `wait_status*`, the predicate can inspect arbitrarily nested
    /// fields (`obj.status.ceph.versions.overall.len() == 1`, …). A predicate that raises an
    /// error aborts the wait with that error rather than silently counting as `false`.
    ///
    /// # Errors
    ///
    /// Returns the predicate's own Rhai error if it raises, otherwise a Rhai error if `timeout`
    /// elapses ([`Error::Elapsed`]) or the watch fails ([`Error::KubeWaitError`]).
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn wait_for(
        ctx: rhai::NativeCallContext,
        obj: &mut K8sObject,
        predicate: rhai::FnPtr,
        timeout: i64,
    ) -> RhaiRes<()> {
        let name = obj.obj.name_any();
        let api = obj.api.clone();
        tracing::debug!("wait_for({}) for {} {}", predicate.fn_name(), obj.kind, name);
        // `await_condition` only lets the closure return `bool`; stash the first predicate
        // error here so we can surface it instead of a misleading timeout.
        let pred_err: std::cell::RefCell<Option<Box<rhai::EvalAltResult>>> = std::cell::RefCell::new(None);
        let cond = |o: Option<&DynamicObject>| -> bool {
            let Some(dynobj) = o else { return false };
            let value = match to_dynamic(dynobj) {
                Ok(v) => v,
                Err(e) => {
                    pred_err.borrow_mut().get_or_insert(e);
                    return false;
                }
            };
            match predicate.call_within_context::<Dynamic>(&ctx, (value,)) {
                Ok(r) => r.as_bool().unwrap_or(false),
                Err(e) => {
                    pred_err.borrow_mut().get_or_insert(e);
                    false
                }
            }
        };
        let outcome = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                tokio::time::timeout(timeout_duration(timeout), await_condition(api, &name, cond))
                    .await
                    .map_err(Error::Elapsed)
            })
        });
        if let Some(e) = pred_err.into_inner() {
            return Err(e);
        }
        outcome
            .map_err(rhai_err)?
            .map_err(Error::KubeWaitError)
            .map_err(rhai_err)?;
        Ok(())
    }
}

/// Generic resource handle resolved from the discovery cache (`K8sGeneric` in Rhai).
#[derive(Clone, Debug)]
pub struct K8sGeneric {
    /// Resolved API handle, `None` when the kind/plural was not found in the discovery cache.
    pub api: Option<Api<DynamicObject>>,
    /// Namespace requested at construction, if any.
    pub ns: Option<String>,
    /// Discovery scope of the resolved resource.
    pub scope: Scope,
    /// Resolved kind (empty when unresolved).
    pub kind: String,
}

impl K8sGeneric {
    /// Resolves a resource by kind or plural (case-insensitive) from the discovery cache.
    ///
    /// On ambiguity the lexicographically smallest group wins (the core group in practice).
    /// Returns a handle with `api: None` when nothing matches.
    #[must_use]
    pub fn new(name: &str, ns: Option<String>) -> K8sGeneric {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                if let Some((res, cap)) = CACHE
                    .read()
                    .await
                    .groups()
                    .flat_map(|group| {
                        group
                            .resources_by_stability()
                            .into_iter()
                            .map(move |res: (ApiResource, ApiCapabilities)| (group, res))
                    })
                    .filter(|(_, (res, _))| {
                        name.eq_ignore_ascii_case(&res.kind) || name.eq_ignore_ascii_case(&res.plural)
                    })
                    .min_by_key(|(group, _res)| group.name())
                    .map(|(_, res)| res)
                {
                    tracing::debug!("K8sGeneric::new Using {}/{}/{}", res.group, res.version, res.kind);
                    let api = if cap.scope == Scope::Cluster || ns.is_none() {
                        Api::all_with(CLIENT.clone(), &res)
                    } else if let Some(namespace) = ns.clone() {
                        Api::namespaced_with(CLIENT.clone(), &namespace, &res)
                    } else {
                        Api::default_namespaced_with(CLIENT.clone(), &res)
                    };
                    K8sGeneric {
                        api: Some(api),
                        ns,
                        scope: cap.scope,
                        kind: res.kind,
                    }
                } else {
                    K8sGeneric {
                        api: None,
                        ns: None,
                        scope: Scope::Cluster,
                        kind: String::new(),
                    }
                }
            })
        })
    }

    /// Resolves a resource by api group, version and kind/plural from the discovery cache.
    ///
    /// Returns a handle with `api: None` when nothing matches.
    #[must_use]
    pub fn new_api_version(api_group: &str, version: &str, name: &str, ns: Option<String>) -> K8sGeneric {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                if let Some((res, cap)) = CACHE
                    .read()
                    .await
                    .groups()
                    .flat_map(|group| {
                        group
                            .resources_by_stability()
                            .into_iter()
                            .map(move |res: (ApiResource, ApiCapabilities)| (group, res))
                    })
                    .filter(|(group, (res, _))| {
                        group.name() == api_group
                            && res.version == version
                            && (name.eq_ignore_ascii_case(&res.kind)
                                || name.eq_ignore_ascii_case(&res.plural))
                    })
                    .min_by_key(|(group, _res)| group.name())
                    .map(|(_, res)| res)
                {
                    tracing::debug!(
                        "K8sGeneric::new_api_version Using {}/{}/{}",
                        res.group,
                        res.version,
                        res.kind
                    );
                    let api = if cap.scope == Scope::Cluster || ns.is_none() {
                        Api::all_with(CLIENT.clone(), &res)
                    } else if let Some(namespace) = ns.clone() {
                        Api::namespaced_with(CLIENT.clone(), &namespace, &res)
                    } else {
                        Api::default_namespaced_with(CLIENT.clone(), &res)
                    };
                    K8sGeneric {
                        api: Some(api),
                        ns,
                        scope: cap.scope,
                        kind: res.kind,
                    }
                } else {
                    K8sGeneric {
                        api: None,
                        ns: None,
                        scope: Scope::Cluster,
                        kind: String::new(),
                    }
                }
            })
        })
    }

    /// Rhai constructor `k8s_resource(name, ns)`: namespaced [`Self::new`].
    #[must_use]
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn new_ns(name: String, ns: String) -> K8sGeneric {
        K8sGeneric::new(name.as_str(), Some(ns))
    }

    /// Rhai constructor `k8s_resource(name)`: cluster-wide [`Self::new`] lookup.
    #[must_use]
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn new_global(name: String) -> K8sGeneric {
        K8sGeneric::new(name.as_str(), None)
    }

    /// Rhai constructor `k8s_resource(api_version, name, ns)`: splits `api_version` on `/`
    /// and delegates to [`Self::new_api_version`] (or [`Self::new`] when no group is present).
    #[must_use]
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn new_group_ns(api_version: String, name: String, ns: String) -> K8sGeneric {
        let arr = api_version.split('/').collect::<Vec<&str>>();
        if arr.len() > 1 {
            K8sGeneric::new_api_version(arr[0], arr[1], name.as_str(), Some(ns))
        } else {
            K8sGeneric::new(name.as_str(), Some(ns))
        }
    }

    /// Discovery scope as `"cluster"` or `"namespace"` for Rhai.
    pub fn rhai_get_scope(&mut self) -> String {
        if self.scope == Scope::Cluster {
            "cluster".to_string()
        } else {
            "namespace".to_string()
        }
    }

    /// True when the resource was resolved at construction ([`Self::api`] is set).
    #[must_use]
    pub fn exist(&self) -> bool {
        self.api.is_some()
    }

    /// [`Self::exist`] as a Rhai value.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if the boolean cannot be converted to a Rhai value (never in
    /// practice, kept for the `RhaiRes` shape).
    pub fn rhai_exist(&mut self) -> RhaiRes<Dynamic> {
        to_dynamic(self.api.is_some())
    }

    /// Lists all objects of this resource.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMethod`] if the resource was not resolved, or
    /// [`Error::KubeError`] if the API call fails.
    pub fn list(&self) -> Result<ObjectList<DynamicObject>> {
        if let Some(api) = self.api.clone() {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(async move { api.list(&ListParams::default()).await.map_err(Error::KubeError) })
            })
        } else {
            Err(Error::UnsupportedMethod)
        }
    }

    /// [`Self::list`] as a Rhai value.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping the [`Self::list`] errors or
    /// [`Error::SerializationError`].
    pub fn rhai_list(&mut self) -> RhaiRes<Dynamic> {
        let res = self.list().map_err(rhai_err)?;
        let v = serde_json::to_value(res).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        to_dynamic(v)
    }

    /// Lists objects of this resource matching a label selector (invalid selectors fail at
    /// request time).
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMethod`] if the resource was not resolved, or
    /// [`Error::KubeError`] if the API call fails.
    pub fn list_labels(&self, labels: String) -> Result<ObjectList<DynamicObject>> {
        if let Some(api) = self.api.clone() {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    let mut lp = ListParams::default();
                    lp = lp.labels(&labels);
                    api.list(&lp).await.map_err(Error::KubeError)
                })
            })
        } else {
            Err(Error::UnsupportedMethod)
        }
    }

    /// [`Self::list_labels`] as a Rhai value.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping the [`Self::list_labels`] errors or
    /// [`Error::SerializationError`].
    pub fn rhai_list_labels(&mut self, labels: String) -> RhaiRes<Dynamic> {
        let res = self.list_labels(labels).map_err(rhai_err)?;
        let v = serde_json::to_value(res).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        to_dynamic(v)
    }

    /// Lists only the object metadata of this resource (cheaper full listing).
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMethod`] if the resource was not resolved, or
    /// [`Error::KubeError`] if the API call fails.
    pub fn list_meta(&self) -> Result<ObjectList<PartialObjectMeta>> {
        if let Some(api) = self.api.clone() {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    api.list_metadata(&ListParams::default())
                        .await
                        .map_err(Error::KubeError)
                })
            })
        } else {
            Err(Error::UnsupportedMethod)
        }
    }

    /// [`Self::list_meta`] as a Rhai value.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping the [`Self::list_meta`] errors or
    /// [`Error::SerializationError`].
    pub fn rhai_list_meta(&mut self) -> RhaiRes<Dynamic> {
        let res = self.list_meta().map_err(rhai_err)?;
        let v = serde_json::to_value(res).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        to_dynamic(v)
    }

    /// Gets a single object by name.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMethod`] if the resource was not resolved, or
    /// [`Error::KubeError`] if the API call fails.
    pub fn get(&self, name: &str) -> Result<DynamicObject> {
        if let Some(api) = self.api.clone() {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(async move { api.get(name).await.map_err(Error::KubeError) })
            })
        } else {
            Err(Error::UnsupportedMethod)
        }
    }

    /// [`Self::get`] as a Rhai value.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping the [`Self::get`] errors or [`Error::SerializationError`].
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn rhai_get(&mut self, name: String) -> RhaiRes<Dynamic> {
        let res = self.get(&name).map_err(rhai_err)?;
        let v = serde_json::to_value(res).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        to_dynamic(v)
    }

    /// Gets a single object's metadata by name.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMethod`] if the resource was not resolved, or
    /// [`Error::KubeError`] if the API call fails.
    pub fn get_meta(&self, name: &str) -> Result<PartialObjectMeta> {
        if let Some(api) = self.api.clone() {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(async move { api.get_metadata(name).await.map_err(Error::KubeError) })
            })
        } else {
            Err(Error::UnsupportedMethod)
        }
    }

    /// [`Self::get_meta`] as a Rhai value.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping the [`Self::get_meta`] errors or
    /// [`Error::SerializationError`].
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn rhai_get_meta(&mut self, name: String) -> RhaiRes<Dynamic> {
        let res = self.get_meta(&name).map_err(rhai_err)?;
        let v = serde_json::to_value(res).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        to_dynamic(v)
    }

    /// Fetches the object's metadata and wraps it in a [`K8sObject`] bound to this resource.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::UnsupportedMethod`] (resource unresolved) or the
    /// [`Self::get_meta`] errors.
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn rhai_get_obj(&mut self, name: String) -> RhaiRes<K8sObject> {
        let Some(api) = self.api.clone() else {
            return Err(rhai_err(Error::UnsupportedMethod));
        };
        let res = self.get_meta(&name).map_err(rhai_err)?;
        Ok(K8sObject {
            api,
            obj: res,
            kind: self.kind.clone(),
        })
    }

    /// Deletes an object by name with foreground propagation.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMethod`] if the resource was not resolved, or
    /// [`Error::KubeError`] if the API call fails.
    pub fn delete(&self, name: &str) -> Result<()> {
        if let Some(api) = self.api.clone() {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    api.delete(name, &DeleteParams::foreground())
                        .await
                        .map_err(Error::KubeError)
                        .map(|_| ())
                })
            })
        } else {
            Err(Error::UnsupportedMethod)
        }
    }

    /// [`Self::delete`] variant for Rhai.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping the [`Self::delete`] errors.
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn rhai_delete(&mut self, name: String) -> RhaiRes<()> {
        self.delete(&name).map_err(rhai_err)
    }

    fn inject_labels_and_owner(
        &self,
        handle: serde_json::Map<String, serde_json::Value>,
    ) -> serde_json::Map<String, serde_json::Value> {
        prepare_handle(
            handle,
            call_get_labels(),
            call_get_owner(),
            call_get_owner_ns(),
            self.ns.clone(),
            self.scope == Scope::Namespaced,
        )
    }
}

// ── Pure helper — testable without a live K8s client ─────────────────────────

/// Normalizes a handle before create/replace/patch/apply: guarantees an object `metadata`,
/// injects missing common labels (never overriding existing ones) and, for a namespaced
/// resource in the owner's namespace, appends the owner reference.
fn prepare_handle(
    mut handle: serde_json::Map<String, serde_json::Value>,
    labels: Option<serde_json::Value>,
    owner: Option<serde_json::Value>,
    owner_ns: Option<String>,
    my_ns: Option<String>,
    is_namespaced: bool,
) -> serde_json::Map<String, serde_json::Value> {
    if !handle.get("metadata").is_some_and(serde_json::Value::is_object) {
        handle.insert("metadata".to_string(), json!({}));
    }
    let Some(metadata) = handle
        .get_mut("metadata")
        .and_then(serde_json::Value::as_object_mut)
    else {
        // unreachable: the branch above just forced metadata to be an object
        return handle;
    };
    if let Some(labels) = labels {
        if !metadata.get("labels").is_some_and(serde_json::Value::is_object) {
            metadata.insert("labels".to_string(), json!({}));
        }
        if let Some(label_map) = labels.as_object()
            && let Some(existing) = metadata
                .get_mut("labels")
                .and_then(serde_json::Value::as_object_mut)
        {
            for (k, v) in label_map {
                if !existing.contains_key(k.as_str()) {
                    existing.insert(k.clone(), v.clone());
                }
            }
        }
    }
    if is_namespaced
        && let Some(owner) = owner
        && let Some(ns) = owner_ns
        && let Some(mine) = my_ns
        && ns == mine
    {
        let references = metadata
            .entry("ownerReferences".to_string())
            .or_insert_with(|| json!([]));
        match references {
            serde_json::Value::Array(items) => items.push(owner),
            // malformed (non-array) references are replaced rather than panicking
            other => *other = vec![owner].into(),
        }
    }
    handle
}

impl K8sGeneric {
    /// Creates a resource from a handle map (labels/owner refs injected first).
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMethod`] if the resource was not resolved,
    /// [`Error::SerializationError`] if the handle is not a valid object, or
    /// [`Error::KubeError`] if the API call fails.
    pub fn create(&self, data: serde_json::Map<String, serde_json::Value>) -> Result<DynamicObject> {
        if let Some(api) = self.api.clone() {
            let handle = self.inject_labels_and_owner(data);
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    match serde_json::from_value(handle.into()) {
                        Ok(obj) => api
                            .create(&PostParams::default(), &obj)
                            .await
                            .map_err(Error::KubeError),
                        Err(e) => Err(Error::SerializationError(e)),
                    }
                })
            })
        } else {
            Err(Error::UnsupportedMethod)
        }
    }

    /// [`Self::create`] variant for Rhai, taking the handle as a Rhai map.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if the map cannot be deserialized, or wrapping the
    /// [`Self::create`] errors and [`Error::SerializationError`].
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn rhai_create(&mut self, data: rhai::Dynamic) -> RhaiRes<Dynamic> {
        let data = rhai::serde::from_dynamic(&data)?;
        let res = self.create(data).map_err(|e: Error| rhai_err(e))?;
        let v = serde_json::to_value(res).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        to_dynamic(v)
    }

    /// Replaces (full update) a named resource from a handle map.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMethod`] if the resource was not resolved,
    /// [`Error::SerializationError`] if the handle is not a valid object, or
    /// [`Error::KubeError`] if the API call fails.
    pub fn replace(
        &self,
        name: &str,
        data: serde_json::Map<String, serde_json::Value>,
    ) -> Result<DynamicObject> {
        if let Some(api) = self.api.clone() {
            let handle = self.inject_labels_and_owner(data);
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    match serde_json::from_value(handle.into()) {
                        Ok(obj) => api
                            .replace(name, &PostParams::default(), &obj)
                            .await
                            .map_err(Error::KubeError),
                        Err(e) => Err(Error::SerializationError(e)),
                    }
                })
            })
        } else {
            Err(Error::UnsupportedMethod)
        }
    }

    /// [`Self::replace`] variant for Rhai.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if the map cannot be deserialized, or wrapping the
    /// [`Self::replace`] errors and [`Error::SerializationError`].
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn rhai_replace(&mut self, name: String, data: rhai::Dynamic) -> RhaiRes<Dynamic> {
        let data = rhai::serde::from_dynamic(&data)?;
        let res = self.replace(&name, data).map_err(|e: Error| rhai_err(e))?;
        let v = serde_json::to_value(res).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        to_dynamic(v)
    }

    /// Server-side applies a patch as the configured field manager (forced).
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMethod`] if the resource was not resolved, or
    /// [`Error::KubeError`] if the API call fails.
    pub fn patch(
        &self,
        name: &str,
        patch_data: serde_json::Map<String, serde_json::Value>,
    ) -> Result<DynamicObject> {
        if let Some(api) = self.api.clone() {
            let handle = self.inject_labels_and_owner(patch_data);
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    api.patch(
                        name,
                        &PatchParams::apply(&crate::get_client_name()).force(),
                        &Patch::Apply(handle),
                    )
                    .await
                    .map_err(Error::KubeError)
                })
            })
        } else {
            Err(Error::UnsupportedMethod)
        }
    }

    /// [`Self::patch`] variant for Rhai.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if the map cannot be deserialized, or wrapping the
    /// [`Self::patch`] errors and [`Error::SerializationError`].
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn rhai_patch(&mut self, name: String, data: rhai::Dynamic) -> RhaiRes<Dynamic> {
        let data = rhai::serde::from_dynamic(&data)?;
        let res = self.patch(&name, data).map_err(|e: Error| rhai_err(e))?;
        let v = serde_json::to_value(res).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        to_dynamic(v)
    }

    /// Server-side applies a patch (as [`Self::patch`]), tolerating the immutable-spec error of
    /// an already-completed `Job` by returning the current object instead of failing.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMethod`] if the resource was not resolved, or
    /// [`Error::KubeError`] if the API call fails and the completed-Job fallback does not apply.
    pub fn apply(
        &self,
        name: &str,
        patch_data: serde_json::Map<String, serde_json::Value>,
    ) -> Result<DynamicObject> {
        if let Some(api) = self.api.clone() {
            let kind = patch_data
                .get("kind")
                .and_then(|k| k.as_str())
                .unwrap_or("")
                .to_string();
            let handle = self.inject_labels_and_owner(patch_data);
            let api_for_get = api.clone();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    match api
                        .patch(
                            name,
                            &PatchParams::apply(&crate::get_client_name()).force(),
                            &Patch::Apply(handle),
                        )
                        .await
                    {
                        Ok(obj) => Ok(obj),
                        Err(e) => {
                            if kind == "Job"
                                && e.to_string().contains("immutable")
                                && let Ok(current) = api_for_get.get(name).await
                                && job_is_completed(&current.data)
                            {
                                tracing::debug!(
                                    "Job {name} spec.template immutable but already completed — skipping"
                                );
                                return Ok(current);
                            }
                            Err(Error::KubeError(e))
                        }
                    }
                })
            })
        } else {
            Err(Error::UnsupportedMethod)
        }
    }

    /// [`Self::apply`] variant for Rhai.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if the map cannot be deserialized, or wrapping the [`Self::apply`]
    /// errors and [`Error::SerializationError`].
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn rhai_apply(&mut self, name: String, data: rhai::Dynamic) -> RhaiRes<Dynamic> {
        let data = rhai::serde::from_dynamic(&data)?;
        let res = self.apply(&name, data).map_err(|e: Error| rhai_err(e))?;
        let v = serde_json::to_value(res).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        to_dynamic(v)
    }
}

fn job_is_completed(data: &serde_json::Value) -> bool {
    let Some(status) = data.get("status") else {
        return false;
    };
    if status
        .get("succeeded")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
        > 0
    {
        return true;
    }
    status.get("completionTime").is_some()
}

// ── k8sraw ───────────────────────────────────────────────────────────────────

/// Shared kube client dedicated to raw API calls, built like [`CLIENT`].
///
/// Panics on first access if the k8s context was not wired (see [`context_is_wired`]).
pub static RAW_CLIENT: LazyLock<Client> = LazyLock::new(build_client);

/// Raw HTTP access to the Kubernetes API server through the kube client (`K8sRaw` in Rhai).
#[derive(Clone)]
pub struct K8sRaw {
    /// Underlying kube client (the shared [`RAW_CLIENT`] by default).
    pub client: Client,
}

impl Default for K8sRaw {
    fn default() -> Self {
        Self::new()
    }
}

impl K8sRaw {
    /// Builds a [`K8sRaw`] bound to the shared [`RAW_CLIENT`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: RAW_CLIENT.clone(),
        }
    }

    /// GETs a path on the API server and returns the JSON body.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RawHTTP`] if the request cannot be built, or [`Error::KubeError`] if the
    /// call fails or the body is not valid JSON.
    pub async fn get_url(&self, url: String) -> Result<serde_json::Value> {
        let req = http::Request::get(url)
            .body(Vec::default())
            .map_err(Error::RawHTTP)?;
        let resp = self
            .client
            .request::<serde_json::Value>(req)
            .await
            .map_err(Error::KubeError)?;
        Ok(resp)
    }

    /// GETs a path with the aggregated-discovery `Accept` header (v2/v2beta1 JSON).
    ///
    /// # Errors
    ///
    /// Returns [`Error::RawHTTP`] if the request cannot be built, or [`Error::KubeError`] if the
    /// call fails or the body is not valid JSON.
    pub async fn get_url_as_disco(&self, url: String) -> Result<serde_json::Value> {
        let req = http::Request::get(url)
            .header("Accept", "application/json;g=apidiscovery.k8s.io;v=v2;as=APIGroupDiscoveryList,application/json;g=apidiscovery.k8s.io;v=v2beta1;as=APIGroupDiscoveryList,application/json")
            .body(Vec::default()).map_err(Error::RawHTTP)?;
        let resp = self
            .client
            .request::<serde_json::Value>(req)
            .await
            .map_err(Error::KubeError)?;
        Ok(resp)
    }

    /// Cluster version information (server `/version` endpoint).
    ///
    /// # Errors
    ///
    /// Forwards the [`Self::get_url`] errors.
    pub async fn get_api_version(&self) -> Result<serde_json::Value> {
        self.get_url("/version".to_string()).await
    }

    /// Full API group discovery list (server `/apis` endpoint).
    ///
    /// # Errors
    ///
    /// Forwards the [`Self::get_url_as_disco`] errors.
    pub async fn get_api_resources(&self) -> Result<serde_json::Value> {
        self.get_url_as_disco("/apis".to_string()).await
    }

    /// [`Self::get_url`] as a Rhai value (JSON round-trip through a string).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping the [`Self::get_url`] errors or
    /// [`Error::SerializationError`].
    pub fn rhai_get_url(&mut self, url: String) -> RhaiRes<Dynamic> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let res = self.get_url(url).await.map_err(rhai_err)?;
                let v = serde_json::to_string(&res)
                    .map_err(Error::SerializationError)
                    .map_err(rhai_err)?;
                serde_json::from_str(&v)
                    .map_err(Error::SerializationError)
                    .map_err(rhai_err)
            })
        })
    }

    /// [`Self::get_api_version`] as a Rhai value (JSON round-trip through a string).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping the [`Self::get_api_version`] errors or
    /// [`Error::SerializationError`].
    pub fn rhai_get_api_version(&mut self) -> RhaiRes<Dynamic> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let ver = self.get_api_version().await.map_err(rhai_err)?;
                let v = serde_json::to_string(&ver)
                    .map_err(Error::SerializationError)
                    .map_err(rhai_err)?;
                serde_json::from_str(&v)
                    .map_err(Error::SerializationError)
                    .map_err(rhai_err)
            })
        })
    }

    /// [`Self::get_api_resources`] as a Rhai value (JSON round-trip through a string).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping the [`Self::get_api_resources`] errors or
    /// [`Error::SerializationError`].
    pub fn rhai_get_api_resources(&mut self) -> RhaiRes<Dynamic> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let ver = self.get_api_resources().await.map_err(rhai_err)?;
                let v = serde_json::to_string(&ver)
                    .map_err(Error::SerializationError)
                    .map_err(rhai_err)?;
                serde_json::from_str(&v)
                    .map_err(Error::SerializationError)
                    .map_err(rhai_err)
            })
        })
    }
}

// ── k8sworkload ──────────────────────────────────────────────────────────────

/// Typed `DaemonSet` handle fetched from a namespace (`K8sDaemonSet` in Rhai).
#[derive(Clone, Debug)]
pub struct K8sDaemonSet {
    /// Namespace-scoped API handle for this `DaemonSet`.
    pub api: Api<DaemonSet>,
    /// The `DaemonSet` as fetched.
    pub obj: DaemonSet,
}
impl K8sDaemonSet {
    /// Condition matching when `number_available` reached `desired_number_scheduled`.
    #[must_use]
    pub fn is_deamonset_available() -> impl Condition<DaemonSet> {
        |obj: Option<&DaemonSet>| {
            if let Some(ds) = &obj
                && let Some(s) = &ds.status
            {
                return s.desired_number_scheduled == s.number_available.unwrap_or(0);
            }
            false
        }
    }

    /// Fetches a `DaemonSet` by namespace and name (`new_deamonset` entry point in Rhai).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::KubeError`] if the API call fails.
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn get_deamonset(namespace: String, name: String) -> RhaiRes<K8sDaemonSet> {
        let api: Api<DaemonSet> = Api::namespaced(CLIENT.clone(), &namespace);
        let d = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { api.get(&name).await.map_err(Error::KubeError) })
        })
        .map_err(rhai_err)?;
        Ok(K8sDaemonSet {
            api: Api::namespaced(CLIENT.clone(), &namespace),
            obj: d,
        })
    }

    /// Metadata rendered as a Rhai value (JSON string round-trip).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::SerializationError`].
    pub fn get_metadata(&mut self) -> RhaiRes<Dynamic> {
        let v =
            serde_json::to_string(&self.obj.metadata).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        serde_json::from_str(&v).map_err(|e| rhai_err(Error::SerializationError(e)))
    }

    /// Spec rendered as a Rhai value (JSON string round-trip).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::SerializationError`].
    pub fn get_spec(&mut self) -> RhaiRes<Dynamic> {
        let v = serde_json::to_string(&self.obj.spec).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        serde_json::from_str(&v).map_err(|e| rhai_err(Error::SerializationError(e)))
    }

    /// Status rendered as a Rhai value (JSON string round-trip).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::SerializationError`].
    pub fn get_status(&mut self) -> RhaiRes<Dynamic> {
        let v =
            serde_json::to_string(&self.obj.status).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        serde_json::from_str(&v).map_err(|e| rhai_err(Error::SerializationError(e)))
    }

    /// Waits up to `timeout` seconds until all desired pods are available.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if `timeout` elapses ([`Error::Elapsed`]) or the watch fails
    /// ([`Error::KubeWaitError`]).
    pub fn wait_available(&mut self, timeout: i64) -> RhaiRes<()> {
        let name = self.obj.name_any();
        let cond = await_condition(self.api.clone(), &name, Self::is_deamonset_available());
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                tokio::time::timeout(timeout_duration(timeout), cond)
                    .await
                    .map_err(Error::Elapsed)
            })
        })
        .map_err(rhai_err)?
        .map_err(|e| rhai_err(Error::KubeWaitError(e)))?;
        Ok(())
    }
}

/// Typed `StatefulSet` handle fetched from a namespace (`K8sStatefulSet` in Rhai).
#[derive(Clone, Debug)]
pub struct K8sStatefulSet {
    /// Namespace-scoped API handle for this `StatefulSet`.
    pub api: Api<StatefulSet>,
    /// The `StatefulSet` as fetched.
    pub obj: StatefulSet,
}
impl K8sStatefulSet {
    /// Condition matching when `available_replicas` reached `spec.replicas` (defaults 1/0).
    #[must_use]
    pub fn is_sts_available() -> impl Condition<StatefulSet> {
        |obj: Option<&StatefulSet>| {
            if let Some(sts) = &obj
                && let Some(spec) = &sts.spec
                && let Some(s) = &sts.status
            {
                return spec.replicas.unwrap_or(1) == s.available_replicas.unwrap_or(0);
            }
            false
        }
    }

    /// Fetches a `StatefulSet` by namespace and name (`get_statefulset` entry point in Rhai).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::KubeError`] if the API call fails.
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn get_sts(namespace: String, name: String) -> RhaiRes<K8sStatefulSet> {
        let api: Api<StatefulSet> = Api::namespaced(CLIENT.clone(), &namespace);
        let d = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { api.get(&name).await.map_err(Error::KubeError) })
        })
        .map_err(rhai_err)?;
        Ok(K8sStatefulSet {
            api: Api::namespaced(CLIENT.clone(), &namespace),
            obj: d,
        })
    }

    /// Metadata rendered as a Rhai value (JSON string round-trip).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::SerializationError`].
    pub fn get_metadata(&mut self) -> RhaiRes<Dynamic> {
        let v =
            serde_json::to_string(&self.obj.metadata).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        serde_json::from_str(&v).map_err(|e| rhai_err(Error::SerializationError(e)))
    }

    /// Spec rendered as a Rhai value (JSON string round-trip).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::SerializationError`].
    pub fn get_spec(&mut self) -> RhaiRes<Dynamic> {
        let v = serde_json::to_string(&self.obj.spec).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        serde_json::from_str(&v).map_err(|e| rhai_err(Error::SerializationError(e)))
    }

    /// Status rendered as a Rhai value (JSON string round-trip).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::SerializationError`].
    pub fn get_status(&mut self) -> RhaiRes<Dynamic> {
        let v =
            serde_json::to_string(&self.obj.status).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        serde_json::from_str(&v).map_err(|e| rhai_err(Error::SerializationError(e)))
    }

    /// Waits up to `timeout` seconds until `available_replicas` reaches the desired count.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if `timeout` elapses ([`Error::Elapsed`]) or the watch fails
    /// ([`Error::KubeWaitError`]).
    pub fn wait_available(&mut self, timeout: i64) -> RhaiRes<()> {
        let name = self.obj.name_any();
        let cond = await_condition(self.api.clone(), &name, Self::is_sts_available());
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                tokio::time::timeout(timeout_duration(timeout), cond)
                    .await
                    .map_err(Error::Elapsed)
            })
        })
        .map_err(rhai_err)?
        .map_err(|e| rhai_err(Error::KubeWaitError(e)))?;
        Ok(())
    }
}

/// Typed `Deployment` handle fetched from a namespace (`K8sDeploy` in Rhai).
#[derive(Clone, Debug)]
pub struct K8sDeploy {
    /// Namespace-scoped API handle for this Deployment.
    pub api: Api<Deployment>,
    /// The Deployment as fetched.
    pub obj: Deployment,
}
impl K8sDeploy {
    /// Condition matching when the `Available` status condition is `"True"`.
    #[must_use]
    pub fn is_deploy_available() -> impl Condition<Deployment> {
        |obj: Option<&Deployment>| {
            if let Some(job) = &obj
                && let Some(s) = &job.status
                && let Some(conds) = &s.conditions
                && let Some(pcond) = conds.iter().find(|c| c.type_ == "Available")
            {
                return pcond.status == "True";
            }
            false
        }
    }

    /// Fetches a `Deployment` by namespace and name (Rhai `get_deployment` entry point).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::KubeError`] if the API call fails.
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn get_deployment(namespace: String, name: String) -> RhaiRes<K8sDeploy> {
        let api: Api<Deployment> = Api::namespaced(CLIENT.clone(), &namespace);
        let d = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { api.get(&name).await.map_err(Error::KubeError) })
        })
        .map_err(rhai_err)?;
        Ok(K8sDeploy {
            api: Api::namespaced(CLIENT.clone(), &namespace),
            obj: d,
        })
    }

    /// Metadata rendered as a Rhai value (JSON string round-trip).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::SerializationError`].
    pub fn get_metadata(&mut self) -> RhaiRes<Dynamic> {
        let v =
            serde_json::to_string(&self.obj.metadata).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        serde_json::from_str(&v).map_err(|e| rhai_err(Error::SerializationError(e)))
    }

    /// Spec rendered as a Rhai value (JSON string round-trip).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::SerializationError`].
    pub fn get_spec(&mut self) -> RhaiRes<Dynamic> {
        let v = serde_json::to_string(&self.obj.spec).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        serde_json::from_str(&v).map_err(|e| rhai_err(Error::SerializationError(e)))
    }

    /// Status rendered as a Rhai value (JSON string round-trip).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::SerializationError`].
    pub fn get_status(&mut self) -> RhaiRes<Dynamic> {
        let v =
            serde_json::to_string(&self.obj.status).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        serde_json::from_str(&v).map_err(|e| rhai_err(Error::SerializationError(e)))
    }

    /// Waits up to `timeout` seconds until the `Available` condition turns `"True"`.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if `timeout` elapses ([`Error::Elapsed`]) or the watch fails
    /// ([`Error::KubeWaitError`]).
    pub fn wait_available(&mut self, timeout: i64) -> RhaiRes<()> {
        let name = self.obj.name_any();
        let cond = await_condition(self.api.clone(), &name, Self::is_deploy_available());
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                tokio::time::timeout(timeout_duration(timeout), cond)
                    .await
                    .map_err(Error::Elapsed)
            })
        })
        .map_err(rhai_err)?
        .map_err(|e| rhai_err(Error::KubeWaitError(e)))?;
        Ok(())
    }
}

/// Typed `Job` handle fetched from a namespace (`K8sJob` in Rhai).
#[derive(Clone, Debug)]
pub struct K8sJob {
    /// Namespace-scoped API handle for this Job.
    pub api: Api<Job>,
    /// The Job as fetched.
    pub obj: Job,
}
impl K8sJob {
    /// Fetches a `Job` by namespace and name (Rhai `get_job` entry point).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::KubeError`] if the API call fails.
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn get_job(namespace: String, name: String) -> RhaiRes<K8sJob> {
        let api: Api<Job> = Api::namespaced(CLIENT.clone(), &namespace);
        let j = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { api.get(&name).await.map_err(Error::KubeError) })
        })
        .map_err(rhai_err)?;
        Ok(K8sJob {
            api: Api::namespaced(CLIENT.clone(), &namespace),
            obj: j,
        })
    }

    /// Metadata rendered as a Rhai value (JSON string round-trip).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::SerializationError`].
    pub fn get_metadata(&mut self) -> RhaiRes<Dynamic> {
        let v =
            serde_json::to_string(&self.obj.metadata).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        serde_json::from_str(&v).map_err(|e| rhai_err(Error::SerializationError(e)))
    }

    /// Spec rendered as a Rhai value (JSON string round-trip).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::SerializationError`].
    pub fn get_spec(&mut self) -> RhaiRes<Dynamic> {
        let v = serde_json::to_string(&self.obj.spec).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        serde_json::from_str(&v).map_err(|e| rhai_err(Error::SerializationError(e)))
    }

    /// Status rendered as a Rhai value (JSON string round-trip).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::SerializationError`].
    pub fn get_status(&mut self) -> RhaiRes<Dynamic> {
        let v =
            serde_json::to_string(&self.obj.status).map_err(|e| rhai_err(Error::SerializationError(e)))?;
        serde_json::from_str(&v).map_err(|e| rhai_err(Error::SerializationError(e)))
    }

    /// Waits up to `timeout` seconds for the Job's `Completed` condition.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if `timeout` elapses ([`Error::Elapsed`]) or the watch fails
    /// ([`Error::KubeWaitError`]).
    pub fn wait_done(&mut self, timeout: i64) -> RhaiRes<()> {
        let name = self.obj.name_any();
        let cond = await_condition(self.api.clone(), &name, conditions::is_job_completed());
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                tokio::time::timeout(timeout_duration(timeout), cond)
                    .await
                    .map_err(Error::Elapsed)
            })
        })
        .map_err(rhai_err)?
        .map_err(|e| rhai_err(Error::KubeWaitError(e)))?;
        Ok(())
    }
}

// ── Rhai registration ────────────────────────────────────────────────────────

/// Registers `K8sGeneric`, `K8sObject` and `DynamicObject` types on a Rhai engine.
pub fn k8sgeneric_rhai_register(engine: &mut Engine) {
    use crate::{register_k8s_generic, register_k8s_object};
    engine
        .register_type_with_name::<DynamicObject>("DynamicObject")
        .register_get("data", |obj: &mut DynamicObject| -> Dynamic {
            Dynamic::from(obj.data.clone())
        });
    register_k8s_object!(engine, K8sObject);
    register_k8s_generic!(
        engine,
        K8sGeneric,
        K8sObject,
        K8sGeneric::new_global,
        K8sGeneric::new_ns,
        K8sGeneric::new_group_ns
    );
}

/// Registers the `K8sRaw` type (raw API access) on a Rhai engine.
pub fn k8sraw_rhai_register(engine: &mut Engine) {
    use crate::register_k8s_raw;
    register_k8s_raw!(engine, K8sRaw, K8sRaw::new);
}

/// Registers the typed workload helpers (`K8sDeploy`, `K8sDaemonSet`, `K8sStatefulSet`,
/// `K8sJob`) on a Rhai engine.
pub fn k8sworkload_rhai_register(engine: &mut Engine) {
    engine
        .register_type_with_name::<K8sDeploy>("K8sDeploy")
        .register_fn("get_deployment", K8sDeploy::get_deployment)
        .register_get("metadata", K8sDeploy::get_metadata)
        .register_get("spec", K8sDeploy::get_spec)
        .register_get("status", K8sDeploy::get_status)
        .register_fn("wait_available", K8sDeploy::wait_available);
    engine
        .register_type_with_name::<K8sDaemonSet>("K8sDaemonSet")
        .register_fn("get_deamonset", K8sDaemonSet::get_deamonset)
        .register_get("metadata", K8sDaemonSet::get_metadata)
        .register_get("spec", K8sDaemonSet::get_spec)
        .register_get("status", K8sDaemonSet::get_status)
        .register_fn("wait_available", K8sDaemonSet::wait_available);
    engine
        .register_type_with_name::<K8sStatefulSet>("K8sStatefulSet")
        .register_fn("get_statefulset", K8sStatefulSet::get_sts)
        .register_get("metadata", K8sStatefulSet::get_metadata)
        .register_get("spec", K8sStatefulSet::get_spec)
        .register_get("status", K8sStatefulSet::get_status)
        .register_fn("wait_available", K8sStatefulSet::wait_available);
    engine
        .register_type_with_name::<K8sJob>("K8sJob")
        .register_fn("get_job", K8sJob::get_job)
        .register_get("metadata", K8sJob::get_metadata)
        .register_get("spec", K8sJob::get_spec)
        .register_get("status", K8sJob::get_status)
        .register_fn("wait_done", K8sJob::wait_done);
}

// ── Macros ───────────────────────────────────────────────────────────────────

/// Registers `$type` as the Rhai type `"K8sObject"`: kind/metadata getters plus `delete` and
/// the `wait_*` methods.
#[macro_export]
macro_rules! register_k8s_object {
    ($engine:expr, $type:ty) => {{
        let _delete: fn(&mut $type) -> $crate::RhaiRes<()> = <$type>::rhai_delete;
        let _wait_deleted: fn(&mut $type, i64) -> $crate::RhaiRes<()> = <$type>::rhai_wait_deleted;
        let _get_kind: fn(&mut $type) -> String = <$type>::get_kind;
        let _original_kind: fn(&mut $type) -> String = <$type>::original_kind;
        let _get_metadata: fn(&mut $type) -> $crate::RhaiRes<rhai::Dynamic> = <$type>::get_metadata;
        let _wait_condition: fn(&mut $type, String, i64) -> $crate::RhaiRes<()> = <$type>::wait_condition;
        let _wait_status: fn(&mut $type, String, i64) -> $crate::RhaiRes<()> = <$type>::wait_status;
        let _wait_status_prop: fn(&mut $type, String, i64) -> $crate::RhaiRes<()> = <$type>::wait_status_prop;
        let _wait_status_string: fn(&mut $type, String, String, i64) -> $crate::RhaiRes<()> =
            <$type>::wait_status_string;
        $engine
            .register_type_with_name::<$type>("K8sObject")
            .register_get("kind", _get_kind)
            .register_get("original_kind", _original_kind)
            .register_get("metadata", _get_metadata)
            .register_fn("delete", _delete)
            .register_fn("wait_condition", _wait_condition)
            .register_fn("wait_status", _wait_status)
            .register_fn("wait_status_prop", _wait_status_prop)
            .register_fn("wait_status_string", _wait_status_string)
            .register_fn("wait_for", <$type>::wait_for)
            .register_fn("wait_deleted", _wait_deleted)
    }};
}

/// Registers `$type` as the Rhai type `"K8sGeneric"`: the three `k8s_resource` constructors
/// (`$new_global`, `$new_ns`, `$new_group_ns`) plus CRUD/list/scope methods. Takes `$obj_type`
/// as the object type returned by `get_obj`.
#[macro_export]
macro_rules! register_k8s_generic {
    ($engine:expr, $type:ty, $obj_type:ty,
     $new_global:expr, $new_ns:expr, $new_group_ns:expr) => {{
        let _scope: fn(&mut $type) -> String = <$type>::rhai_get_scope;
        let _exist: fn(&mut $type) -> $crate::RhaiRes<rhai::Dynamic> = <$type>::rhai_exist;
        let _list: fn(&mut $type) -> $crate::RhaiRes<rhai::Dynamic> = <$type>::rhai_list;
        let _list_labels: fn(&mut $type, String) -> $crate::RhaiRes<rhai::Dynamic> =
            <$type>::rhai_list_labels;
        let _list_meta: fn(&mut $type) -> $crate::RhaiRes<rhai::Dynamic> = <$type>::rhai_list_meta;
        let _get: fn(&mut $type, String) -> $crate::RhaiRes<rhai::Dynamic> = <$type>::rhai_get;
        let _get_meta: fn(&mut $type, String) -> $crate::RhaiRes<rhai::Dynamic> = <$type>::rhai_get_meta;
        let _get_obj: fn(&mut $type, String) -> $crate::RhaiRes<$obj_type> = <$type>::rhai_get_obj;
        let _delete: fn(&mut $type, String) -> $crate::RhaiRes<()> = <$type>::rhai_delete;
        let _create: fn(&mut $type, rhai::Dynamic) -> $crate::RhaiRes<rhai::Dynamic> = <$type>::rhai_create;
        let _replace: fn(&mut $type, String, rhai::Dynamic) -> $crate::RhaiRes<rhai::Dynamic> =
            <$type>::rhai_replace;
        let _patch: fn(&mut $type, String, rhai::Dynamic) -> $crate::RhaiRes<rhai::Dynamic> =
            <$type>::rhai_patch;
        let _apply: fn(&mut $type, String, rhai::Dynamic) -> $crate::RhaiRes<rhai::Dynamic> =
            <$type>::rhai_apply;
        $engine
            .register_type_with_name::<$type>("K8sGeneric")
            .register_fn("k8s_resource", $new_global)
            .register_fn("k8s_resource", $new_ns)
            .register_fn("k8s_resource", $new_group_ns)
            .register_fn("list", _list)
            .register_fn("list", _list_labels)
            .register_fn("update_k8s_crd_cache", $crate::k8s::update_cache)
            .register_fn("list_meta", _list_meta)
            .register_fn("get", _get)
            .register_fn("get_meta", _get_meta)
            .register_fn("get_obj", _get_obj)
            .register_fn("delete", _delete)
            .register_fn("create", _create)
            .register_fn("replace", _replace)
            .register_fn("patch", _patch)
            .register_fn("apply", _apply)
            .register_fn("exist", _exist)
            .register_get("scope", _scope)
    }};
}

/// Registers `$type` as the Rhai type `"K8sRaw"` with the `$new` constructor and the
/// `get_url` / `get_cluster_version` / `get_api_resources` methods.
#[macro_export]
macro_rules! register_k8s_raw {
    ($engine:expr, $type:ty, $new:expr) => {{
        let _get_url: fn(&mut $type, String) -> $crate::RhaiRes<rhai::Dynamic> = <$type>::rhai_get_url;
        let _get_version: fn(&mut $type) -> $crate::RhaiRes<rhai::Dynamic> = <$type>::rhai_get_api_version;
        let _get_api_resources: fn(&mut $type) -> $crate::RhaiRes<rhai::Dynamic> =
            <$type>::rhai_get_api_resources;
        $engine
            .register_type_with_name::<$type>("K8sRaw")
            .register_fn("new_k8s_raw", $new)
            .register_fn("get_url", _get_url)
            .register_fn("get_cluster_version", _get_version)
            .register_fn("get_api_resources", _get_api_resources)
    }};
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_excludes_aggregated_keeps_local_apiservices() {
        let aggregated = serde_json::json!({
            "group": "metrics.k8s.io",
            "service": { "name": "metrics-server", "namespace": "kube-system" }
        });
        assert_eq!(
            aggregated_apiservice_group(&aggregated),
            Some("metrics.k8s.io".to_string())
        );

        let local_null = serde_json::json!({ "group": "storage.k8s.io", "service": null });
        assert_eq!(aggregated_apiservice_group(&local_null), None);
        let local_absent = serde_json::json!({ "group": "apps" });
        assert_eq!(aggregated_apiservice_group(&local_absent), None);

        let empty_group = serde_json::json!({ "group": "", "service": { "name": "x" } });
        assert_eq!(aggregated_apiservice_group(&empty_group), None);
    }

    #[test]
    fn test_job_is_completed_succeeded() {
        let data = serde_json::json!({"status": {"succeeded": 1}});
        assert!(job_is_completed(&data));
    }

    #[test]
    fn test_job_is_completed_zero_succeeded() {
        let data = serde_json::json!({"status": {"succeeded": 0}});
        assert!(!job_is_completed(&data));
    }

    #[test]
    fn test_job_is_completed_completion_time() {
        let data = serde_json::json!({"status": {"completionTime": "2024-01-01T00:00:00Z"}});
        assert!(job_is_completed(&data));
    }

    #[test]
    fn test_job_is_completed_no_status() {
        let data = serde_json::json!({});
        assert!(!job_is_completed(&data));
    }

    #[test]
    fn test_job_is_completed_still_running() {
        let data = serde_json::json!({"status": {"active": 1, "succeeded": 0}});
        assert!(!job_is_completed(&data));
    }

    // ── prepare_handle ────────────────────────────────────────────────────────

    fn map(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        match v {
            serde_json::Value::Object(m) => m,
            _ => panic!("expected object"),
        }
    }

    #[test]
    fn prepare_handle_inserts_metadata_when_absent() {
        let input = map(serde_json::json!({"kind": "ConfigMap", "spec": {}}));
        let out = prepare_handle(input, None, None, None, None, false);
        assert!(out.contains_key("metadata"), "metadata must be added");
        assert!(out["metadata"].is_object());
    }

    #[test]
    fn prepare_handle_inserts_metadata_when_not_object() {
        let input = map(serde_json::json!({"metadata": "bad-string", "kind": "ConfigMap"}));
        let out = prepare_handle(input, None, None, None, None, false);
        assert!(
            out["metadata"].is_object(),
            "non-object metadata must be replaced with {{}}"
        );
    }

    #[test]
    fn prepare_handle_preserves_existing_metadata() {
        let input = map(serde_json::json!({"metadata": {"name": "foo"}, "kind": "ConfigMap"}));
        let out = prepare_handle(input, None, None, None, None, false);
        assert_eq!(out["metadata"]["name"], "foo");
    }

    #[test]
    fn prepare_handle_injects_labels_when_missing() {
        let input = map(serde_json::json!({"kind": "ConfigMap"}));
        let labels = serde_json::json!({"app": "myapp", "tier": "backend"});
        let out = prepare_handle(input, Some(labels), None, None, None, false);
        assert_eq!(out["metadata"]["labels"]["app"], "myapp");
        assert_eq!(out["metadata"]["labels"]["tier"], "backend");
    }

    #[test]
    fn prepare_handle_labels_do_not_override_existing() {
        let input = map(serde_json::json!({"metadata": {"labels": {"app": "existing"}}}));
        let labels = serde_json::json!({"app": "override-attempt", "extra": "v"});
        let out = prepare_handle(input, Some(labels), None, None, None, false);
        assert_eq!(
            out["metadata"]["labels"]["app"], "existing",
            "existing label must not be overridden"
        );
        assert_eq!(
            out["metadata"]["labels"]["extra"], "v",
            "new label must be injected"
        );
    }

    #[test]
    fn prepare_handle_owner_ref_injected_when_same_ns() {
        let input = map(serde_json::json!({"kind": "ConfigMap"}));
        let owner = serde_json::json!({"apiVersion": "v1", "kind": "Pod", "name": "owner", "uid": "abc"});
        let out = prepare_handle(
            input,
            None,
            Some(owner.clone()),
            Some("mynamespace".to_string()),
            Some("mynamespace".to_string()),
            true,
        );
        let refs = out["metadata"]["ownerReferences"].as_array().unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0]["uid"], "abc");
    }

    #[test]
    fn prepare_handle_owner_ref_not_injected_when_different_ns() {
        let input = map(serde_json::json!({"kind": "ConfigMap"}));
        let owner = serde_json::json!({"apiVersion": "v1", "kind": "Pod", "name": "owner", "uid": "abc"});
        let out = prepare_handle(
            input,
            None,
            Some(owner),
            Some("other-ns".to_string()),
            Some("mynamespace".to_string()),
            true,
        );
        assert!(
            out["metadata"]
                .as_object()
                .unwrap()
                .get("ownerReferences")
                .is_none()
        );
    }

    // ── Condition closures ────────────────────────────────────────────────────

    fn dynobj(extra: serde_json::Value) -> DynamicObject {
        let mut base = serde_json::json!({
            "apiVersion": "v1",
            "kind": "Foo",
            "metadata": {"name": "test"}
        });
        if let (Some(obj), serde_json::Value::Object(fields)) = (base.as_object_mut(), extra) {
            obj.extend(fields);
        }
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn is_condition_matches_ready_true() {
        let obj = dynobj(serde_json::json!({
            "status": {"conditions": [{"type": "Ready", "status": "True"}]}
        }));
        assert!(K8sObject::is_condition("Ready".to_string()).matches_object(Some(&obj)));
    }

    #[test]
    fn is_condition_no_match_wrong_type() {
        let obj = dynobj(serde_json::json!({
            "status": {"conditions": [{"type": "Available", "status": "True"}]}
        }));
        assert!(!K8sObject::is_condition("Ready".to_string()).matches_object(Some(&obj)));
    }

    #[test]
    fn is_condition_no_match_status_false() {
        let obj = dynobj(serde_json::json!({
            "status": {"conditions": [{"type": "Ready", "status": "False"}]}
        }));
        assert!(!K8sObject::is_condition("Ready".to_string()).matches_object(Some(&obj)));
    }

    #[test]
    fn is_condition_missing_conditions_returns_false() {
        let obj = dynobj(serde_json::json!({"status": {}}));
        assert!(!K8sObject::is_condition("Ready".to_string()).matches_object(Some(&obj)));
    }

    #[test]
    fn is_condition_missing_status_returns_false() {
        let obj = dynobj(serde_json::json!({}));
        assert!(!K8sObject::is_condition("Ready".to_string()).matches_object(Some(&obj)));
    }

    #[test]
    fn is_condition_on_none_returns_false() {
        assert!(!K8sObject::is_condition("Ready".to_string()).matches_object(None));
    }

    #[test]
    fn is_status_true_boolean() {
        let obj = dynobj(serde_json::json!({"status": {"healthy": true}}));
        assert!(K8sObject::is_status("healthy".to_string()).matches_object(Some(&obj)));
    }

    #[test]
    fn is_status_false_boolean() {
        let obj = dynobj(serde_json::json!({"status": {"healthy": false}}));
        assert!(!K8sObject::is_status("healthy".to_string()).matches_object(Some(&obj)));
    }

    #[test]
    fn is_status_missing_prop() {
        let obj = dynobj(serde_json::json!({"status": {}}));
        assert!(!K8sObject::is_status("healthy".to_string()).matches_object(Some(&obj)));
    }

    #[test]
    fn have_status_non_null() {
        let obj = dynobj(serde_json::json!({"status": {"phase": "Running"}}));
        assert!(K8sObject::have_status("phase".to_string()).matches_object(Some(&obj)));
    }

    #[test]
    fn have_status_null_value() {
        let obj = dynobj(serde_json::json!({"status": {"phase": null}}));
        assert!(!K8sObject::have_status("phase".to_string()).matches_object(Some(&obj)));
    }

    #[test]
    fn have_status_missing_prop() {
        let obj = dynobj(serde_json::json!({"status": {}}));
        assert!(!K8sObject::have_status("phase".to_string()).matches_object(Some(&obj)));
    }

    #[test]
    fn have_status_value_matches() {
        let obj = dynobj(serde_json::json!({"status": {"phase": "Running"}}));
        assert!(
            K8sObject::have_status_value("phase".to_string(), "Running".to_string())
                .matches_object(Some(&obj))
        );
    }

    #[test]
    fn have_status_value_no_match() {
        let obj = dynobj(serde_json::json!({"status": {"phase": "Pending"}}));
        assert!(
            !K8sObject::have_status_value("phase".to_string(), "Running".to_string())
                .matches_object(Some(&obj))
        );
    }

    #[test]
    fn have_status_value_missing_prop() {
        let obj = dynobj(serde_json::json!({"status": {}}));
        assert!(
            !K8sObject::have_status_value("phase".to_string(), "Running".to_string())
                .matches_object(Some(&obj))
        );
    }
}
