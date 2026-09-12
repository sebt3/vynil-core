//! Mock OCI registry for tests. See [`crate::oci::Registry`] for the real client.

use crate::RhaiRes;
use rhai::{Dynamic, Engine};

/// No-op OCI registry mock exposing the Rhai surface of [`crate::oci::Registry`]: every call
/// succeeds and returns static test data.
#[derive(Clone, Debug)]
pub struct OciRegistryMock;

impl OciRegistryMock {
    /// Mirrors `Registry::list_tags`; always returns an empty Rhai array.
    ///
    /// # Errors
    ///
    /// Never fails in the mock; the `Result` only mirrors the real API signature.
    pub fn list_tags(&mut self, _repository: String) -> RhaiRes<Dynamic> {
        Ok(Dynamic::from_array(vec![]))
    }

    /// Mirrors `Registry::get_manifest`; always returns a manifest with empty `annotations`.
    ///
    /// # Errors
    ///
    /// Never fails in the mock; the `Result` only mirrors the real API signature.
    pub fn get_manifest(&mut self, _repository: String, _tag: String) -> RhaiRes<Dynamic> {
        let mut map = rhai::Map::new();
        map.insert("annotations".into(), Dynamic::from_map(rhai::Map::new()));
        Ok(Dynamic::from_map(map))
    }

    /// Mirrors `Registry::push_image`; always returns a fixed mock digest.
    ///
    /// # Errors
    ///
    /// Never fails in the mock; the `Result` only mirrors the real API signature.
    pub fn push_image(
        &mut self,
        _dir: String,
        _repo: String,
        _tag: String,
        _ann: Dynamic,
    ) -> RhaiRes<rhai::ImmutableString> {
        Ok("sha256:mock-digest-for-testing".into())
    }

    /// Mirrors `Registry::sign_image`; always a successful no-op.
    ///
    /// # Errors
    ///
    /// Never fails in the mock; the `Result` only mirrors the real API signature.
    pub fn sign_image(&mut self, _repo: String, _tag: String, _digest: String, _key: String) -> RhaiRes<()> {
        Ok(())
    }
}

/// Registers the `OciRegistryMock` type and its Rhai methods on a Rhai `engine`.
pub fn oci_mock_rhai_register(engine: &mut Engine) {
    engine
        .register_type_with_name::<OciRegistryMock>("OciRegistryMock")
        .register_fn("new_registry", |_: String, _: String, _: String| OciRegistryMock)
        .register_fn("list_tags", OciRegistryMock::list_tags)
        .register_fn("get_manifest", OciRegistryMock::get_manifest)
        .register_fn("push_image", OciRegistryMock::push_image)
        .register_fn("sign_image", OciRegistryMock::sign_image);
}
