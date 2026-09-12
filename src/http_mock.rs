//! Mock HTTP client for tests — Rhai-compatible drop-in for [`crate::http::RestClient`].
//!
//! Configure expected `path`/`method` → `return_obj` mappings and register via
//! `httpmock_rhai_register` instead of `crate::http::http_rhai_register`.

use crate::RhaiRes;
use rhai::{Dynamic, Engine, Map};
use serde::{Deserialize, Serialize};

/// HTTP method matched by a mock entry.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Default)]
pub enum HttpMethod {
    /// `GET`.
    #[default]
    Get,
    /// `HEAD`.
    Head,
    /// `DELETE`.
    Delete,
    /// `PATCH`.
    Patch,
    /// `POST`.
    Post,
    /// `PUT`.
    Put,
}

/// One mocked request → canned response mapping.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct HttpMockItem {
    /// Request path the entry matches.
    pub path: String,
    /// HTTP method the entry matches.
    pub method: HttpMethod,
    /// Map returned (as the response body) when the entry matches.
    pub return_obj: Map,
}

/// In-memory mock that answers `get`/`post`/… from a pre-configured list. See module docs.
#[derive(Clone, Debug)]
pub struct RestClientMock {
    baseurl: String,
    mocks: Vec<HttpMockItem>,
}
impl RestClientMock {
    /// Creates a mock client for `base` answering from `mocks` (first match wins).
    #[must_use]
    pub fn new(base: &str, mocks: Vec<HttpMockItem>) -> Self {
        Self {
            baseurl: base.to_string(),
            mocks,
        }
    }

    /// No-op, mirrors `RestClient::set_server_ca`.
    pub fn set_server_ca(&mut self, _ca: &str) {}

    /// No-op, mirrors `RestClient::set_mtls`.
    pub fn set_mtls(&mut self, _cert: &str, _key: &str) {}

    /// No-op, mirrors `RestClient::headers_reset`.
    pub fn headers_reset(&mut self) {}

    /// No-op, mirrors `RestClient::add_header`.
    pub fn add_header(&mut self, _key: String, _value: String) {}

    /// No-op, mirrors `RestClient::add_header_json`.
    pub fn add_header_json(&mut self) {}

    /// No-op, mirrors `RestClient::add_header_bearer`.
    pub fn add_header_bearer(&mut self, _token: &str) {}

    /// No-op, mirrors `RestClient::add_header_basic`.
    pub fn add_header_basic(&mut self, _username: &str, _password: &str) {}

    /// Sets the base URL used for display/logging by this mock.
    pub fn baseurl(&mut self, base: String) {
        self.baseurl = base;
    }

    /// Returns the `return_obj` of the first `GET` mock matching `path`.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error when no `GET` mock entry matches `path`.
    // signature imposée par l'API Rhai (vyvil-core.sdd)
    #[allow(clippy::needless_pass_by_value)]
    pub fn get(&mut self, path: String) -> RhaiRes<Map> {
        let found: Vec<HttpMockItem> = self
            .mocks
            .clone()
            .into_iter()
            .filter(|m| m.method == HttpMethod::Get && m.path == path)
            .collect();
        if found.is_empty() {
            Err(format!("Failed to find GET {path} in the Mock database").into())
        } else {
            Ok(found[0].clone().return_obj)
        }
    }

    /// Returns the `return_obj` of the first `HEAD` mock matching `path`.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error when no `HEAD` mock entry matches `path`.
    // signature imposée par l'API Rhai (vyvil-core.sdd)
    #[allow(clippy::needless_pass_by_value)]
    pub fn head(&mut self, path: String) -> RhaiRes<Map> {
        let found: Vec<HttpMockItem> = self
            .mocks
            .clone()
            .into_iter()
            .filter(|m| m.method == HttpMethod::Head && m.path == path)
            .collect();
        if found.is_empty() {
            Err(format!("Failed to find HEAD {path} in the Mock database").into())
        } else {
            Ok(found[0].clone().return_obj)
        }
    }

    /// Returns the `return_obj` of the first `PATCH` mock matching `path`; `_val` is ignored.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error when no `PATCH` mock entry matches `path`.
    // signature imposée par l'API Rhai (vyvil-core.sdd)
    #[allow(clippy::needless_pass_by_value)]
    pub fn patch(&mut self, path: String, _val: Dynamic) -> RhaiRes<Map> {
        let found: Vec<HttpMockItem> = self
            .mocks
            .clone()
            .into_iter()
            .filter(|m| m.method == HttpMethod::Patch && m.path == path)
            .collect();
        if found.is_empty() {
            Err(format!("Failed to find PATCH {path} in the Mock database").into())
        } else {
            Ok(found[0].clone().return_obj)
        }
    }

    /// Returns the `return_obj` of the first `PUT` mock matching `path`; `_val` is ignored.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error when no `PUT` mock entry matches `path`.
    // signature imposée par l'API Rhai (vyvil-core.sdd)
    #[allow(clippy::needless_pass_by_value)]
    pub fn put(&mut self, path: String, _val: Dynamic) -> RhaiRes<Map> {
        let found: Vec<HttpMockItem> = self
            .mocks
            .clone()
            .into_iter()
            .filter(|m| m.method == HttpMethod::Put && m.path == path)
            .collect();
        if found.is_empty() {
            Err(format!("Failed to find PUT {path} in the Mock database").into())
        } else {
            Ok(found[0].clone().return_obj)
        }
    }

