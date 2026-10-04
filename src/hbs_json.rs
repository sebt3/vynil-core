//! `json_to_str`/`str_to_json`/`from_json`/`to_json`/`json_query`/`json_str_query` Handlebars
//! helpers.
//!
//! Vendored (not depended on) from `handlebars_misc_helpers` 0.17.0's `json_helpers.rs`
//! (CC0-1.0, <https://github.com/davidB/handlebars_misc_helpers>) rather than pulled in as a
//! dependency: that crate pins `jmespath 0.3.0`, whose `Function` trait lacks a `Send` bound —
//! harmless on its own, but a hard compile failure for any consumer whose dependency graph also
//! unifies in `lazy_static`'s `spin_no_std` feature (e.g. via `rsa`/`num-bigint-dig`, itself
//! pulled by OIDC stacks). `jmespath` fixed this upstream in 0.5.0 (`Function: Sync + Send`), so
//! vynil-core depends on `jmespath` directly at that version instead.
use handlebars::{
    Context, Handlebars, Helper, HelperDef, HelperResult, Output, RenderContext, RenderError,
    RenderErrorReason, Renderable, ScopedJson, StringOutput, handlebars_helper,
};
use serde::Serialize;
use serde_json::Value as Json;
use std::str::FromStr;
use thiserror::Error;
use toml::value::Table;

type TablePartition = Vec<(String, toml::Value)>;

#[derive(Debug, Error)]
enum JsonError {
    #[error("query failure for expression '{expression}'")]
    JsonQueryFailure {
        expression: String,
        source: jmespath::JmespathError,
    },
    #[error("fail to convert '{input}'")]
    ToJsonValueError {
        input: String,
        source: serde_json::error::Error,
    },
    #[error("data format unknown '{format}'")]
    DataFormatUnknown { format: String },
}

fn to_nested_error<E>(cause: E) -> RenderError
where
    E: std::error::Error + Send + Sync + 'static,
{
    RenderErrorReason::NestedError(Box::new(cause)).into()
}

fn to_other_error<T: AsRef<str>>(desc: T) -> RenderError {
    RenderErrorReason::Other(desc.as_ref().to_string()).into()
}

#[derive(Debug, Clone)]
enum DataFormat {
    Json,
    JsonPretty,
    Yaml,
    Toml,
    TomlPretty,
}

impl FromStr for DataFormat {
    type Err = JsonError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "json_pretty" => Ok(Self::JsonPretty),
            "yaml" => Ok(Self::Yaml),
            "toml" => Ok(Self::Toml),
            "toml_pretty" => Ok(Self::TomlPretty),
            _ => Err(JsonError::DataFormatUnknown {
                format: s.to_string(),
            }),
        }
    }
}

fn to_opt_res<T, E>(v: Result<Option<T>, E>) -> Option<Result<T, E>> {
    match v {
        Err(e) => Some(Err(e)),
        Ok(v) => v.map(Ok),
    }
}

// `toml` serializes tables-after-non-tables as an error (ValueAfterTable), so a plain
// `Json -> toml::Value` conversion needs its map keys reordered: scalars first, arrays next,
// tables last (recursively, since a table's own entries face the same constraint).
fn to_ordored_toml_value(data: &Json) -> Result<Option<toml::Value>, RenderError> {
    match data {
        Json::String(v) => Ok(Some(toml::Value::from(v.as_str()))),
        Json::Array(v) => v
            .iter()
            .filter_map(|i| to_opt_res(to_ordored_toml_value(i)))
            .collect::<Result<Vec<_>, _>>()
            .map(|a| Some(toml::Value::Array(a))),
        Json::Object(obj) => obj
            .iter()
            .filter_map(|kv| {
                to_opt_res(to_ordored_toml_value(kv.1)).map(|rnv| rnv.map(|nv| (kv.0.to_owned(), nv)))
            })
            .collect::<Result<Table, _>>()
            .map(|m| Some(toml::Value::Table(sort_toml_map(m)))),
        Json::Number(v) => {
            if let Some(i) = v.as_i64() {
                Ok(Some(toml::Value::Integer(i)))
            } else if let Some(x) = v.as_f64() {
                Ok(Some(toml::Value::Float(x)))
            } else {
                Err(to_other_error(format!(
                    "to_toml: can not convert a Json Number: {v}"
                )))
            }
        }
        Json::Bool(v) => Ok(Some(toml::Value::Boolean(*v))),
        Json::Null => Ok(None),
    }
}

