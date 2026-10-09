//! S3 helpers (`s3_get_yaml`, `s3_list_keys`) exposed to Rhai.
//!
//! Feature `s3` only (implies `rhai`). Uses `object_store` with S3 (and HTTP endpoint for mocks).

use crate::{Error, RhaiRes, rhai_err};
use futures::StreamExt;
use object_store::{ObjectStore, path::Path};
use rhai::{Dynamic, Engine};

/// Join `prefix` and `key` into one object key with a single `/` separator:
/// trailing `/` trimmed from `prefix`, leading `/` trimmed from `key`, one `/`
/// between the two when the remaining prefix is non-empty, the bare key
/// otherwise (`"p"` and `"p/"` prefixes are equivalent — decided, s3.sdd).
fn join_prefix_key(prefix: &str, key: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    let key = key.trim_start_matches('/');
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}/{key}")
    }
}

fn build_store(
    bucket: &str,
    region: &str,
    endpoint: &str,
    access_key: &str,
    secret_key: &str,
) -> crate::Result<Box<dyn ObjectStore>> {
    use object_store::aws::AmazonS3Builder;
    if access_key.is_empty() != secret_key.is_empty() {
        return Err(Error::Other(
            "s3: access_key and secret_key must be provided together".to_string(),
        ));
    }
    let mut builder = AmazonS3Builder::new()
        .with_bucket_name(bucket)
        .with_region(region);
    if !access_key.is_empty() {
        builder = builder
            .with_access_key_id(access_key)
            .with_secret_access_key(secret_key);
    }
    if !endpoint.is_empty() {
        builder = builder.with_endpoint(endpoint).with_allow_http(true);
    }
    let store = builder.build().map_err(|e| Error::Other(e.to_string()))?;
    Ok(Box::new(store))
}

/// Read the object `prefix` + `key` (normalized join) from `store` and transmute it
/// YAML → JSON → [`Dynamic`]. Private seam for `InMemory` tests (s3.sdd).
async fn get_yaml_from_store(store: &dyn ObjectStore, prefix: &str, key: &str) -> crate::Result<Dynamic> {
    let full_key = join_prefix_key(prefix, key);
    let path = Path::from(full_key.as_str());
    let result = store.get(&path).await.map_err(|e| Error::Other(e.to_string()))?;
    let bytes = result.bytes().await.map_err(|e| Error::Other(e.to_string()))?;
    let body = String::from_utf8(bytes.to_vec()).map_err(Error::UTF8)?;
    let value: serde_yaml::Value =
        serde_yaml::from_str(&body).map_err(|e| Error::YamlError(e.to_string()))?;
    let json = serde_json::to_string(&value).map_err(Error::SerializationError)?;
    serde_json::from_str::<Dynamic>(&json).map_err(Error::SerializationError)
}

/// List keys under `prefix` in `store` as FULL locations (prefix included, never
/// trimmed), sorted in lexicographic byte order (decided, s3.sdd). Private seam
/// for `InMemory` tests.
async fn list_keys_from_store(store: &dyn ObjectStore, prefix: &str) -> crate::Result<Vec<String>> {
    let prefix_path = Path::from(prefix);
    let mut list = store.list(Some(&prefix_path));
    let mut keys = vec![];
    while let Some(meta) = list.next().await {
        let meta = meta.map_err(|e: object_store::Error| Error::Other(e.to_string()))?;
        keys.push(meta.location.to_string());
    }
    keys.sort();
    Ok(keys)
}

