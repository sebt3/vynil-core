//! HTTP client (`RestClient`) and free helpers (`http_get_yaml`, `headers_get`).
//!
//! Requires the `http` feature (which implies `rhai`). All requests use the global
//! client identity from [`crate::set_client_name`] as `User-Agent`.

use crate::{Error, Error::UnsupportedMethod, RhaiRes, rhai_err};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{Certificate, Client, Response};
use rhai::{Dynamic, Engine, Map};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use serde_yaml;
use tokio::runtime::Handle;
use tracing::{debug, warn};

/// Read verb accepted by [`RestClient::obj_read`].
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, JsonSchema, Default)]
pub enum ReadMethod {
    /// `GET` (the only supported read verb).
    #[default]
    Get,
}
/// Create verbs accepted by [`RestClient::obj_create`].
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, JsonSchema, Default)]
pub enum CreateMethod {
    /// `POST` to the collection path.
    #[default]
    Post,
    /// `PUT` to the collection path.
    Put,
}

/// Update verbs accepted by [`RestClient::obj_update`].
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, JsonSchema, Default)]
pub enum UpdateMethod {
    /// `PATCH` (server-side merge semantics).
    #[default]
    Patch,
    /// `PUT` (full replace).
    Put,
    /// `POST` to the object path.
    Post,
    /// No request: the input is echoed back unchanged.
    None,
}

/// Delete verbs accepted by [`RestClient::obj_delete`].
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, JsonSchema, Default)]
pub enum DeleteMethod {
    /// `DELETE` (the only supported delete verb).
    #[default]
    Delete,
}

/// Reqwest-based HTTP client with builder-style header / TLS configuration.
///
///
/// ```rust,no_run
/// # vynil_core::set_client_name(|| "my-app.example.com".into());
/// let mut c = vynil_core::http::RestClient::new("https://example.com");
/// c.add_header_bearer("token");
/// // let resp: serde_json::Value = c.json_get("api/v1/foo").unwrap();
/// ```
#[derive(Clone, Debug)]
pub struct RestClient {
    baseurl: String,
    headers: Map,
    server_ca: Option<String>,
    client_key: Option<String>,
    client_cert: Option<String>,
}

impl RestClient {
    /// Creates a client whose requests target `base` (joined as `base/path`).
    #[must_use]
    pub fn new(base: &str) -> Self {
        Self {
            baseurl: base.to_string(),
            headers: Map::new(),
            server_ca: None,
            client_cert: None,
            client_key: None,
        }
    }

    /// Sets the base URL (chainable).
    pub fn baseurl(&mut self, base: &str) -> &mut RestClient {
        self.baseurl = base.to_string();
        self
    }

    /// Sets a PEM CA certificate added as trust root (makes requests use rustls).
    pub fn set_server_ca(&mut self, ca: &str) {
        self.server_ca = Some(ca.to_string());
    }

    /// Sets the client PEM cert and key used to build the mTLS identity.
    pub fn set_mtls(&mut self, cert: &str, key: &str) {
        self.client_cert = Some(cert.to_string());
        self.client_key = Some(key.to_string());
    }

    /// [`Self::baseurl`] variant for Rhai (returns nothing).
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn baseurl_rhai(&mut self, base: String) {
        self.baseurl(base.as_str());
    }

    /// Clears all custom headers (chainable).
    pub fn headers_reset(&mut self) -> &mut RestClient {
        self.headers = Map::new();
        self
    }

    /// [`Self::headers_reset`] variant for Rhai (returns nothing).
    pub fn headers_reset_rhai(&mut self) {
        self.headers_reset();
    }

    /// Appends a header sent on every request (chainable; repeated names are kept).
    pub fn add_header(&mut self, key: &str, value: &str) -> &mut RestClient {
        self.headers
            .insert(key.to_string().into(), value.to_string().into());
        self
    }

    /// [`Self::add_header`] variant for Rhai (returns nothing).
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn add_header_rhai(&mut self, key: String, value: String) {
        self.add_header(key.as_str(), value.as_str());
    }

    /// Adds a JSON `Content-Type` unless one is already set (chainable).
    pub fn add_header_json_content(&mut self) -> &mut RestClient {
        if self
            .headers
            .clone()
            .into_iter()
            .any(|(c, _)| c == *"Content-Type")
        {
            self
        } else {
            self.add_header("Content-Type", "application/json; charset=utf-8")
        }
    }

    /// Adds a JSON `Accept` unless one is already set (chainable); logs current headers at debug
    /// level.
    pub fn add_header_json_accept(&mut self) -> &mut RestClient {
        for (key, val) in self.headers.clone() {
            debug!("RestClient.header: {:} {:}", key, val);
        }
        if self.headers.clone().into_iter().any(|(c, _)| c == *"Accept") {
            self
        } else {
            self.add_header("Accept", "application/json")
        }
    }

    /// Adds the JSON `Content-Type` and `Accept` headers.
    pub fn add_header_json(&mut self) {
        self.add_header_json_content().add_header_json_accept();
    }

    /// Sets an `Authorization: Bearer <token>` header.
    pub fn add_header_bearer(&mut self, token: &str) {
        self.add_header("Authorization", format!("Bearer {token}").as_str());
    }

    /// Sets an `Authorization: Basic <base64(username:password)>` header.
    pub fn add_header_basic(&mut self, username: &str, password: &str) {
        let hash = STANDARD.encode(format!("{username}:{password}"));
        self.add_header("Authorization", format!("Basic {hash}").as_str());
    }

    fn get_client(&mut self) -> std::result::Result<Client, reqwest::Error> {
        let five_sec = std::time::Duration::from_mins(5);
        let mut builder = Client::builder()
            .user_agent(crate::get_client_name())
            .timeout(five_sec);
        if let Some(ca) = &self.server_ca {
            let ca_cert = Certificate::from_pem(ca.as_bytes())?;
            builder = builder.add_root_certificate(ca_cert).use_rustls_tls();
        }
        if let (Some(key), Some(cert)) = (&self.client_key, &self.client_cert) {
            let cli_cert = format!("{key}\n{cert}");
            builder = builder
                .identity(reqwest::Identity::from_pem(cli_cert.as_bytes())?)
                .use_rustls_tls();
        }
        builder.build()
    }

