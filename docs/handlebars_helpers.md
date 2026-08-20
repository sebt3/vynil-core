# Handlebars helpers

Reference for every Handlebars helper available through `vynil_core::hbs::HandleBars`. For the
Rhai side see [rhai_helpers.md](rhai_helpers.md); for test doubles see [mocking.md](mocking.md).

## How registration works

`HandleBars::new()` builds on `handlebars_misc_helpers::new_hbs()` — strict mode on, HTML
auto-escaping disabled (`no_escape`, since output is usually YAML/JSON, not HTML) — then layers
`vynil-core`'s own helpers on top. Everything in this document is available on every
`HandleBars` instance out of the box; there is no manual registration step (unlike the Rhai
`http`/`k8s`/`s3` modules).

Two more entry points let a consumer extend a `HandleBars` instance at runtime rather than at
compile time:

| Method | Loads |
|---|---|
| `register_helper_dir(dir)` | Every `*.rhai` file in `dir` as a Handlebars **script helper** (Handlebars' `script_helper` feature — the helper body is Rhai code) |
| `register_partial_dir(dir)` | Every `*.hbs` file in `dir` as a named template/partial |

## Feature tags

Only one helper pair is gated by a `vynil-core` Cargo feature — `gen_password` /
`gen_password_alphanum`, behind feature **`password`** (same reasoning as the Rhai side: off by
default so a consumer with its own password semantics doesn't get a name collision).

Everything else in this document — including every `handlebars_misc_helpers` sub-group below
(string / json / jsonnet / regex / uuid) — is compiled in unconditionally. Those sub-groups are
themselves Cargo features, but of the `handlebars_misc_helpers` *dependency*, fixed once in
`vynil-core`'s own `Cargo.toml`; a consumer of `vynil-core` cannot toggle them independently. They
are grouped below by origin for orientation, not because you can turn them off.

> **Note on `CORE_HBS_HELPERS`:** `vynil_core::hbs::CORE_HBS_HELPERS` is a curated `&[&str]`
> constant listing helper names, meant for consumers that need to introspect "what's available"
> (e.g. to validate user-supplied templates). It is not exhaustive — in particular it omits most
> of the `cruet`-based case-conversion family documented further down, in the
> "`handlebars_misc_helpers` — string feature" section (only
> `to_lower_case`/`to_upper_case`/`trim*`/`replace`/`quote`/`unquote`/`first_non_empty`
> are listed there, even though the rest are registered and callable). This document reflects what
> is actually registered, not just what's in that constant.

---

## 1. Handlebars-rs built-ins (always)

Standard control-flow, from the `handlebars` crate itself, no feature involved:

| Helper | Purpose |
|---|---|
| `{{#if cond}}…{{else}}…{{/if}}` | Conditional |
| `{{#unless cond}}…{{/unless}}` | Inverted conditional |
| `{{#each items}}…{{/each}}` | Iteration (`this`, `@index`, `@key`, …) |
| `{{#with value}}…{{/with}}` | Rescopes the block context |
| `{{lookup obj "key"}}` | Dynamic property/index lookup |
| `{{{{raw}}}}…{{{{/raw}}}}` | Emits its contents unprocessed |
| `{{log value}}` | Logs at the configured level |
| `{{*inline "name"}}…{{/inline}}` | Decorator — defines an inline partial named `name` for the rest of the template |

## 2. Comparison / logic extras (always)

Also from `handlebars` core (`helper_extras`), always registered:

| Helper | Signature | Notes |
|---|---|---|
| `eq` | `(a, b)` | |
| `ne` | `(a, b)` | |
| `gt` / `gte` / `lt` / `lte` | `(a, b)` | Numeric/lexicographic comparison |
| `and` / `or` | `(a, b, …)` | |
| `not` | `(a)` | |
| `len` | `(collection)` | Length of a string, array, or object |

Typically used inside `{{#if (eq a b)}}`.

## 3. Case-conversion — `handlebars` crate's `string_helpers` feature (always, via Cargo.toml)

Backed by [`heck`](https://docs.rs/heck):

| Helper | Example |
|---|---|
| `lowerCamelCase` | `lower camel case` → `lowerCamelCase` |
| `upperCamelCase` | → `UpperCamelCase` |
| `snakeCase` | → `snake_case` |
| `kebabCase` | → `kebab-case` |
| `shoutySnakeCase` | → `SHOUTY_SNAKE_CASE` |
| `shoutyKebabCase` | → `SHOUTY-KEBAB-CASE` |
| `titleCase` | → `Title Case` |
| `trainCase` | → `Train-Case` |

Each takes a single string argument, e.g. `{{snakeCase "Hello World"}}`.

> There's a second, independent case-conversion family further down, in the
> "`handlebars_misc_helpers` — string feature" section (`to_snake_case`, `to_camel_case`, …,
> backed by `cruet` instead of `heck`). Both are registered;
> pick whichever naming/behavior you need — they aren't identical in edge-case handling.

## 4. `handlebars_misc_helpers` — path (always)

| Helper | Example (on `/hello/bar/foo.txt`) |
|---|---|
| `parent` | `/hello/bar` |
| `file_name` | `foo.txt` |
| `extension` | `txt` |
| `canonicalize` | Resolves `.`/`..` and symlinks via the filesystem; empty string if the path doesn't exist |

`parent`/`file_name`/`extension` canonicalize the path first *if it exists on disk*, so relative
inputs like `.` behave sensibly; if the path doesn't exist they operate on the literal string.

## 5. `handlebars_misc_helpers` — env (always)

| Helper | Returns |
|---|---|
| `env_var "NAME"` | The environment variable's value, or `""` if unset. A few names are special-cased to Rust's `std::env::consts`/`USER` lookup instead of a real env var: `ARCH`, `OS`, `FAMILY`, `DLL_EXTENSION`, `DLL_PREFIX`, `DLL_SUFFIX`, `EXE_EXTENSION`, `EXE_SUFFIX`, `USERNAME` (tries `USERNAME`/`username`/`USER`/`user`, else `"noname"`) |

## 6. `handlebars_misc_helpers` — file (always)

| Helper | Returns |
|---|---|
| `read_to_str "path"` | File contents as a string; `""` (with a warning) if the path doesn't exist |

## 7. `handlebars_misc_helpers` — string feature

Trim/replace/quote basics:

| Helper | Signature | Notes |
|---|---|---|
| `to_lower_case` | `(s)` | |
| `to_upper_case` | `(s)` | |
| `trim` | `(s)` | |
| `trim_start` | `(s)` | |
| `trim_end` | `(s)` | |
| `replace` | `(s, from, to)` | |
| `quote` | `(quote_char, s)` | Wraps `s` in `quote_char` (first char of that arg), escaping as needed |
| `unquote` | `(s)` | Strips a matching pair of quotes; returns `s` unchanged if unquoting fails |
| `first_non_empty` | `(a, b, …)` | First argument that is a non-empty string (nulls/non-strings are skipped); `""` if none match |

Case-conversion family (`cruet`-backed; each is a `to_*`/`is_*` pair, `is_*` returns a bool):

| `to_*` helper | `is_*` helper | Example output |
|---|---|---|
| `to_camel_case` | `is_camel_case` | `helloFooBars` |
| `to_pascal_case` | `is_pascal_case` | `HelloFooBars` |
| `to_snake_case` | `is_snake_case` | `hello_foo_bars` |
| `to_screaming_snake_case` | `is_screaming_snake_case` | `HELLO_FOO_BARS` |
| `to_kebab_case` | `is_kebab_case` | `hello-foo-bars` |
| `to_train_case` | `is_train_case` | `Hello-Foo-Bars` |
| `to_sentence_case` | `is_sentence_case` | `Hello foo bars` |
| `to_title_case` | `is_title_case` | `Hello Foo Bars` |
| `to_class_case` | `is_class_case` | `HelloFooBar` (singularized) |
| `to_table_case` | `is_table_case` | `hello_foo_bars` (pluralized snake_case) |
| `to_foreign_key` | `is_foreign_key` | e.g. `Message` → `message_id` |

Plus, with no `is_*` counterpart:

| Helper | Purpose |
|---|---|
| `ordinalize` | `1` → `1st` |
| `deordinalize` | `1st` → `1` |
| `deconstantize` | Strips the last `::`-separated segment (`Foo::Bar::Baz` → `Foo::Bar`) |
| `demodulize` | Keeps only the last `::`-separated segment (`Foo::Bar::Baz` → `Baz`) |
| `to_plural` | `bar` → `bars` |
| `to_singular` | `bars` → `bar` |

## 8. `handlebars_misc_helpers` — json feature

All accept an optional `format=` hash argument: `"json"` (default), `"json_pretty"`, `"yaml"`,
`"toml"`, or `"toml_pretty"`.

| Helper | Form | Purpose |
|---|---|---|
| `str_to_json` | `(s)` | Parses `s` (in `format`) into a JSON value |
| `json_to_str` | `(value)` | Serializes `value` to a string in `format` |
| `{{#from_json}}…{{/from_json}}` | block | Parses the block's rendered content as JSON, re-emits it in `format` |
| `{{#to_json}}…{{/to_json}}` | block | Parses the block's rendered content as `format`, re-emits it as pretty JSON |
| `json_query` | `(expr, data)` | [JMESPath](https://jmespath.org/) query against a JSON value; `null` if nothing matches |
| `json_str_query` | `(expr, s)` | Same, but parses `s` first (as `format`) and re-serializes the result in `format` (falls back to plain JSON for scalar results) |

Empty or `null` input to any of these returns an empty string rather than an error.

## 9. `handlebars_misc_helpers` — jsonnet feature

| Helper | Form | Purpose |
|---|---|---|
| `{{#jsonnet}}…{{/jsonnet}}` | block | Evaluates the block's rendered content as a [Jsonnet](https://jsonnet.org/) snippet, emits the resulting JSON |

## 10. `handlebars_misc_helpers` — regex feature

| Helper | Signature | Returns |
|---|---|---|
| `regex_captures` | `pattern=".."  on=".."` (hash args, both required) | An object mapping each named capture group to its match, plus positional `_0`, `_1`, … for every group (named or not); `null` if the pattern doesn't match |
| `regex_is_match` | `pattern=".."  on=".."` | `bool` |

## 11. `handlebars_misc_helpers` — uuid feature

| Helper | Returns |
|---|---|
| `uuid_new_v4` | A random UUID v4 |
| `uuid_new_v7` | ⚠ Also generates a v4, not a real v7 — that's a bug in `handlebars_misc_helpers` 0.17.0's `uuid_new_v7_fct` (it calls `Uuid::new_v4()`), not a `vynil-core` behavior. Don't rely on v7's time-ordering property from this helper. |

---

## 12. `vynil-core`'s own helpers (always)

| Helper | Signature | Returns |
|---|---|---|
| `concat` | `(a, b)` | String concatenation |
| `to_decimal` | `(octal: string)` | The string parsed as base-8, as a decimal string (`0` and a warning if invalid) |
| `base64_encode` | `(s)` | |
| `base64_decode` | `(s)` | `""` and a warning on invalid input, rather than erroring the render |
| `url_encode` | `(s)` | Percent-encoding |
| `header_basic` | `(username, password)` | `"Basic <base64(user:pass)>"` — a ready-to-use `Authorization` header value |
| `argon_hash` | `(password)` | Argon2 hash (fresh random salt each call) |
| `bcrypt_hash` | `(password)` | bcrypt hash, `DEFAULT_COST` |
| `crc32_hash` | `(text)` | CRC-32, as a number |
| `gen_private_key` | `(algo, {bits=4096})` | PKCS8 PEM. `algo`: `"rsa"` or `"ed25519"` (bits ignored for ed25519) |

All of the string-typed arguments above tolerate non-string JSON input by falling back to `""`
(with a `tracing::warn!`) rather than failing the render.

## 13. `vynil-core`'s own helpers — feature `password`

| Helper | Signature | Returns |
|---|---|---|
| `gen_password` | `(len, {lower=1, upper=1, digits=1, symbols=1})` | Password with at least that many of each class |
| `gen_password_alphanum` | `(len)` | Same as `gen_password` with `symbols=0` |

Both return `""` (with a warning) instead of failing the render if the generation spec is
invalid (e.g. minimums exceeding `len`).
