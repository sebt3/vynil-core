//! Glob matching helper for Rhai (`glob` function wrapping `wildmatch`).

use rhai::{Engine, ImmutableString};
use wildmatch::WildMatch;

fn glob_fn(text: ImmutableString, pattern: ImmutableString) -> bool {
    WildMatch::new(pattern.as_ref()).matches(text.as_ref())
}

/// Register the `glob(text, pattern) -> bool` Rhai helper.
pub fn glob_rhai_register(engine: &mut Engine) {
    engine.register_fn("glob", glob_fn);
}