    /// Sends a `GET` to `base/path` with the configured headers (blocks on the tokio runtime).
    ///
    /// # Errors
    ///
    /// Returns [`reqwest::Error`] if the client cannot be built (PEM/TLS issues) or the request
    /// fails; builder errors are also logged at warn level.
    pub fn http_get(&mut self, path: &str) -> std::result::Result<Response, reqwest::Error> {
        debug!("http_get '{}' ", format!("{}/{}", self.baseurl, path));
        match self.get_client() {
            Ok(client) => {
                let mut req = client.get(format!("{}/{}", self.baseurl, path));
                for (key, val) in self.headers.clone() {
                    req = req.header(key.to_string(), val.to_string());
                }
                tokio::task::block_in_place(|| Handle::current().block_on(async move { req.send().await }))
            }
            Err(e) => {
                if e.is_builder() {
                    warn!("CLIENT: {e:?}");
                }
                Err(e)
            }
        }
    }

    /// GETs `path` and returns the response body as text.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ReqwestError`] if the request or body read fails, [`Error::MethodFailed`]
    /// on a non-success status (with the body excerpt).
    pub fn body_get(&mut self, path: &str) -> crate::Result<String> {
        let response = self.http_get(path).map_err(Error::ReqwestError)?;
        if !response.status().is_success() {
            let status = response.status();
            let text = tokio::task::block_in_place(|| {
                Handle::current().block_on(async move { response.text().await })
            })
            .map_err(Error::ReqwestError)?;
            return Err(Error::MethodFailed(
                "Get".to_string(),
                status.as_u16(),
                format!(
                    "The server returned the error: {} {} | {text}",
                    status.as_str(),
                    status.canonical_reason().unwrap_or("unknown")
                ),
            ));
        }
        let text =
            tokio::task::block_in_place(|| Handle::current().block_on(async move { response.text().await }))
                .map_err(Error::ReqwestError)?;
        Ok(text)
    }

    /// GETs `path` and deserializes the response body as JSON.
    ///
    /// # Errors
    ///
    /// Forwards [`Self::body_get`] errors and returns [`Error::JsonError`] on a non-JSON body.
    pub fn json_get(&mut self, path: &str) -> crate::Result<Value> {
        let text = self.body_get(path)?;
        let json = serde_json::from_str(&text).map_err(Error::JsonError)?;
        Ok(json)
    }