fn sort_toml_map(data: Table) -> Table {
    let (tables, non_tables): (TablePartition, TablePartition) =
        data.into_iter().partition(|v| v.1.is_table());
    let (arrays, others): (TablePartition, TablePartition) =
        non_tables.into_iter().partition(|v| v.1.is_array());
    let mut m = Table::new();
    m.extend(others);
    m.extend(arrays);
    m.extend(tables);
    m
}

impl DataFormat {
    fn read_string(&self, data: &str) -> Result<Json, RenderError> {
        if data.is_empty() {
            return Ok(Json::String(String::new()));
        }
        match self {
            DataFormat::Json | DataFormat::JsonPretty => serde_json::from_str(data).map_err(to_nested_error),
            DataFormat::Yaml => serde_yaml::from_str(data).map_err(to_nested_error),
            DataFormat::Toml | DataFormat::TomlPretty => toml::from_str(data).map_err(to_nested_error),
        }
    }

    fn write_string(&self, data: &Json) -> Result<String, RenderError> {
        match data {
            Json::Null => Ok(String::new()),
            Json::String(c) if c.is_empty() => Ok(String::new()),
            _ => match self {
                DataFormat::Json => serde_json::to_string(data).map_err(to_nested_error),
                DataFormat::JsonPretty => serde_json::to_string_pretty(data).map_err(to_nested_error),
                DataFormat::Yaml => serde_yaml::to_string(data)
                    .map_err(to_nested_error)
                    .map(|s| s.trim_start_matches("---\n").to_string()),
                DataFormat::Toml => {
                    let data_toml = to_ordored_toml_value(data)?;
                    toml::to_string(&data_toml).map_err(to_nested_error)
                }
                DataFormat::TomlPretty => {
                    let data_toml = to_ordored_toml_value(data)?;
                    toml::to_string_pretty(&data_toml).map_err(to_nested_error)
                }
            },
        }
    }
}

// `JsonError` porte la chaîne d'erreur de rendu, trop volumineux pour un retour inline (vyvil-core.sdd)
#[allow(clippy::result_large_err)]
fn json_query<T: Serialize, E: AsRef<str>>(expr: E, data: T) -> Result<Json, JsonError> {
    let res = jmespath::compile(expr.as_ref())
        .and_then(|e| e.search(data))
        .map_err(|source| JsonError::JsonQueryFailure {
            expression: expr.as_ref().to_string(),
            source,
        })?;
    serde_json::to_value(res.as_ref()).map_err(|source| JsonError::ToJsonValueError {
        input: format!("{res:?}"),
        source,
    })
}

fn find_data_format(h: &Helper) -> Result<DataFormat, RenderError> {
    let param = h
        .hash_get("format")
        .and_then(|v| v.value().as_str())
        .unwrap_or("json");
    DataFormat::from_str(param).map_err(to_nested_error)
}

fn find_str_param(pos: usize, h: &Helper) -> Result<String, RenderError> {
    h.param(pos)
        .ok_or_else(|| to_other_error(format!("param {pos} (the string) not found")))
        .map(|v| v.value().as_str().unwrap_or("").to_owned())
}

#[allow(non_camel_case_types)]
struct str_to_json_fct;

impl HelperDef for str_to_json_fct {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'reg>, RenderError> {
        let data: String = find_str_param(0, h)?;
        let format = find_data_format(h)?;
        let result = format.read_string(&data)?;
        Ok(ScopedJson::Derived(result))
    }
}

#[allow(non_camel_case_types)]
struct json_to_str_fct;

impl HelperDef for json_to_str_fct {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'reg>, RenderError> {
        let format = find_data_format(h)?;
        let data = h
            .param(0)
            .ok_or_else(|| to_other_error("param 0 (the json) not found"))
            .map(handlebars::PathAndJson::value)?;
        let result = format.write_string(data)?;
        Ok(ScopedJson::Derived(Json::String(result)))
    }
}

#[allow(non_camel_case_types)]
struct json_str_query_fct;

