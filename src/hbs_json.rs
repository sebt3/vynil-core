//! `json_to_str`/`str_to_json`/`from_json`/`to_json`/`json_query`/`json_str_query` Handlebars
//! helpers.
//!
//! Vendored (not depended on) from `handlebars_misc_helpers` 0.17.0's `json_helpers.rs`
//! (CC0-1.0, https://github.com/davidB/handlebars_misc_helpers) rather than pulled in as a
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
            if v.is_i64() {
                Ok(Some(toml::Value::Integer(v.as_i64().unwrap())))
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
            .map(|v| v.value())?;
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
        .map(|t| t.render(r, ctx, rc, &mut content))
        .unwrap_or(Ok(()))?;
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
        .map(|t| t.render(r, ctx, rc, &mut content))
        .unwrap_or(Ok(()))?;
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
        assert_eq!(render(r##"{{ json_to_str "" }}"##), "");
        assert_eq!(render(r##"{{ str_to_json "" }}"##), "");
        assert_eq!(render(r##"{{ json_query "foo" "" }}"##), "");
        assert_eq!(render(r##"{{ json_str_query "foo" "" }}"##), "");
    }

    #[test]
    fn null_input_returns_empty() {
        assert_eq!(render(r##"{{ json_to_str null }}"##), "");
        assert_eq!(render(r##"{{ str_to_json null }}"##), "");
    }

    #[test]
    fn json_to_str_roundtrip() {
        assert_eq!(render(r##"{{ json_to_str {} }}"##), "{}");
        assert_eq!(
            render(r##"{{ json_to_str {"foo":{"bar":{"baz":true}}} }}"##),
            r##"{"foo":{"bar":{"baz":true}}}"##
        );
        assert_eq!(
            render(r##"{{ json_to_str ( str_to_json "{\"foo\":true}" ) }}"##),
            r##"{"foo":true}"##
        );
    }

    #[test]
    fn json_query_extracts_field() {
        assert_eq!(
            render(r##"{{ json_to_str ( json_query "foo" {"foo":{"bar":{"baz":true}}} ) }}"##),
            r##"{"bar":{"baz":true}}"##
        );
    }

    #[test]
    fn json_str_query_on_yaml() {
        assert_eq!(
            render(r##"{{ json_str_query "foo.bar.baz" "foo:\n bar:\n  baz: true\n" format="yaml"}}"##),
            "true"
        );
    }

    #[test]
    fn json_str_query_on_toml() {
        assert_eq!(
            render(r##"{{ json_str_query "foo.bar.baz" "[foo.bar]\nbaz=true\n" format="toml"}}"##),
            "true"
        );
    }

    #[test]
    fn to_json_block_wraps_rendered_content() {
        assert_eq!(
            render(r##"{{#to_json}}{"foo":{"bar":{"baz":true}}}{{/to_json}}"##),
            "{\n  \"foo\": {\n    \"bar\": {\n      \"baz\": true\n    }\n  }\n}"
        );
    }

    #[test]
    fn from_json_block_converts_to_yaml() {
        assert_eq!(
            render(r##"{{#from_json format="yaml"}}{"foo":{"bar":true}}{{/from_json}}"##),
            "foo:\n  bar: true\n"
        );
    }

    #[test]
    fn data_format_symmetry() {
        for (fmt, data) in [
            (DataFormat::Json, r##"{"foo":{"bar":{"baz":true}}}"##),
            (DataFormat::Toml, "[foo.bar]\nbaz = true\n"),
        ] {
            let actual = fmt.write_string(&fmt.read_string(data).unwrap()).unwrap();
            assert_eq!(actual, data);
        }
    }
}