/// Fetch `key` (prefixed by `prefix`) from `bucket`/`region`/`endpoint` and parse it as YAML
/// into a Rhai [`Dynamic`] map. Rhai signature: `s3_get_yaml(bucket, region, prefix, endpoint, access_key, secret_key, key)`.
///
/// Blocks through `crate::rt::block_on` (never panics the host; see `rt.sdd`).
///
/// # Errors
///
/// Returns a Rhai error wrapping [`Error::Other`] on store build or object fetch failures
/// (including incomplete credentials), [`Error::UTF8`] on non-UTF-8 bytes,
/// [`Error::YamlError`] on invalid YAML, [`Error::SerializationError`] on the JSON/Rhai
/// conversion, or [`Error::Other`] on a `current_thread` runtime.
pub fn s3_get_yaml(
    bucket: String,
    region: String,
    prefix: String,
    endpoint: String,
    access_key: String,
    secret_key: String,
    key: String,
) -> RhaiRes<Dynamic> {
    crate::rt::block_on(async move {
        let store = build_store(&bucket, &region, &endpoint, &access_key, &secret_key)?;
        get_yaml_from_store(store.as_ref(), &prefix, &key).await
    })
    .and_then(|r| r)
    .map_err(rhai_err)
}

/// List keys under `prefix` in `bucket`/`region`/`endpoint` — full locations, sorted
/// lexicographically. Rhai: `s3_list_keys(...) -> [String]`.
///
/// Blocks through `crate::rt::block_on` (never panics the host; see `rt.sdd`).
///
/// # Errors
///
/// Returns a Rhai error wrapping [`Error::Other`] if the store cannot be built
/// (including incomplete credentials), the listing fails, or the runtime is
/// `current_thread`.
pub fn s3_list_keys(
    bucket: String,
    region: String,
    prefix: String,
    endpoint: String,
    access_key: String,
    secret_key: String,
) -> RhaiRes<Vec<String>> {
    crate::rt::block_on(async move {
        let store = build_store(&bucket, &region, &endpoint, &access_key, &secret_key)?;
        list_keys_from_store(store.as_ref(), &prefix).await
    })
    .and_then(|r| r)
    .map_err(rhai_err)
}