impl HelperDef for json_str_query_fct {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'reg>, RenderError> {
        let format = find_data_format(h)?;
        let expr = find_str_param(0, h)?;
        let data_str = find_str_param(1, h)?;
        let data = format.read_string(&data_str)?;
        let result = json_query(expr, data).map_err(to_nested_error).and_then(|v| {
            let output_format = if v.is_array() || v.is_object() {
                format
            } else {
                DataFormat::Json
            };
            output_format.write_string(&v).map(|s| {
                if v.is_array() || v.is_object() {
                    s
                } else {
                    s.trim().to_owned()
                }
            })
        })?;
        Ok(ScopedJson::Derived(Json::String(result)))
    }
}

fn from_json_block<'reg, 'rc>(
    h: &Helper<'rc>,
    r: &'reg Handlebars,
    ctx: &'rc Context,
    rc: &mut RenderContext<'reg, 'rc>,
    out: &mut dyn Output,
) -> HelperResult {
    let format = find_data_format(h)?;
    let mut content = StringOutput::default();
    h.template()
        .map_or(Ok(()), |t| t.render(r, ctx, rc, &mut content))?;
    let data = DataFormat::Json.read_string(&content.into_string().map_err(to_nested_error)?)?;
    let res = format.write_string(&data)?;
    out.write(&res).map_err(to_nested_error)
}

fn to_json_block<'reg, 'rc>(
    h: &Helper<'rc>,
    r: &'reg Handlebars,
    ctx: &'rc Context,
    rc: &mut RenderContext<'reg, 'rc>,
    out: &mut dyn Output,
) -> HelperResult {
    let format = find_data_format(h)?;
    let mut content = StringOutput::default();
    h.template()
        .map_or(Ok(()), |t| t.render(r, ctx, rc, &mut content))?;
    let data = format.read_string(&content.into_string().map_err(to_nested_error)?)?;
    let res = DataFormat::JsonPretty.write_string(&data)?;
    out.write(&res).map_err(RenderError::from)
}

handlebars_helper!(json_query_fct: |expr: str, data: Json| json_query(expr, data).map_err(to_nested_error)?);