    /// GET for Rhai: returns a map with `code` (i64), `headers` (for [`headers_get`]), `body`
    /// and `json` (parsed body, empty object when not valid JSON).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if the request fails (full cause chain via [`crate::error_chain`])
    /// or if the response body cannot be read.
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn rhai_get(&mut self, path: String) -> RhaiRes<Map> {
        let mut ret = Map::new();
        match self.http_get(path.as_str()) {
            Ok(result) => {
                ret.insert(
                    "code".to_string().into(),
                    Dynamic::from_int(result.status().as_u16().to_string().parse::<i64>().unwrap_or(0)),
                );
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        let headers = result
                            .headers()
                            .into_iter()
                            .map(|(key, val)| {
                                (
                                    key.as_str().to_string(),
                                    val.to_str().unwrap_or_default().to_string(),
                                )
                            })
                            .collect::<Vec<(String, String)>>();
                        let text = match result.text().await {
                            Ok(t) => t,
                            Err(e) => {
                                ret.insert(
                                    "body".to_string().into(),
                                    Dynamic::from(format!("Error reading response body: {e}")),
                                );
                                ret.insert("json".to_string().into(), Dynamic::from(json!({})));
                                ret.insert("headers".to_string().into(), Dynamic::from(headers));
                                return Err(format!("Error reading response body: {e}").into());
                            }
                        };
                        ret.insert(
                            "json".to_string().into(),
                            serde_json::from_str(&text).unwrap_or(Dynamic::from(json!({}))),
                        );
                        ret.insert("headers".to_string().into(), Dynamic::from(headers.clone()));
                        ret.insert("body".to_string().into(), Dynamic::from(text));
                        Ok(ret)
                    })
                })
            }
            Err(e) => Err(crate::error_chain(&e).into()),
        }
    }

    /// Sends a `HEAD` to `base/path` with the configured headers (blocks on the tokio runtime).
    ///
    /// # Errors
    ///
    /// Returns [`reqwest::Error`] if the client cannot be built (PEM/TLS issues) or the request
    /// fails; builder errors are also logged at warn level.
    pub fn http_head(&mut self, path: &str) -> std::result::Result<Response, reqwest::Error> {
        debug!("http_head '{}' ", format!("{}/{}", self.baseurl, path));
        match self.get_client() {
            Ok(client) => {
                let mut req = client.head(format!("{}/{}", self.baseurl, path));
                for (key, val) in self.headers.clone() {
                    req = req.header(key.to_string(), val.to_string());
                }
                tokio::task::block_in_place(|| Handle::current().block_on(async move { req.send().await }))
            }
            Err(e) => {
                if e.is_builder() {
                    warn!("CLIENT: {e:?}");
                }
                Err(e)
            }
        }
    }

    /// Returns the response headers of `path` as `(name, value)` pairs (the request is actually
    /// a `GET`, not a `HEAD`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::ReqwestError`] if the request fails, [`Error::MethodFailed`] on a
    /// non-success status.
    pub fn header_head(&mut self, path: &str) -> crate::Result<Vec<(String, String)>> {
        let response = self.http_get(path).map_err(Error::ReqwestError)?;
        if !response.status().is_success() {
            let status = response.status();
            return Err(Error::MethodFailed(
                "Get".to_string(),
                status.as_u16(),
                format!(
                    "The server returned the error: {} {}",
                    status.as_str(),
                    status.canonical_reason().unwrap_or("unknown")
                ),
            ));
        }
        Ok(response
            .headers()
            .into_iter()
            .map(|(key, val)| {
                (
                    key.as_str().to_string(),
                    val.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect())
    }

    /// `HEAD` for Rhai: returns a map with `code` (i64) and `headers` (for [`headers_get`]);
    /// the body is not fetched.
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if the request fails (full cause chain via [`crate::error_chain`]).
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn rhai_head(&mut self, path: String) -> RhaiRes<Map> {
        let mut ret = Map::new();
        match self.http_head(path.as_str()) {
            Ok(result) => {
                ret.insert(
                    "code".to_string().into(),
                    Dynamic::from_int(result.status().as_u16().to_string().parse::<i64>().unwrap_or(0)),
                );
                let headers = result
                    .headers()
                    .into_iter()
                    .map(|(key, val)| {
                        (
                            key.as_str().to_string(),
                            val.to_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect::<Vec<(String, String)>>();
                ret.insert("headers".to_string().into(), Dynamic::from(headers.clone()));
                Ok(ret)
            }
            Err(e) => Err(crate::error_chain(&e).into()),
        }
    }

    /// Sends a `PATCH` with `body` to `base/path` (blocks on the tokio runtime).
    ///
    /// # Errors
    ///
    /// Returns [`Error::ReqwestError`] if the client cannot be built (PEM/TLS issues) or the
    /// request fails; builder errors are also logged at warn level.
    pub fn http_patch(&mut self, path: &str, body: &str) -> crate::Result<Response> {
        debug!("http_patch '{}' ", format!("{}/{}", self.baseurl, path));
        match self.get_client() {
            Ok(client) => {
                let mut req = client
                    .patch(format!("{}/{}", self.baseurl, path))
                    .body(body.to_string());
                for (key, val) in self.headers.clone() {
                    req = req.header(key.to_string(), val.to_string());
                }
                tokio::task::block_in_place(|| Handle::current().block_on(async move { req.send().await }))
                    .map_err(Error::ReqwestError)
            }
            Err(e) => Err(Error::ReqwestError(e)),
        }
    }

    /// `PATCH`es `path` and returns the response body as text.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ReqwestError`] if the request or body read fails, [`Error::MethodFailed`]
    /// on a non-success status (with the body excerpt).
    pub fn body_patch(&mut self, path: &str, body: &str) -> crate::Result<String> {
        let response = self.http_patch(path, body)?;
        if !response.status().is_success() {
            let status = response.status();
            let text = tokio::task::block_in_place(|| {
                Handle::current().block_on(async move { response.text().await })
            })
            .map_err(Error::ReqwestError)?;
            return Err(Error::MethodFailed(
                "Patch".to_string(),
                status.as_u16(),
                format!(
                    "The server returned the error: {} {} | {text}",
                    status.as_str(),
                    status.canonical_reason().unwrap_or("unknown")
                ),
            ));
        }
        let text =
            tokio::task::block_in_place(|| Handle::current().block_on(async move { response.text().await }))
                .map_err(Error::ReqwestError)?;
        Ok(text)
    }

    /// `PATCH`es `path` with `input` serialized as JSON and returns the JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`Error::JsonError`] on serialization/deserialization failures, and forwards
    /// [`Self::body_patch`] errors.
    pub fn json_patch(&mut self, path: &str, input: &Value) -> crate::Result<Value> {
        let body = serde_json::to_string(input).map_err(Error::JsonError)?;
        let text = self.body_patch(path, body.as_str())?;
        let json = serde_json::from_str(&text).map_err(Error::JsonError)?;
        Ok(json)
    }

    /// `PATCH` for Rhai: `val` is sent as-is when it is a string, JSON-serialized otherwise;
    /// returns the same map shape as [`Self::rhai_get`].
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if the body cannot be serialized, the request fails (full cause
    /// chain via [`crate::error_chain`]) or the response body cannot be read.
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn rhai_patch(&mut self, path: String, val: Dynamic) -> RhaiRes<Map> {
        let body = if val.is_string() {
            val.to_string()
        } else {
            match serde_json::to_string(&val) {
                Ok(s) => s,
                Err(e) => return Err(format!("Failed to serialize body: {e}").into()),
            }
        };
        let mut ret = Map::new();
        match self.http_patch(path.as_str(), &body) {
            Ok(result) => {
                ret.insert(
                    "code".to_string().into(),
                    Dynamic::from_int(result.status().as_u16().to_string().parse::<i64>().unwrap_or(0)),
                );
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        let headers = result
                            .headers()
                            .into_iter()
                            .map(|(key, val)| {
                                (
                                    key.as_str().to_string(),
                                    val.to_str().unwrap_or_default().to_string(),
                                )
                            })
                            .collect::<Vec<(String, String)>>();
                        let text = match result.text().await {
                            Ok(t) => t,
                            Err(e) => {
                                ret.insert(
                                    "body".to_string().into(),
                                    Dynamic::from(format!("Error reading response body: {e}")),
                                );
                                ret.insert("json".to_string().into(), Dynamic::from(json!({})));
                                ret.insert("headers".to_string().into(), Dynamic::from(headers));
                                return Err(format!("Error reading response body: {e}").into());
                            }
                        };
                        ret.insert(
                            "json".to_string().into(),
                            serde_json::from_str(&text).unwrap_or(Dynamic::from(json!({}))),
                        );
                        ret.insert("headers".to_string().into(), Dynamic::from(headers.clone()));
                        ret.insert("body".to_string().into(), Dynamic::from(text));
                        Ok(ret)
                    })
                })
            }
            Err(e) => Err(crate::error_chain(&e).into()),
        }
    }

    /// Sends a `PUT` with `body` to `base/path` (blocks on the tokio runtime).
    ///
    /// # Errors
    ///
    /// Returns [`Error::ReqwestError`] if the client cannot be built (PEM/TLS issues) or the
    /// request fails; builder errors are also logged at warn level.
    pub fn http_put(&mut self, path: &str, body: &str) -> crate::Result<Response> {
        debug!("http_put '{}' ", format!("{}/{}", self.baseurl, path));
        match self.get_client() {
            Ok(client) => {
                let mut req = client
                    .put(format!("{}/{}", self.baseurl, path))
                    .body(body.to_string());
                for (key, val) in self.headers.clone() {
                    req = req.header(key.to_string(), val.to_string());
                }
                tokio::task::block_in_place(|| Handle::current().block_on(async move { req.send().await }))
                    .map_err(Error::ReqwestError)
            }
            Err(e) => Err(Error::ReqwestError(e)),
        }
    }

    /// `PUT`s `path` and returns the response body as text.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ReqwestError`] if the request or body read fails, [`Error::MethodFailed`]
    /// on a non-success status (with the body excerpt).
    pub fn body_put(&mut self, path: &str, body: &str) -> crate::Result<String> {
        let response = self.http_put(path, body)?;
        if !response.status().is_success() {
            let status = response.status();
            let text = tokio::task::block_in_place(|| {
                Handle::current().block_on(async move { response.text().await })
            })
            .map_err(Error::ReqwestError)?;
            return Err(Error::MethodFailed(
                "Put".to_string(),
                status.as_u16(),
                format!(
                    "The server returned the error: {} {} | {text}",
                    status.as_str(),
                    status.canonical_reason().unwrap_or("unknown")
                ),
            ));
        }
        let text =
            tokio::task::block_in_place(|| Handle::current().block_on(async move { response.text().await }))
                .map_err(Error::ReqwestError)?;
        Ok(text)
    }

    /// `PUT`s `path` with `input` serialized as JSON and returns the JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`Error::JsonError`] on serialization/deserialization failures, and forwards
    /// [`Self::body_put`] errors.
    pub fn json_put(&mut self, path: &str, input: &Value) -> crate::Result<Value> {
        let body = serde_json::to_string(input).map_err(Error::JsonError)?;
        let text = self.body_put(path, body.as_str())?;
        let json = serde_json::from_str(&text).map_err(Error::JsonError)?;
        Ok(json)
    }

    /// `PUT` for Rhai: `val` is sent as-is when it is a string, JSON-serialized otherwise;
    /// returns the same map shape as [`Self::rhai_get`].
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if the body cannot be serialized, the request fails (full cause
    /// chain via [`crate::error_chain`]) or the response body cannot be read.
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn rhai_put(&mut self, path: String, val: Dynamic) -> RhaiRes<Map> {
        let body = if val.is_string() {
            val.to_string()
        } else {
            match serde_json::to_string(&val) {
                Ok(s) => s,
                Err(e) => return Err(format!("Failed to serialize body: {e}").into()),
            }
        };
        let mut ret = Map::new();
        match self.http_put(path.as_str(), &body) {
            Ok(result) => {
                ret.insert(
                    "code".to_string().into(),
                    Dynamic::from_int(result.status().as_u16().to_string().parse::<i64>().unwrap_or(0)),
                );
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        let headers = result
                            .headers()
                            .into_iter()
                            .map(|(key, val)| {
                                (
                                    key.as_str().to_string(),
                                    val.to_str().unwrap_or_default().to_string(),
                                )
                            })
                            .collect::<Vec<(String, String)>>();
                        let text = match result.text().await {
                            Ok(t) => t,
                            Err(e) => {
                                ret.insert(
                                    "body".to_string().into(),
                                    Dynamic::from(format!("Error reading response body: {e}")),
                                );
                                ret.insert("json".to_string().into(), Dynamic::from(json!({})));
                                ret.insert("headers".to_string().into(), Dynamic::from(headers));
                                return Err(format!("Error reading response body: {e}").into());
                            }
                        };
                        ret.insert(
                            "json".to_string().into(),
                            serde_json::from_str(&text).unwrap_or(Dynamic::from(json!({}))),
                        );
                        ret.insert("headers".to_string().into(), Dynamic::from(headers.clone()));
                        ret.insert("body".to_string().into(), Dynamic::from(text));
                        Ok(ret)
                    })
                })
            }
            Err(e) => Err(crate::error_chain(&e).into()),
        }
    }

    /// Sends a `POST` with `body` to `base/path` (blocks on the tokio runtime).
    ///
    /// # Errors
    ///
    /// Returns [`Error::ReqwestError`] if the client cannot be built (PEM/TLS issues) or the
    /// request fails; builder errors are also logged at warn level.
    pub fn http_post(&mut self, path: &str, body: &str) -> crate::Result<Response> {
        debug!("http_post '{}' ", format!("{}/{}", self.baseurl, path));
        match self.get_client() {
            Ok(client) => {
                let mut req = client
                    .post(format!("{}/{}", self.baseurl, path))
                    .body(body.to_string());
                for (key, val) in self.headers.clone() {
                    req = req.header(key.to_string(), val.to_string());
                }
                tokio::task::block_in_place(|| Handle::current().block_on(async move { req.send().await }))
                    .map_err(Error::ReqwestError)
            }
            Err(e) => Err(Error::ReqwestError(e)),
        }
    }

    /// `POST`s `path` and returns the response body as text.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ReqwestError`] if the request or body read fails, [`Error::MethodFailed`]
    /// on a non-success status (with the body excerpt).
    pub fn body_post(&mut self, path: &str, body: &str) -> crate::Result<String> {
        let response = self.http_post(path, body)?;
        if !response.status().is_success() {
            let status = response.status();
            let text = tokio::task::block_in_place(|| {
                Handle::current().block_on(async move { response.text().await })
            })
            .map_err(Error::ReqwestError)?;
            return Err(Error::MethodFailed(
                "Post".to_string(),
                status.as_u16(),
                format!(
                    "The server returned the error: {} {} | {text}",
                    status.as_str(),
                    status.canonical_reason().unwrap_or("unknown")
                ),
            ));
        }
        let text =
            tokio::task::block_in_place(|| Handle::current().block_on(async move { response.text().await }))
                .map_err(Error::ReqwestError)?;
        Ok(text)
    }

    /// `POST`s `path` with `input` serialized as JSON and returns the JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`Error::JsonError`] on serialization/deserialization failures, and forwards
    /// [`Self::body_post`] errors.
    pub fn json_post(&mut self, path: &str, input: &Value) -> crate::Result<Value> {
        let body = serde_json::to_string(input).map_err(Error::JsonError)?;
        let text = self.body_post(path, body.as_str())?;
        let json = serde_json::from_str(&text).map_err(Error::JsonError)?;
        Ok(json)
    }

    /// `POST` for Rhai: `val` is sent as-is when it is a string, JSON-serialized otherwise;
    /// returns the same map shape as [`Self::rhai_get`].
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if the body cannot be serialized, the request fails (full cause
    /// chain via [`crate::error_chain`]) or the response body cannot be read.
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn rhai_post(&mut self, path: String, val: Dynamic) -> RhaiRes<Map> {
        let body = if val.is_string() {
            val.to_string()
        } else {
            match serde_json::to_string(&val) {
                Ok(s) => s,
                Err(e) => return Err(format!("Failed to serialize body: {e}").into()),
            }
        };
        let mut ret = Map::new();
        match self.http_post(path.as_str(), &body) {
            Ok(result) => {
                ret.insert(
                    "code".to_string().into(),
                    Dynamic::from_int(result.status().as_u16().to_string().parse::<i64>().unwrap_or(0)),
                );
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        let headers = result
                            .headers()
                            .into_iter()
                            .map(|(key, val)| {
                                (
                                    key.as_str().to_string(),
                                    val.to_str().unwrap_or_default().to_string(),
                                )
                            })
                            .collect::<Vec<(String, String)>>();
                        let text = match result.text().await {
                            Ok(t) => t,
                            Err(e) => {
                                ret.insert(
                                    "body".to_string().into(),
                                    Dynamic::from(format!("Error reading response body: {e}")),
                                );
                                ret.insert("json".to_string().into(), Dynamic::from(json!({})));
                                ret.insert("headers".to_string().into(), Dynamic::from(headers));
                                return Err(format!("Error reading response body: {e}").into());
                            }
                        };
                        ret.insert(
                            "json".to_string().into(),
                            serde_json::from_str(&text).unwrap_or(Dynamic::from(json!({}))),
                        );
                        ret.insert("headers".to_string().into(), Dynamic::from(headers.clone()));
                        ret.insert("body".to_string().into(), Dynamic::from(text));
                        Ok(ret)
                    })
                })
            }
            Err(e) => Err(crate::error_chain(&e).into()),
        }
    }

    /// Sends a urlencoded form `POST` to `base/path` (a custom `Content-Type` header is
    /// deliberately skipped; blocks on the tokio runtime).
    ///
    /// # Errors
    ///
    /// Returns [`Error::ReqwestError`] if the client cannot be built (PEM/TLS issues) or the
    /// request fails; builder errors are also logged at warn level.
    pub fn http_post_form(&mut self, path: &str, params: &[(String, String)]) -> crate::Result<Response> {
        debug!("http_post_form '{}' ", format!("{}/{}", self.baseurl, path));
        match self.get_client() {
            Ok(client) => {
                let mut req = client.post(format!("{}/{}", self.baseurl, path)).form(params);
                for (key, val) in self.headers.clone() {
                    if key.as_str() != "Content-Type" {
                        req = req.header(key.to_string(), val.to_string());
                    }
                }
                tokio::task::block_in_place(|| Handle::current().block_on(async move { req.send().await }))
                    .map_err(Error::ReqwestError)
            }
            Err(e) => Err(Error::ReqwestError(e)),
        }
    }

    /// Form `POST` for Rhai: map entries become urlencoded params (values via `to_string`);
    /// returns the same map shape as [`Self::rhai_get`].
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if the request fails (full cause chain via [`crate::error_chain`])
    /// or the response body cannot be read.
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn rhai_post_form(&mut self, path: String, val: Map) -> RhaiRes<Map> {
        let params: Vec<(String, String)> = val
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let mut ret = Map::new();
        match self.http_post_form(path.as_str(), &params) {
            Ok(result) => {
                ret.insert(
                    "code".to_string().into(),
                    Dynamic::from_int(result.status().as_u16().to_string().parse::<i64>().unwrap_or(0)),
                );
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        let headers = result
                            .headers()
                            .into_iter()
                            .map(|(key, val)| {
                                (
                                    key.as_str().to_string(),
                                    val.to_str().unwrap_or_default().to_string(),
                                )
                            })
                            .collect::<Vec<(String, String)>>();
                        let text = match result.text().await {
                            Ok(t) => t,
                            Err(e) => {
                                ret.insert(
                                    "body".to_string().into(),
                                    Dynamic::from(format!("Error reading response body: {e}")),
                                );
                                ret.insert("json".to_string().into(), Dynamic::from(json!({})));
                                ret.insert("headers".to_string().into(), Dynamic::from(headers));
                                return Err(format!("Error reading response body: {e}").into());
                            }
                        };
                        ret.insert(
                            "json".to_string().into(),
                            serde_json::from_str(&text).unwrap_or(Dynamic::from(json!({}))),
                        );
                        ret.insert("headers".to_string().into(), Dynamic::from(headers.clone()));
                        ret.insert("body".to_string().into(), Dynamic::from(text));
                        Ok(ret)
                    })
                })
            }
            Err(e) => Err(crate::error_chain(&e).into()),
        }
    }

    /// Sends a `DELETE` to `base/path` (blocks on the tokio runtime).
    ///
    /// # Errors
    ///
    /// Returns [`Error::ReqwestError`] if the client cannot be built (PEM/TLS issues) or the
    /// request fails; builder errors are also logged at warn level.
    pub fn http_delete(&mut self, path: &str) -> crate::Result<Response> {
        debug!("http_delete '{}' ", format!("{}/{}", self.baseurl, path));
        match self.get_client() {
            Ok(client) => {
                let mut req = client.delete(format!("{}/{}", self.baseurl, path));
                for (key, val) in self.headers.clone() {
                    req = req.header(key.to_string(), val.to_string());
                }
                tokio::task::block_in_place(|| Handle::current().block_on(async move { req.send().await }))
                    .map_err(Error::ReqwestError)
            }
            Err(e) => Err(Error::ReqwestError(e)),
        }
    }

    /// `DELETE`s `path` and returns the response body as text; `404` is treated as success
    /// (idempotent delete).
    ///
    /// # Errors
    ///
    /// Returns [`Error::ReqwestError`] if the request or body read fails, [`Error::MethodFailed`]
    /// on a failure status other than `404` (with the body excerpt).
    pub fn body_delete(&mut self, path: &str) -> crate::Result<String> {
        let response = self.http_delete(path)?;
        if !response.status().is_success() && response.status() != reqwest::StatusCode::NOT_FOUND {
            let status = response.status();
            let text = tokio::task::block_in_place(|| {
                Handle::current().block_on(async move { response.text().await })
            })
            .map_err(Error::ReqwestError)?;
            return Err(Error::MethodFailed(
                "Delete".to_string(),
                status.as_u16(),
                format!(
                    "The server returned the error: {} {} | {text}",
                    status.as_str(),
                    status.canonical_reason().unwrap_or("unknown")
                ),
            ));
        }
        let text =
            tokio::task::block_in_place(|| Handle::current().block_on(async move { response.text().await }))
                .map_err(Error::ReqwestError)?;
        Ok(text)
    }

    /// `DELETE`s `path` and returns the JSON response; a non-JSON body is wrapped as
    /// `{"body": <text>}`.
    ///
    /// # Errors
    ///
    /// Forwards [`Self::body_delete`] errors.
    pub fn json_delete(&mut self, path: &str) -> crate::Result<Value> {
        let text = self.body_delete(path)?;
        let json =
            serde_json::from_str(&text).or_else(|_| Ok::<serde_json::Value, Error>(json!({"body": text})))?;
        Ok(json)
    }

    /// `DELETE` for Rhai: returns the same map shape as [`Self::rhai_get`] (`404` is not an
    /// error at the transport level but surfaces in `code`).
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if the request fails (full cause chain via [`crate::error_chain`])
    /// or the response body cannot be read.
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn rhai_delete(&mut self, path: String) -> RhaiRes<Map> {
        let mut ret = Map::new();
        match self.http_delete(path.as_str()) {
            Ok(result) => {
                ret.insert(
                    "code".to_string().into(),
                    Dynamic::from_int(result.status().as_u16().to_string().parse::<i64>().unwrap_or(0)),
                );
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        let headers = result
                            .headers()
                            .into_iter()
                            .map(|(key, val)| {
                                (
                                    key.as_str().to_string(),
                                    val.to_str().unwrap_or_default().to_string(),
                                )
                            })
                            .collect::<Vec<(String, String)>>();
                        let text = match result.text().await {
                            Ok(t) => t,
                            Err(e) => {
                                ret.insert(
                                    "body".to_string().into(),
                                    Dynamic::from(format!("Error reading response body: {e}")),
                                );
                                ret.insert("json".to_string().into(), Dynamic::from(json!({})));
                                ret.insert("headers".to_string().into(), Dynamic::from(headers));
                                return Err(format!("Error reading response body: {e}").into());
                            }
                        };
                        ret.insert(
                            "json".to_string().into(),
                            serde_json::from_str(&text).unwrap_or(Dynamic::from(json!({}))),
                        );
                        ret.insert("headers".to_string().into(), Dynamic::from(headers.clone()));
                        ret.insert("body".to_string().into(), Dynamic::from(text));
                        Ok(ret)
                    })
                })
            }
            Err(e) => Err(crate::error_chain(&e).into()),
        }
    }

    /// Reads `path` (suffixed with `/<key>` when `key` is not empty) as JSON.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMethod`] for a method other than [`ReadMethod::Get`], and
    /// forwards [`Self::json_get`] errors.
    #[allow(clippy::needless_pass_by_value)] // signature publique exposée sur crates.io (vyvil-core.sdd)
    pub fn obj_read(&mut self, method: ReadMethod, path: &str, key: &str) -> crate::Result<Value> {
        let full_path = if key.is_empty() {
            path.to_string()
        } else {
            format!("{path}/{key}")
        };
        if method == ReadMethod::Get {
            self.json_get(&full_path)
        } else {
            Err(UnsupportedMethod)
        }
    }

    /// Creates `input` at `path` using the given verb.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMethod`] for a method other than Post/Put, and forwards the
    /// [`Self::json_post`] / [`Self::json_put`] errors.
    #[allow(clippy::needless_pass_by_value)] // signature publique exposée sur crates.io (vyvil-core.sdd)
    pub fn obj_create(&mut self, method: CreateMethod, path: &str, input: &Value) -> crate::Result<Value> {
        if method == CreateMethod::Post {
            self.json_post(path, input)
        } else if method == CreateMethod::Put {
            self.json_put(path, input)
        } else {
            Err(UnsupportedMethod)
        }
    }

    /// Updates `path` (suffixed with `/<key>`, plus trailing slash when `use_slash`);
    /// [`UpdateMethod::None`] echoes `input` without any request.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMethod`] for an unknown method value, and forwards the
    /// patch/put/post JSON errors.
    #[allow(clippy::needless_pass_by_value)] // signature publique exposée sur crates.io (vyvil-core.sdd)
    pub fn obj_update(
        &mut self,
        method: UpdateMethod,
        path: &str,
        key: &str,
        input: &Value,
        use_slash: bool,
    ) -> crate::Result<Value> {
        let full_path = if key.is_empty() {
            path.to_string()
        } else if use_slash {
            format!("{path}/{key}/")
        } else {
            format!("{path}/{key}")
        };
        if method == UpdateMethod::Patch {
            self.json_patch(&full_path, input)
        } else if method == UpdateMethod::Put {
            self.json_put(&full_path, input)
        } else if method == UpdateMethod::Post {
            self.json_post(&full_path, input)
        } else if method == UpdateMethod::None {
            Ok(input.clone())
        } else {
            Err(UnsupportedMethod)
        }
    }

    /// Deletes `path` (suffixed with `/<key>` when `key` is not empty).
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMethod`] for a method other than [`DeleteMethod::Delete`], and
    /// forwards [`Self::json_delete`] errors.
    #[allow(clippy::needless_pass_by_value)] // signature publique exposée sur crates.io (vyvil-core.sdd)
    pub fn obj_delete(&mut self, method: DeleteMethod, path: &str, key: &str) -> crate::Result<Value> {
        let full_path = if key.is_empty() {
            path.to_string()
        } else {
            format!("{path}/{key}")
        };
        if method == DeleteMethod::Delete {
            self.json_delete(&full_path)
        } else {
            Err(UnsupportedMethod)
        }
    }

    /// Sends a `DELETE` carrying `body` to `base/path` (blocks on the tokio runtime).
    ///
    /// # Errors
    ///
    /// Returns [`Error::ReqwestError`] if the client cannot be built (PEM/TLS issues) or the
    /// request fails; builder errors are also logged at warn level.
    pub fn http_delete_with_body(&mut self, path: &str, body: &str) -> crate::Result<Response> {
        debug!(
            "http_delete_with_body '{}' ",
            format!("{}/{}", self.baseurl, path)
        );
        match self.get_client() {
            Ok(client) => {
                let mut req = client
                    .delete(format!("{}/{}", self.baseurl, path))
                    .body(body.to_string());
                for (key, val) in self.headers.clone() {
                    req = req.header(key.to_string(), val.to_string());
                }
                tokio::task::block_in_place(|| Handle::current().block_on(async move { req.send().await }))
                    .map_err(Error::ReqwestError)
            }
            Err(e) => Err(Error::ReqwestError(e)),
        }
    }

    /// `DELETE`s `path` with `body` and returns the response text; `404` is treated as success.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ReqwestError`] if the request or body read fails, [`Error::MethodFailed`]
    /// on a failure status other than `404` (with the body excerpt).
    pub fn body_delete_with_body(&mut self, path: &str, body: &str) -> crate::Result<String> {
        let response = self.http_delete_with_body(path, body)?;
        if !response.status().is_success() && response.status() != reqwest::StatusCode::NOT_FOUND {
            let status = response.status();
            let text = tokio::task::block_in_place(|| {
                Handle::current().block_on(async move { response.text().await })
            })
            .map_err(Error::ReqwestError)?;
            return Err(Error::MethodFailed(
                "Delete".to_string(),
                status.as_u16(),
                format!(
                    "The server returned the error: {} {} | {text}",
                    status.as_str(),
                    status.canonical_reason().unwrap_or("unknown")
                ),
            ));
        }
        let text =
            tokio::task::block_in_place(|| Handle::current().block_on(async move { response.text().await }))
                .map_err(Error::ReqwestError)?;
        Ok(text)
    }

    /// `DELETE`s `path` with `input` serialized as JSON; a non-JSON response is wrapped as
    /// `{"body": <text>}`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::JsonError`] on serialization failures, and forwards
    /// [`Self::body_delete_with_body`] errors.
    pub fn json_delete_with_body(&mut self, path: &str, input: &Value) -> crate::Result<Value> {
        let body = serde_json::to_string(input).map_err(Error::JsonError)?;
        let text = self.body_delete_with_body(path, body.as_str())?;
        let json =
            serde_json::from_str(&text).or_else(|_| Ok::<serde_json::Value, Error>(json!({"body": text})))?;
        Ok(json)
    }

    /// `DELETE`-with-body for Rhai: `val` is sent as-is when it is a string, JSON-serialized
    /// otherwise; returns the same map shape as [`Self::rhai_get`].
    ///
    /// # Errors
    ///
    /// Returns a Rhai error if the body cannot be serialized, the request fails (full cause
    /// chain via [`crate::error_chain`]) or the response body cannot be read.
    #[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
    pub fn rhai_delete_with_body(&mut self, path: String, val: Dynamic) -> RhaiRes<Map> {
        let body = if val.is_string() {
            val.to_string()
        } else {
            match serde_json::to_string(&val) {
                Ok(s) => s,
                Err(e) => return Err(format!("Failed to serialize body: {e}").into()),
            }
        };
        let mut ret = Map::new();
        match self.http_delete_with_body(path.as_str(), &body) {
            Ok(result) => {
                ret.insert(
                    "code".to_string().into(),
                    Dynamic::from_int(result.status().as_u16().to_string().parse::<i64>().unwrap_or(0)),
                );
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        let headers = result
                            .headers()
                            .into_iter()
                            .map(|(key, val)| {
                                (
                                    key.as_str().to_string(),
                                    val.to_str().unwrap_or_default().to_string(),
                                )
                            })
                            .collect::<Vec<(String, String)>>();
                        let text = match result.text().await {
                            Ok(t) => t,
                            Err(e) => {
                                ret.insert(
                                    "body".to_string().into(),
                                    Dynamic::from(format!("Error reading response body: {e}")),
                                );
                                ret.insert("json".to_string().into(), Dynamic::from(json!({})));
                                ret.insert("headers".to_string().into(), Dynamic::from(headers));
                                return Err(format!("Error reading response body: {e}").into());
                            }
                        };
                        ret.insert(
                            "json".to_string().into(),
                            serde_json::from_str(&text).unwrap_or(Dynamic::from(json!({}))),
                        );
                        ret.insert("headers".to_string().into(), Dynamic::from(headers.clone()));
                        ret.insert("body".to_string().into(), Dynamic::from(text));
                        Ok(ret)
                    })
                })
            }
            Err(e) => Err(crate::error_chain(&e).into()),
        }
    }

    /// Deletes `path` carrying `input` as a JSON request body.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMethod`] for a method other than [`DeleteMethod::Delete`], and
    /// forwards [`Self::json_delete_with_body`] errors.
    #[allow(clippy::needless_pass_by_value)] // signature publique exposée sur crates.io (vyvil-core.sdd)
    pub fn obj_delete_with_body(
        &mut self,
        method: DeleteMethod,
        path: &str,
        input: &Value,
    ) -> crate::Result<Value> {
        if method == DeleteMethod::Delete {
            self.json_delete_with_body(path, input)
        } else {
            Err(UnsupportedMethod)
        }
    }
}