/// Registers the S3 Rhai helpers on a Rhai `engine`.
pub fn s3_rhai_register(engine: &mut Engine) {
    engine
        .register_fn("s3_get_yaml", s3_get_yaml)
        .register_fn("s3_list_keys", s3_list_keys);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    use object_store::{PutPayload, memory::InMemory, path::Path};

    async fn put_key(store: &InMemory, key: &str, body: &[u8]) {
        store
            .put(&Path::from(key), PutPayload::from(body.to_vec()))
            .await
            .unwrap();
    }

    // ── Verrou de découpage : jonction prefix/key normalisée, pure (s3.sdd « prefix et
    // key se joignent par un seul séparateur » — la forme normalisée verrouillée ici, la
    // lecture InMemory complète arrive avec la tâche « Convertir les Scenario »). ──
    #[test]
    fn join_prefix_key_normalizes_separators() {
        assert_eq!(join_prefix_key("p", "/k"), "p/k");
        assert_eq!(join_prefix_key("", "k"), "k");
        assert_eq!(join_prefix_key("p/", "k"), "p/k");
        assert_eq!(join_prefix_key("p//", "/k"), "p/k");
        // « p » et « p/ » équivalents (décision actée) : même clé rendue.
        assert_eq!(join_prefix_key("p", "k.yaml"), join_prefix_key("p/", "k.yaml"));
        // Jamais de concaténation sans séparateur ni de double `/` interne.
        for (prefix, key) in [
            ("p", "/k"),
            ("p/", "k"),
            ("p//", "/k"),
            ("a/b/", "/c/d"),
            ("", "/k"),
        ] {
            let joined = join_prefix_key(prefix, key);
            assert!(
                !joined.is_empty() && !joined.contains("//"),
                "double / : {joined}"
            );
            if !prefix.trim_end_matches('/').is_empty() {
                assert!(joined.contains('/'), "séparateur perdu : {joined}");
            }
        }
    }

    // ── Verrou de découpage : credentials en couple obligatoire (s3.sdd Must « build_store »,
    // chaîne exacte actée) ; les deux vides = default chain, le build passe. ──
    #[test]
    fn incomplete_credentials_rejected_with_exact_message() {
        let expected = "s3: access_key and secret_key must be provided together";
        for (access, secret) in [("access", ""), ("", "secret")] {
            match build_store("bucket", "region", "", access, secret) {
                Err(Error::Other(msg)) => assert_eq!(msg, expected),
                other => panic!("Error::Other attendu, rendu : {other:?}"),
            }
        }
    }

    #[test]
    fn empty_credentials_build_passes_default_chain_to_request_time() {
        // Sonde locale mesurée 0.11.2 : region vide, aucune credential → build Ok (la chaîne
        // de credential se résout à la requête). Écart remonté : le `Must` de s3.sdd dit
        // « bucket vide → build Err (MissingBucketName) », mais ici build("") rend Ok —
        // la sonde bucket-vide appartient à la tâche « Convertir les Scenario ».
        assert!(build_store("bucket", "", "", "", "").is_ok());
    }

    // ── Les deux tests InMemory existants, traversant désormais les fonctions internes
    // (seam décidé), contrats inchangés. ──
    #[tokio::test(flavor = "multi_thread")]
    async fn test_s3_get_yaml_with_inmemory() {
        let yaml = "packages:\n  - name: mypkg\n";
        let store = InMemory::new();
        put_key(&store, "mybucket/index.yaml", yaml.as_bytes()).await;

        let store: Box<dyn ObjectStore> = Box::new(store);
        let result = get_yaml_from_store(store.as_ref(), "mybucket/", "index.yaml")
            .await
            .unwrap();
        assert!(result.is_map());
        let map = result.cast::<rhai::Map>();
        assert!(map.contains_key("packages"));
    }

    // ── Verrou de découpage : tri lexicographique du listing (décision actée), locations
    // COMPLÈTES jamais rognées. Clés déposées en désordre pour que le tri soit observable. ──
    #[tokio::test(flavor = "multi_thread")]
    async fn test_s3_list_keys_with_inmemory() {
        let store = InMemory::new();
        for key in &["prefix/c.yaml", "prefix/a.yaml", "prefix/b.yaml"] {
            put_key(&store, key, b"key: val").await;
        }

        let store: Box<dyn ObjectStore> = Box::new(store);
        let keys = list_keys_from_store(store.as_ref(), "prefix/").await.unwrap();
        assert_eq!(keys, vec!["prefix/a.yaml", "prefix/b.yaml", "prefix/c.yaml"]);
    }

    // ── Verrou du pont (s3.sdd « hors runtime multi-thread, jamais de panique », bras
    // current_thread) : l'appel public rend l'erreur explicite de `crate::rt::block_on`,
    // sans exécuter le corps, sans panique. ──
    #[tokio::test(flavor = "current_thread")]
    async fn current_thread_runtime_returns_explicit_error_without_panic() {
        let err = s3_list_keys(
            "bucket".to_string(),
            "region".to_string(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        )
        .expect_err("current_thread doit rendre une erreur explicite");
        assert!(
            err.to_string().contains("requires a multi-thread tokio runtime"),
            "message attendu explicite, obtenu : {err}"
        );
    }

    // ── Bras « hors de tout runtime » du pont : le runtime temporaire sert, le corps
    // s'exécute sans panique, et la garde credentials (dans build_store) parle avant tout
    // réseau. Verrouille « hors runtime multi-thread, jamais de panique », bras sans runtime. ──
    #[test]
    fn without_runtime_temporary_runtime_serves_without_panic() {
        let err = s3_list_keys(
            "bucket".to_string(),
            "region".to_string(),
            String::new(),
            String::new(),
            "access".to_string(),
            String::new(),
        )
        .expect_err("credential seule : la garde de build_store doit parler, hors runtime aussi");
        assert!(
            err.to_string()
                .contains("s3: access_key and secret_key must be provided together"),
            "la garde credentials doit remonter via le pont, obtenu : {err}"
        );
    }
}