pub(crate) fn register(handlebars: &mut Handlebars) {
    handlebars.register_helper("json_to_str", Box::new(json_to_str_fct));
    handlebars.register_helper("str_to_json", Box::new(str_to_json_fct));
    handlebars.register_helper("from_json", Box::new(from_json_block));
    handlebars.register_helper("to_json", Box::new(to_json_block));
    handlebars.register_helper("json_query", Box::new(json_query_fct));
    handlebars.register_helper("json_str_query", Box::new(json_str_query_fct));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(tmpl: &str) -> String {
        let mut hb = Handlebars::new();
        hb.register_escape_fn(handlebars::no_escape);
        register(&mut hb);
        hb.render_template(tmpl, &Json::Null).unwrap()
    }

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(render(r#"{{ json_to_str "" }}"#), "");
        assert_eq!(render(r#"{{ str_to_json "" }}"#), "");
        assert_eq!(render(r#"{{ json_query "foo" "" }}"#), "");
        assert_eq!(render(r#"{{ json_str_query "foo" "" }}"#), "");
    }

    #[test]
    fn null_input_returns_empty() {
        assert_eq!(render(r"{{ json_to_str null }}"), "");
        assert_eq!(render(r"{{ str_to_json null }}"), "");
    }

    #[test]
    fn json_to_str_roundtrip() {
        assert_eq!(render(r"{{ json_to_str {} }}"), "{}");
        assert_eq!(
            render(r#"{{ json_to_str {"foo":{"bar":{"baz":true}}} }}"#),
            r#"{"foo":{"bar":{"baz":true}}}"#
        );
        assert_eq!(
            render(r#"{{ json_to_str ( str_to_json "{\"foo\":true}" ) }}"#),
            r#"{"foo":true}"#
        );
    }

    #[test]
    fn json_query_extracts_field() {
        assert_eq!(
            render(r#"{{ json_to_str ( json_query "foo" {"foo":{"bar":{"baz":true}}} ) }}"#),
            r#"{"bar":{"baz":true}}"#
        );
    }

    #[test]
    fn json_str_query_on_yaml() {
        assert_eq!(
            render(r#"{{ json_str_query "foo.bar.baz" "foo:\n bar:\n  baz: true\n" format="yaml"}}"#),
            "true"
        );
    }

    #[test]
    fn json_str_query_on_toml() {
        assert_eq!(
            render(r#"{{ json_str_query "foo.bar.baz" "[foo.bar]\nbaz=true\n" format="toml"}}"#),
            "true"
        );
    }

    #[test]
    fn to_json_block_wraps_rendered_content() {
        assert_eq!(
            render(r#"{{#to_json}}{"foo":{"bar":{"baz":true}}}{{/to_json}}"#),
            "{\n  \"foo\": {\n    \"bar\": {\n      \"baz\": true\n    }\n  }\n}"
        );
    }

    #[test]
    fn from_json_block_converts_to_yaml() {
        assert_eq!(
            render(r#"{{#from_json format="yaml"}}{"foo":{"bar":true}}{{/from_json}}"#),
            "foo:\n  bar: true\n"
        );
    }

    #[test]
    fn data_format_symmetry() {
        for (fmt, data) in [
            (DataFormat::Json, r#"{"foo":{"bar":{"baz":true}}}"#),
            (DataFormat::Toml, "[foo.bar]\nbaz = true\n"),
        ] {
            let actual = fmt.write_string(&fmt.read_string(data).unwrap()).unwrap();
            assert_eq!(actual, data);
        }
    }

    // ── Scenario « la table des portes limite le module à hbs » (membres jouables) ──
    // Le Given « crate compilée sans aucune feature » et le membre « @crate::hbs_json
    // ne cite ni crypto, ni rhai, ni hbs-scripting » sont des verrous de compilation :
    // les portes sont jouées par la batterie (`cargo check/test --no-default-features
    // --features hbs`), pas par un test. Seuls les noms listés se verrouillent ici.
    #[test]
    fn gating_core_helpers_enumerate_the_six() {
        for name in [
            "json_to_str",
            "str_to_json",
            "from_json",
            "to_json",
            "json_query",
            "json_str_query",
        ] {
            assert!(
                crate::hbs::CORE_HBS_HELPERS.contains(&name),
                "{name} absent de CORE_HBS_HELPERS"
            );
        }
    }

    // ── Table complète des cinq formats en lecture ET en écriture (face privée, dont
    // `JsonPretty` en lecture acceptée par le même bras que `Json`) + casse de `format` ──
    #[test]
    fn data_format_table_reads_and_writes_all_five_formats() {
        let doc = serde_json::json!({"a": 1});
        assert_eq!(DataFormat::Json.read_string(r#"{"a":1}"#).unwrap(), doc);
        assert_eq!(DataFormat::JsonPretty.read_string(r#"{"a":1}"#).unwrap(), doc);
        assert_eq!(DataFormat::Yaml.read_string("a: 1\n").unwrap(), doc);
        assert_eq!(DataFormat::Toml.read_string("a = 1\n").unwrap(), doc);
        assert_eq!(DataFormat::TomlPretty.read_string("a = 1\n").unwrap(), doc);
        assert_eq!(DataFormat::Json.write_string(&doc).unwrap(), r#"{"a":1}"#);
        assert_eq!(
            DataFormat::JsonPretty.write_string(&doc).unwrap(),
            "{\n  \"a\": 1\n}"
        );
        assert_eq!(DataFormat::Yaml.write_string(&doc).unwrap(), "a: 1\n");
        assert_eq!(DataFormat::Toml.write_string(&doc).unwrap(), "a = 1\n");
        assert_eq!(DataFormat::TomlPretty.write_string(&doc).unwrap(), "a = 1\n");
        // casse indifférente : la valeur est lowercasée avant comparaison
        for (raw, debug_expected) in [
            ("JSON", "Json"),
            ("Json_Pretty", "JsonPretty"),
            ("YAML", "Yaml"),
            ("Toml", "Toml"),
            ("TOML_PRETTY", "TomlPretty"),
        ] {
            assert_eq!(
                format!("{:?}", DataFormat::from_str(raw).unwrap()),
                debug_expected
            );
        }
    }

    // ── Scenario « le format se lit en hash et résiste à la casse » ──
    #[test]
    fn format_hash_survives_case_and_rejects_aliases_and_empty() {
        // membre 1 : le nom survit à la casse — lu en TOML, écrit en json_pretty
        assert_eq!(
            render(r#"{{#to_json format="TOML"}}b=1{{/to_json}}"#),
            "{\n  \"b\": 1\n}"
        );
        // membre 2 : l'alias `yml` n'est pas reconnu, message exact à la casse brute
        let err = render_err(r#"{{ json_to_str {"a":1} format="yml" }}"#);
        assert_eq!(nested(&err).to_string(), "data format unknown 'yml'");
        // membre 3 (`And`) : la valeur vide n'est pas le défaut, message entre quotes vides
        let err = render_err(r#"{{ json_to_str {"a":1} format="" }}"#);
        assert_eq!(nested(&err).to_string(), "data format unknown ''");
    }

    // ── Scenario « vide et null rendent vide quel que soit le format » (complément de
    // `empty_input_returns_empty`/`null_input_returns_empty` : « quel que soit le format ») ──
    #[test]
    fn empty_and_null_short_circuit_every_format() {
        for fmt in [
            DataFormat::Json,
            DataFormat::JsonPretty,
            DataFormat::Yaml,
            DataFormat::Toml,
            DataFormat::TomlPretty,
        ] {
            assert_eq!(
                fmt.read_string("").unwrap(),
                Json::String(String::new()),
                "la lecture vide rend la chaîne vide quel que soit le format"
            );
            assert_eq!(
                fmt.write_string(&Json::Null).unwrap(),
                "",
                "l'écriture Null rend vide quel que soit le format"
            );
            assert_eq!(
                fmt.write_string(&Json::String(String::new())).unwrap(),
                "",
                "l'écriture chaîne vide rend vide quel que soit le format"
            );
        }
        // membre 2 (`Given` écriture Null avec format explicite) : le format n'atteint jamais le Null
        assert_eq!(render(r#"{{ json_to_str null format="toml" }}"#), "");
    }

    // ── Scenario « str_to_json rend une valeur et json_to_str sa chaîne » ──
    #[test]
    fn str_to_json_renders_navigable_value_and_roundtrip_compares_parsed() {
        // membre 1 : le rendu est la valeur JSON elle-même (navigable dans le template),
        // pas sa chaîne échappée — un objet rendu en chaîne ne résoudrait pas `foo`
        assert_eq!(
            render(r#"{{#with (str_to_json "{\"foo\":true}")}}{{foo}}{{/with}}"#),
            "true"
        );
        // membres 2 + 3 : le nidé rend la chaîne compacte et l'assertion compare des
        // @serde_json::Value parsées — l'ordre des clés n'est pas contractuel
        let out = render(r#"{{ json_to_str ( str_to_json "{\"foo\":true,\"zed\":2}" ) }}"#);
        assert_eq!(
            serde_json::from_str::<Json>(&out).unwrap(),
            serde_json::json!({"foo": true, "zed": 2}),
        );
    }

    // ── Scenario « le paramètre chaîne manquant ou non textuel parle » ──
    #[test]
    fn missing_string_params_report_their_position() {
        // membre 1 : aucune position → param 0
        assert_eq!(
            other_msg(&render_err(r"{{ json_str_query }}")),
            "param 0 (the string) not found"
        );
        // membre 2 : un seul paramètre → la même message cite la position 1
        assert_eq!(
            other_msg(&render_err(r#"{{ json_str_query "x" }}"#)),
            "param 1 (the string) not found"
        );
    }

    // Tâche 2 de hbs_json.sdd : `find_str_param` doit refuser le non-chaîne en
    // `param 0 (the string) is not a string` au lieu de `unwrap_or("")`. L'existant
    // coerce `5` en chaîne vide et le gabarit rend "" — rouge jusqu'à la tâche 2.
    #[test]
    #[ignore = "attend la tâche 2 de hbs_json.sdd : find_str_param coerce encore le non-chaîne en \"\""]
    fn non_string_string_param_is_not_coerced_to_empty() {
        // membre 3 : un nombre en position de chaîne échoue, sans coercition
        assert_eq!(
            other_msg(&render_err(r"{{ str_to_json 5 }}")),
            "param 0 (the string) is not a string"
        );
    }

    // ── Scenario « la rognure --- precède le yaml écrit seulement » ──
    #[test]
    fn yaml_doc_prefix_is_trimmed_on_write_only() {
        // membre 1 : écriture yaml → `a: 1` immédiatement, sans préfixe de document
        assert_eq!(render(r#"{{ json_to_str {"a":1} format="yaml" }}"#), "a: 1\n");
        // membre 2 : à la lecture, `---\na: 1\n` est accepté sans aucune découpe
        assert_eq!(
            render(r#"{{ json_to_str ( str_to_json "---\na: 1\n" format="yaml" ) }}"#),
            r#"{"a":1}"#
        );
    }

    // ── Scenario « json_query rend la valeur nulle au non-match et l'erreur au mauvais chemin » ──
    #[test]
    fn json_query_non_match_renders_null_value() {
        // non une chaîne "null" : la valeur nulle JMESPath est faux dans un `#if`
        // (coercition exacte laissée à @jmespath 0.5.0)
        assert_eq!(
            render(r#"{{#if (json_query "missing.field" {"foo":"bar"})}}set{{else}}unset{{/if}}"#),
            "unset"
        );
    }

    #[test]
    fn json_query_invalid_expression_nests_jmespath_failure() {
        // membre 2 : NestedError portant JsonQueryFailure à l'affichage exact, source
        // @jmespath::JmespathError nichée telle quelle
        let err = render_err(r#"{{ json_query "foo..bar" {"foo":"bar"} }}"#);
        let inner = nested(&err);
        assert_eq!(inner.to_string(), "query failure for expression 'foo..bar'");
        assert!(
            inner
                .source()
                .is_some_and(|s| s.downcast_ref::<jmespath::JmespathError>().is_some()),
            "la source @jmespath::JmespathError doit rester nichée telle quelle"
        );
    }

    // ── Scenario « json_str_query force le scalaire en json trim et garde le format conteneur » ──
    #[test]
    fn json_str_query_scalars_force_trimmed_compact_json() {
        // membre 1 (booléen) : le format demandé (yaml) est IGNORE pour un scalaire : le
        // yaml aurait rendu "true\n" (et "---" rogné) ; equality verrouille json compact ET trim
        assert_eq!(
            render(r#"{{ json_str_query "foo.bar.baz" "foo:\n bar:\n  baz: true\n" format="yaml" }}"#),
            "true"
        );
        // membre 2 (chaîne, discriminant du membre 1) : le rendu porte les guillemets du
        // json compact — un impl défectueux qui garderait le format puis rognerait rendrait
        // `bar` sans guillemets (serde_yaml d'une chaîne n'en émet pas) ; equality exacte
        assert_eq!(
            render(r#"{{ json_str_query "a.b" "a:\n  b: bar\n" format="yaml" }}"#),
            r#""bar""#
        );
    }

    #[test]
    fn json_str_query_container_keeps_format_without_trim() {
        // membre 3 (tableau → yaml) : réencodé dans le format demandé (et non du json
        // compact), newline final compris — la forme conteneur ne rogne pas
        assert_eq!(
            render(r#"{{ json_str_query "foo.bar" "foo:\n bar:\n  - 1\n  - 2\n" format="yaml" }}"#),
            "- 1\n- 2\n"
        );
        // conteneur objet en toml : réencodé en toml (`baz = true\n`, forme impossible en
        // json), newline finale conservée — pas de trim
        assert_eq!(
            render(r#"{{ json_str_query "foo" "[foo]\nbaz=true\n" format="toml" }}"#),
            "baz = true\n"
        );
    }

    // membre 4 (`But`) : conteneur non-table en toml — @toml 0.8 exige une table racine
    // et son sérialiseur retourne son `UnsupportedType` (branche `into_table`). L'échec
    // remonte en Reason::NestedError nichant l'erreur d'écriture @toml, nature verrouillée.
    #[test]
    fn json_str_query_array_container_fails_as_toml_document_root() {
        let err = render_err(r#"{{ json_str_query "foo.bar" "foo.bar=[1,2]\n" format="toml" }}"#);
        let inner = nested(&err);
        assert!(
            inner.downcast_ref::<toml::ser::Error>().is_some(),
            "l'erreur d'écriture @toml (UnsupportedType) doit rester nichée telle quelle, \
             obtenu {inner:?}"
        );
    }

    // ── Scenario « to_json lit le format demandé et écrit du json joli » (membres non
    // couverts par `to_json_block_wraps_rendered_content`) ──
    #[test]
    fn to_json_reads_the_requested_format_and_fails_on_unparsable_content() {
        // membre 1 : contenu lu en toml, écriture toujours json_pretty (indentation deux espaces)
        assert_eq!(
            render("{{#to_json format=\"toml\"}}[foo.bar]\nbaz=true\n{{/to_json}}"),
            "{\n  \"foo\": {\n    \"bar\": {\n      \"baz\": true\n    }\n  }\n}"
        );
        // membre 3 (`But`) : contenu non vide qui ne parse pas dans le format demandé échoue
        let err = render_err(r#"{{#to_json format="json"}}not json{{/to_json}}"#);
        assert!(
            nested(&err).downcast_ref::<serde_json::Error>().is_some(),
            "l'échec de lecture doit remonter la nature de l'erreur du parser"
        );
    }

    // ── Scenario « from_json lit dur json et écrit dans le format » (membres non couverts
    // par `from_json_block_converts_to_yaml`) ──
    #[test]
    fn from_json_writes_toml_and_reads_body_as_hard_json() {
        // membre 2 : sortie toml avec la table repoussée selon l'ordonnancement local
        assert_eq!(
            render(r#"{{#from_json format="toml"}}{"foo":{"bar":true}}{{/from_json}}"#),
            "[foo]\nbar = true\n"
        );
        // membre 3 (`And`) : le lecteur de corps est JSON en dur — une erreur de corps
        // est un @serde_json::Error quel que soit `format`, jamais une erreur yaml
        let err = render_err(r#"{{#from_json format="yaml"}}not json{{/from_json}}"#);
        let inner = nested(&err);
        assert!(
            inner.downcast_ref::<serde_json::Error>().is_some(),
            "le corps se lit en JSON dur : la nature de l'erreur est @serde_json"
        );
        assert!(inner.downcast_ref::<serde_yaml::Error>().is_none());
    }

    // ── Scenario « vers toml les null sortent et les tables descendent » (membres
    // observables dans cette build de @toml) ──
    #[test]
    fn toml_conversion_drains_nulls_and_pushes_tables_last() {
        // membres 1–2 : `b` disparaît, `arr` perd son élément null, le reste survit
        let converted = to_ordored_toml_value(&serde_json::json!(
            {"z": 1, "b": null, "arr": [1, null], "tab": {"k": 2}}
        ))
        .unwrap()
        .expect("la table non vide survit à l'égouttage");
        let toml::Value::Table(table) = &converted else {
            panic!("attendait une table, obtenu {converted:?}");
        };
        assert!(!table.contains_key("b"), "la clé null doit disparaître");
        assert_eq!(
            table.get("arr"),
            Some(&toml::Value::Array(vec![toml::Value::Integer(1)])),
            "le tableau doit perdre son élément null"
        );
        assert_eq!(
            table.get("tab"),
            Some(&{
                let mut t = Table::new();
                t.insert("k".to_owned(), toml::Value::Integer(2));
                toml::Value::Table(t)
            })
        );
        // membre 3 (`And`) : l'ordre observable est celui de la SÉRIALISATION @toml —
        // scalaires avant sous-tables — pas l'ordre d'insertion du tri local (@toml sans
        // `preserve_order` backingue en BTreeMap). Verrou : la sortie sérialisée exacte,
        // pas une liste de clés.
        let converted = to_ordored_toml_value(&serde_json::json!({"tab": {"k": 2}, "z": 1}))
            .unwrap()
            .expect("conversion");
        assert_eq!(toml::to_string(&converted).unwrap(), "z = 1\n\n[tab]\nk = 2\n");
    }

    // ── Scenario « le paramètre valeur de json_to_str n'a pas besoin d'être une chaîne »
    // (l'objet littéral est verrouillé par `json_to_str_roundtrip`) ──
    #[test]
    fn json_to_str_value_param_takes_any_value_and_names_the_missing_one() {
        // membre 1 : non-chaînes acceptées brutes (le littéral objet est ailleurs)
        assert_eq!(render(r"{{ json_to_str 5 }}"), "5");
        assert_eq!(render(r"{{ json_to_str true }}"), "true");
        // membre 2 : invocation sans paramètre → message exact nommant `the json`
        assert_eq!(
            other_msg(&render_err(r"{{ json_to_str }}")),
            "param 0 (the json) not found"
        );
    }

    // ── Scenario « block sans bloc et corps vide ne craschent pas » ──
    #[test]
    fn block_helpers_without_body_render_empty() {
        // corps vide : lecture de la chaîne vide court-circuitée à l'écriture, quel que soit `format`
        assert_eq!(render(r"{{#from_json}}{{/from_json}}"), "");
        assert_eq!(render(r#"{{#from_json format="toml"}}{{/from_json}}"#), "");
        assert_eq!(render(r"{{#to_json}}{{/to_json}}"), "");
        assert_eq!(render(r#"{{#to_json format="yaml"}}{{/to_json}}"#), "");
        // absence de bloc (template() == None) : même cour-circuit, jamais d'erreur d'absence
        assert_eq!(render(r"{{from_json}}"), "");
        assert_eq!(render(r"{{to_json}}"), "");
    }

    // ── Scenario « les six noms vivent dans le HandleBars public après new_hbs » ──
    #[test]
    fn public_engine_resolves_the_six_unescaped_and_register_is_idempotent() {
        let mut hb = crate::hbs::HandleBars::new();
        // Le `no_escape` vient de @new_hbs en amont (./hbs.rs::new) : ce module ne pose
        // jamais l'échappement — seul l'auxiliaire local `render` le pose explicitement.
        // Le chevron doit rester non échappé dans le moteur public.
        let out = hb
            .render(r#"{{ json_to_str "{\"x\":\"<y>\"}" }}"#, &Json::Null)
            .unwrap();
        assert_eq!(out, r#""{\"x\":\"<y>\"}""#);
        assert!(!out.contains("&lt;"), "le chevron doit rester non échappé");
        // les six noms se résolvent par un template sur le moteur public
        assert_eq!(
            hb.render(r#"{{ json_to_str {"a":1} }}"#, &Json::Null).unwrap(),
            r#"{"a":1}"#
        );
        assert_eq!(
            hb.render(
                r#"{{#with (str_to_json "{\"foo\":true}")}}{{foo}}{{/with}}"#,
                &Json::Null
            )
            .unwrap(),
            "true"
        );
        assert_eq!(
            hb.render(
                r#"{{#from_json format="yaml"}}{"a":1}{{/from_json}}"#,
                &Json::Null
            )
            .unwrap(),
            "a: 1\n"
        );
        assert_eq!(
            hb.render(r#"{{#to_json}}{"a":1}{{/to_json}}"#, &Json::Null)
                .unwrap(),
            "{\n  \"a\": 1\n}"
        );
        assert_eq!(
            hb.render(r#"{{ json_to_str ( json_query "a" {"a":[1]} ) }}"#, &Json::Null)
                .unwrap(),
            "[1]"
        );
        assert_eq!(
            hb.render(r#"{{ json_str_query "a" "a: 1\n" format="yaml" }}"#, &Json::Null)
                .unwrap(),
            "1"
        );
        // `register` rappelé deux fois : remplacement des homonymes, sans accumulation ni erreur
        super::register(hb.engine_mut());
        assert_eq!(
            hb.render(r#"{{ json_to_str {"a":1} }}"#, &Json::Null).unwrap(),
            r#"{"a":1}"#
        );
        assert_eq!(
            hb.render(r#"{{#to_json}}{"a":1}{{/to_json}}"#, &Json::Null)
                .unwrap(),
            "{\n  \"a\": 1\n}"
        );
        assert_eq!(
            hb.render(r#"{{ json_str_query "a" "a: 1\n" format="yaml" }}"#, &Json::Null)
                .unwrap(),
            "1"
        );
    }

    // auxiliaires d'échec : la forme d'habillage finale (préfixe @handlebars) est exclue,
    // on assert sur le `RenderErrorReason` nu
    fn render_err(tmpl: &str) -> RenderError {
        let mut hb = Handlebars::new();
        hb.register_escape_fn(handlebars::no_escape);
        register(&mut hb);
        hb.render_template(tmpl, &Json::Null)
            .expect_err("le gabarit devait échouer")
    }

    fn other_msg(err: &RenderError) -> String {
        match err.reason() {
            RenderErrorReason::Other(msg) => msg.clone(),
            other => panic!("attendait Reason::Other, obtenu {other:?}"),
        }
    }

    fn nested<'a>(err: &'a RenderError) -> &'a (dyn std::error::Error + 'static) {
        match err.reason() {
            RenderErrorReason::NestedError(inner) => &**inner,
            other => panic!("attendait Reason::NestedError, obtenu {other:?}"),
        }
    }
}