/// Case-insensitive lookup of a single response header by name. `headers` is the opaque
/// `Vec<(String, String)>` returned as the `headers` field of `get`/`post`/... results — it
/// carries no rhai-visible indexing or iteration of its own, so this is the only way to read
/// a specific header from a script. Returns `()` when the header is absent.
#[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
pub fn headers_get(headers: Vec<(String, String)>, name: String) -> Dynamic {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&name))
        .map_or(Dynamic::UNIT, |(_, v)| Dynamic::from(v.clone()))
}

/// Case-insensitive presence check for a response header by name. See [`headers_get`].
#[must_use]
#[allow(clippy::needless_pass_by_value)] // signature imposée par l'API Rhai (vyvil-core.sdd)
pub fn headers_has(headers: Vec<(String, String)>, name: String) -> bool {
    headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(&name))
}

/// GETs `url`, optionally authenticated (`bearer` / `basic` `credential`), and returns the YAML
/// body as a Rhai value.
///
/// # Errors
///
/// Returns [`Error::Other`] on an invalid auth header, a client builder failure or a non-success
/// status (`SCAN-HTTP-001`), [`Error::ReqwestError`] on request/body-read failure, and
/// [`Error::YamlError`] / [`Error::SerializationError`] when the body is not valid YAML/JSON.
pub fn http_get_yaml(url: String, auth_type: String, credential: String) -> RhaiRes<Dynamic> {
    tokio::task::block_in_place(|| {
        Handle::current().block_on(async move {
            let mut headers = reqwest::header::HeaderMap::new();
            match auth_type.as_str() {
                "bearer" => {
                    let value = format!("Bearer {credential}")
                        .parse()
                        .map_err(|e| Error::Other(format!("invalid bearer credential: {e}")))?;
                    headers.insert(reqwest::header::AUTHORIZATION, value);
                }
                "basic" => {
                    let encoded = STANDARD.encode(&credential);
                    let value = format!("Basic {encoded}")
                        .parse()
                        .map_err(|e| Error::Other(format!("invalid basic credential: {e}")))?;
                    headers.insert(reqwest::header::AUTHORIZATION, value);
                }
                _ => {}
            }
            let client = reqwest::Client::builder()
                .default_headers(headers)
                .timeout(std::time::Duration::from_mins(5))
                .build()
                .map_err(|e| Error::Other(e.to_string()))?;
            let response = client.get(&url).send().await.map_err(Error::ReqwestError)?;
            if !response.status().is_success() {
                return Err(Error::Other(format!(
                    "SCAN-HTTP-001: HTTP {} for {}",
                    response.status(),
                    url
                )));
            }
            let body = response.text().await.map_err(Error::ReqwestError)?;
            let value: serde_yaml::Value =
                serde_yaml::from_str(&body).map_err(|e| Error::YamlError(e.to_string()))?;
            let json = serde_json::to_string(&value).map_err(Error::SerializationError)?;
            serde_json::from_str::<Dynamic>(&json).map_err(Error::SerializationError)
        })
    })
    .map_err(rhai_err)
}