    /// Returns the `return_obj` of the first `POST` mock matching `path`; `_val` is ignored.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error when no `POST` mock entry matches `path`.
    // signature imposée par l'API Rhai (vyvil-core.sdd)
    #[allow(clippy::needless_pass_by_value)]
    pub fn post(&mut self, path: String, _val: Dynamic) -> RhaiRes<Map> {
        let found: Vec<HttpMockItem> = self
            .mocks
            .clone()
            .into_iter()
            .filter(|m| m.method == HttpMethod::Post && m.path == path)
            .collect();
        if found.is_empty() {
            Err(format!("Failed to find POST {path} in the Mock database").into())
        } else {
            Ok(found[0].clone().return_obj)
        }
    }

    /// Returns the `return_obj` of the first `POST` mock matching `path`; `_val` is ignored.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error when no `POST` mock entry matches `path`.
    // signature imposée par l'API Rhai (vyvil-core.sdd)
    #[allow(clippy::needless_pass_by_value)]
    pub fn post_form(&mut self, path: String, _val: Map) -> RhaiRes<Map> {
        let found: Vec<HttpMockItem> = self
            .mocks
            .clone()
            .into_iter()
            .filter(|m| m.method == HttpMethod::Post && m.path == path)
            .collect();
        if found.is_empty() {
            Err(format!("Failed to find POST {path} in the Mock database").into())
        } else {
            Ok(found[0].clone().return_obj)
        }
    }

    /// Returns the `return_obj` of the first `DELETE` mock matching `path`.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error when no `DELETE` mock entry matches `path`.
    // signature imposée par l'API Rhai (vyvil-core.sdd)
    #[allow(clippy::needless_pass_by_value)]
    pub fn delete(&mut self, path: String) -> RhaiRes<Map> {
        let found: Vec<HttpMockItem> = self
            .mocks
            .clone()
            .into_iter()
            .filter(|m| m.method == HttpMethod::Delete && m.path == path)
            .collect();
        if found.is_empty() {
            Err(format!("Failed to find DELETE {path} in the Mock database").into())
        } else {
            Ok(found[0].clone().return_obj)
        }
    }

    /// Returns the `return_obj` of the first `DELETE` mock matching `path`; `_val` is ignored.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error when no `DELETE` mock entry matches `path`.
    // signature imposée par l'API Rhai (vyvil-core.sdd)
    #[allow(clippy::needless_pass_by_value)]
    pub fn delete_with_body(&mut self, path: String, _val: Dynamic) -> RhaiRes<Map> {
        let found: Vec<HttpMockItem> = self
            .mocks
            .clone()
            .into_iter()
            .filter(|m| m.method == HttpMethod::Delete && m.path == path)
            .collect();
        if found.is_empty() {
            Err(format!("Failed to find DELETE {path} in the Mock database").into())
        } else {
            Ok(found[0].clone().return_obj)
        }
    }
}

/// Registers the mock Rhai client (same surface as `crate::http::http_rhai_register`) on a Rhai
/// `engine`, every new client answering from the `mocks` list.
pub fn httpmock_rhai_register(engine: &mut Engine, mocks: Vec<HttpMockItem>) {
    let new = move |base: &str| -> RestClientMock { RestClientMock::new(base, mocks.clone()) };
    engine
        .register_type_with_name::<RestClientMock>("RestClient")
        .register_fn("new_http_client", new)
        .register_fn("headers_reset", RestClientMock::headers_reset)
        .register_fn("set_baseurl", RestClientMock::baseurl)
        .register_fn("set_server_ca", RestClientMock::set_server_ca)
        .register_fn("set_mtls_cert_key", RestClientMock::set_mtls)
        .register_fn("add_header", RestClientMock::add_header)
        .register_fn("add_header_json", RestClientMock::add_header_json)
        .register_fn("add_header_bearer", RestClientMock::add_header_bearer)
        .register_fn("add_header_basic", RestClientMock::add_header_basic)
        .register_fn("head", RestClientMock::head)
        .register_fn("get", RestClientMock::get)
        .register_fn("http_get", RestClientMock::get)
        .register_fn("delete", RestClientMock::delete)
        .register_fn("http_delete", RestClientMock::delete)
        .register_fn("delete_with_body", RestClientMock::delete_with_body)
        .register_fn("http_delete_with_body", RestClientMock::delete_with_body)
        .register_fn("patch", RestClientMock::patch)
        .register_fn("http_patch", RestClientMock::patch)
        .register_fn("post", RestClientMock::post)
        .register_fn("http_post", RestClientMock::post)
        .register_fn("put", RestClientMock::put)
        .register_fn("http_put", RestClientMock::put)
        .register_fn("post_form", RestClientMock::post_form)
        .register_fn("http_post_form", RestClientMock::post_form)
        .register_fn("headers_get", crate::http::headers_get)
        .register_fn("headers_has", crate::http::headers_has);
}
