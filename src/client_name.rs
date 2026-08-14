use std::sync::OnceLock;

static CLIENT_NAME: OnceLock<Box<dyn Fn() -> String + Send + Sync>> = OnceLock::new();

/// Configure the identity vynil-core reports as the HTTP `User-Agent` and, when the `k8s`
/// feature is active, as the Server-Side-Apply field manager.
///
/// There is no built-in default: the consuming application must call this once, before issuing
/// any `RestClient` or `k8s` call, or those calls will panic.
pub fn set_client_name(f: impl Fn() -> String + Send + Sync + 'static) {
    CLIENT_NAME.set(Box::new(f)).ok();
}

pub fn client_name_is_set() -> bool {
    CLIENT_NAME.get().is_some()
}

pub fn get_client_name() -> String {
    CLIENT_NAME.get().map(|f| f()).expect(
        "vynil_core: client name not configured — call vynil_core::set_client_name(...) before \
         performing HTTP or Kubernetes requests",
    )
}