/// Registers `RestClient` (`new_http_client`/`new_client`), the HTTP verbs and the header
/// helpers (`http_get_yaml`, `headers_get`, `headers_has`) on a Rhai engine.
pub fn http_rhai_register(engine: &mut Engine) {
    engine
        .register_type_with_name::<RestClient>("RestClient")
        .register_fn("new_http_client", RestClient::new)
        .register_fn("new_client", RestClient::new)
        .register_fn("headers_reset", RestClient::headers_reset_rhai)
        .register_fn("set_baseurl", RestClient::baseurl_rhai)
        .register_fn("set_server_ca", RestClient::set_server_ca)
        .register_fn("set_mtls_cert_key", RestClient::set_mtls)
        .register_fn("add_header", RestClient::add_header_rhai)
        .register_fn("add_header_json", RestClient::add_header_json)
        .register_fn("add_header_bearer", RestClient::add_header_bearer)
        .register_fn("add_header_basic", RestClient::add_header_basic)
        .register_fn("head", RestClient::rhai_head)
        .register_fn("get", RestClient::rhai_get)
        .register_fn("http_get", RestClient::rhai_get)
        .register_fn("delete", RestClient::rhai_delete)
        .register_fn("http_delete", RestClient::rhai_delete)
        .register_fn("delete_with_body", RestClient::rhai_delete_with_body)
        .register_fn("http_delete_with_body", RestClient::rhai_delete_with_body)
        .register_fn("patch", RestClient::rhai_patch)
        .register_fn("http_patch", RestClient::rhai_patch)
        .register_fn("post", RestClient::rhai_post)
        .register_fn("http_post", RestClient::rhai_post)
        .register_fn("put", RestClient::rhai_put)
        .register_fn("http_put", RestClient::rhai_put)
        .register_fn("post_form", RestClient::rhai_post_form)
        .register_fn("http_post_form", RestClient::rhai_post_form)
        .register_fn("http_get_yaml", http_get_yaml)
        .register_fn("headers_get", headers_get)
        .register_fn("headers_has", headers_has);
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path},
    };

    #[tokio::test(flavor = "multi_thread")]
    async fn rhai_get_on_connection_failure_surfaces_real_cause() {
        // Regression test for a JukeBox `Gitlab` source whose scan silently failed with the
        // opaque `error sending request for url (...)` message and nothing else: connection-level
        // reqwest errors (DNS, TLS, refused, timeout) only expose their real cause via `source()`,
        // not `Display`. `rhai_get`'s error branch must walk that chain instead of `format!("{e}")`,
        // or an operator has no way to tell a network/proxy problem from a GitLab API rejection.
        crate::set_client_name(|| "vynil-core-tests".to_string());
        let mut client = RestClient::new("http://127.0.0.1:1");
        let err = client
            .rhai_get("api/v4/projects?search=box&per_page=20".to_string())
            .expect_err("port 1 should refuse the connection");
        let message = err.to_string();
        assert!(
            message.contains("error sending request for url (http://127.0.0.1:1/api/v4/projects"),
            "message should still carry reqwest's own text: {message}"
        );
        assert!(
            message.contains("Connection refused") || message.contains("refused"),
            "message must carry the real transport-level cause, not just the opaque reqwest \
             wrapper, or the bug is back: {message}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_http_get_yaml_ok() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.yaml"))
            .respond_with(ResponseTemplate::new(200).set_body_string("packages:\n  - name: test\n"))
            .mount(&server)
            .await;

        let result = http_get_yaml(
            format!("{}/index.yaml", server.uri()),
            String::new(),
            String::new(),
        );
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        let d = result.unwrap();
        assert!(d.is_map(), "expected map Dynamic");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_http_get_yaml_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing.yaml"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let result = http_get_yaml(
            format!("{}/missing.yaml", server.uri()),
            String::new(),
            String::new(),
        );
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("SCAN-HTTP-001"),
            "error should contain SCAN-HTTP-001: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_http_get_yaml_bearer() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.yaml"))
            .and(header("authorization", "Bearer token123"))
            .respond_with(ResponseTemplate::new(200).set_body_string("key: value\n"))
            .mount(&server)
            .await;

        let result = http_get_yaml(
            format!("{}/index.yaml", server.uri()),
            "bearer".to_string(),
            "token123".to_string(),
        );
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_http_get_yaml_basic() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.yaml"))
            .and(header("authorization", "Basic dXNlcjpwYXNz"))
            .respond_with(ResponseTemplate::new(200).set_body_string("key: value\n"))
            .mount(&server)
            .await;

        let result = http_get_yaml(
            format!("{}/index.yaml", server.uri()),
            "basic".to_string(),
            "user:pass".to_string(),
        );
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }

    #[test]
    fn test_headers_get_finds_value_case_insensitively() {
        let headers = vec![("X-Total-Pages".to_string(), "3".to_string())];
        let found = headers_get(headers, "x-total-pages".to_string());
        assert_eq!(found.into_string().unwrap(), "3");
    }

    #[test]
    fn test_headers_get_missing_returns_unit() {
        let headers = vec![("content-type".to_string(), "application/json".to_string())];
        let found = headers_get(headers, "x-total-pages".to_string());
        assert!(
            found.is_unit(),
            "expected unit for a missing header, got {found:?}"
        );
    }

    #[test]
    fn test_headers_has_case_insensitive() {
        let headers = vec![("X-Total-Pages".to_string(), "3".to_string())];
        assert!(headers_has(headers.clone(), "x-total-pages".to_string()));
        assert!(!headers_has(headers, "x-total-count".to_string()));
    }
}
