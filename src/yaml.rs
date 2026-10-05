//! YAML ↔ JSON / Rhai helpers.
//!
//! The Rust helpers (`yaml_str_to_json`, `yaml_serialize_to_string`) are always available;
//! the Rhai helpers (`yaml_encode`, `yaml_decode`, `yaml_decode_multi`) are registered by
//! `yaml_rhai_register` when the `rhai` feature is enabled.

use crate::{Error, Result};
#[cfg(feature = "rhai")] use crate::{RhaiRes, rhai_err};
#[cfg(feature = "rhai")] use rhai::{Dynamic, Engine, ImmutableString, Map};

/// Parses a YAML string to a `serde_json::Value`.
///
/// # Errors
///
/// Returns [`Error::YamlError`] when the input is not valid YAML or cannot be converted.
pub fn yaml_str_to_json(s: &str) -> Result<serde_json::Value> {
    serde_yaml::from_str(s).map_err(|e| Error::YamlError(e.to_string()))
}

/// Serialises any `serde::Serialize` value to a YAML string.
///
/// # Errors
///
/// Returns [`Error::YamlError`] when the value cannot be serialised to YAML.
pub fn yaml_serialize_to_string<T: serde::Serialize>(val: &T) -> Result<String> {
    serde_yaml::to_string(val).map_err(|e| Error::YamlError(e.to_string()))
}

/// Serialises a slice of `serde::Serialize` values as a multi-document YAML string.
///
/// # Errors
///
/// Returns [`Error::YamlError`] when one of the values cannot be serialised to YAML.
pub fn yaml_all_serialize_to_string<T: serde::Serialize>(vals: &[T]) -> Result<String> {
    let mut out = String::new();
    for v in vals {
        out.push_str("---\n");
        out.push_str(&serde_yaml::to_string(v).map_err(|e| Error::YamlError(e.to_string()))?);
    }
    Ok(out)
}

/// Converts an internal `serde_yaml::Value` into a `rhai::Dynamic`, the null scalar becoming
/// the unit value — `Dynamic` cannot visit a null presented as `visit_none` (empty document,
/// comment-only document). Scalar mapping (bool / integer / float / string / sequence /
/// mapping) is left to serde; nothing is corrected or normalised here. Fails only through
/// `yaml_key_to_string`, on a non-scalar mapping key.
#[cfg(feature = "rhai")]
fn dynamic_from_yaml_value(value: serde_yaml::Value) -> RhaiRes<Dynamic> {
    match value {
        serde_yaml::Value::Null => Ok(Dynamic::UNIT),
        serde_yaml::Value::Bool(b) => Ok(b.into()),
        serde_yaml::Value::Number(n) => Ok(n
            .as_i64()
            .map_or_else(|| n.as_f64().map_or(Dynamic::UNIT, Into::into), Into::into)),
        serde_yaml::Value::String(s) => Ok(s.into()),
        serde_yaml::Value::Sequence(seq) => Ok(seq
            .into_iter()
            .map(dynamic_from_yaml_value)
            .collect::<RhaiRes<rhai::Array>>()?
            .into()),
        serde_yaml::Value::Mapping(map) => {
            let mut m = Map::new();
            for (position, (k, v)) in map.into_iter().enumerate() {
                m.insert(
                    yaml_key_to_string(k, position)?.into(),
                    dynamic_from_yaml_value(v)?,
                );
            }
            Ok(m.into())
        }
        // Tag dropped, content rendered bare: Dynamic carries no tag. Measured behaviour
        // change, not parity — the direct-Dynamic path this replaced failed tagged
        // documents with `invalid type: enum` instead of rendering the content.
        serde_yaml::Value::Tagged(tagged) => dynamic_from_yaml_value(tagged.value),
    }
}

/// Stringifies a mapping key the way `serde_yaml` presents scalar keys to string visitors:
/// `1: un` yields the string key `1`, `true: un` the string key `true`. Non-scalar keys
/// (sequence, mapping) are refused with an error naming the key position: a `Debug` form
/// would become a visible, script-searchable key name (measured: `Sequence [Number(1), …]`).
#[cfg(feature = "rhai")]
fn yaml_key_to_string(key: serde_yaml::Value, position: usize) -> RhaiRes<String> {
    match key {
        serde_yaml::Value::String(s) => Ok(s),
        serde_yaml::Value::Number(n) => Ok(n.to_string()),
        serde_yaml::Value::Bool(b) => Ok(b.to_string()),
        serde_yaml::Value::Null => Ok("~".to_string()),
        // Tag stripped like on the value side; the content then decides.
        serde_yaml::Value::Tagged(tagged) => yaml_key_to_string(tagged.value, position),
        serde_yaml::Value::Sequence(_) | serde_yaml::Value::Mapping(_) => Err(rhai_err(Error::YamlError(
            format!("non-scalar mapping key at position {position}"),
        ))),
    }
}

/// Registers the `yaml_encode` / `yaml_decode` / `yaml_decode_multi` helpers on a Rhai `engine`.
///
/// # Errors
///
/// This function itself never fails: the four registrations cannot error. Errors belong to the
/// helpers it registers, which report `serde_yaml` rejections to the script as a catchable
/// `YamlError: <message>` Rhai error — the same text [`yaml_str_to_json`] raises as
/// [`Error::YamlError`].
#[cfg(feature = "rhai")]
pub fn yaml_rhai_register(engine: &mut Engine) {
    engine
        .register_fn("yaml_encode", |val: Dynamic| -> RhaiRes<ImmutableString> {
            serde_yaml::to_string(&val)
                .map_err(|e| rhai_err(Error::YamlError(e.to_string())))
                .map(std::convert::Into::into)
        })
        .register_fn("yaml_encode", |val: Map| -> RhaiRes<ImmutableString> {
            serde_yaml::to_string(&val)
                .map_err(|e| rhai_err(Error::YamlError(e.to_string())))
                .map(std::convert::Into::into)
        })
        .register_fn("yaml_decode", |val: ImmutableString| -> RhaiRes<Dynamic> {
            serde_yaml::from_str::<serde_yaml::Value>(val.as_ref())
                .map_err(|e| rhai_err(Error::YamlError(e.to_string())))
                .and_then(dynamic_from_yaml_value)
        })
        .register_fn(
            "yaml_decode_multi",
            |val: ImmutableString| -> RhaiRes<Vec<Dynamic>> {
                let mut result = Vec::new();
                for doc in serde_yaml::Deserializer::from_str(val.as_ref()) {
                    let v: serde_yaml::Value = serde::Deserialize::deserialize(doc)
                        .map_err(|e| rhai_err(Error::YamlError(e.to_string())))?;
                    result.push(dynamic_from_yaml_value(v)?);
                }
                Ok(result)
            },
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    // Scenario « chaîne vide, `null` côté Rust comme côté script » (yaml.sdd l.351-355),
    // face Rust : `yaml_str_to_json("")` rend `serde_json::Value::Null` — la concordance
    // avec l'unité côté script est jouée en `mod script` ci-dessous.
    #[test]
    fn scenario_chaine_vide_rust_face_null() {
        assert_eq!(yaml_str_to_json("").unwrap(), serde_json::Value::Null);
    }

    #[cfg(feature = "rhai")]
    mod script {
        use super::*;

        // Engine fraîchement enregistré par `yaml_rhai_register` (spec : c'est l'enregistreuse
        // qui est contractuelle, pas `Script::new_bare` — réservé au Scenario d'intégration).
        fn engine() -> Engine {
            let mut e = Engine::new();
            crate::yaml::yaml_rhai_register(&mut e);
            e
        }

        // Rend l'`array` de script retourné par un snippet qui construit `[...]`.
        fn eval_array(e: &Engine, script: &str) -> rhai::Array {
            e.eval::<Dynamic>(script)
                .unwrap_or_else(|err| panic!("évaluation échouée: {err}"))
                .try_cast::<rhai::Array>()
                .expect("le snippet devait rendre un array")
        }

        // Reproduit le refus `serde_yaml` attendu sur `input` : même itérateur que la production,
        // borné à `n` documents — l'itérateur de `serde_yaml::Deserializer` rechaine
        // indéfiniment `Some(Err)` après une faute (`Progress::Fail`, de.rs), la production
        // n'en voit jamais qu'un grâce à la sortie par `?` à la première erreur.
        fn refus_serde_yaml(input: &str, n: usize) -> String {
            let mut refus = String::new();
            for doc in serde_yaml::Deserializer::from_str(input).take(n) {
                if let Err(e) = <serde_yaml::Value as serde::Deserialize>::deserialize(doc) {
                    refus = e.to_string();
                }
            }
            refus
        }

        // Scenario « scalaire null décodé en unité » (yaml.sdd l.345-349) :
        // `type_of(yaml_decode("~"))` rend `()` et `yaml_decode("nothing: \n")["nothing"]`
        // rend l'unité de même (Must l.77-82 : null → unité, jamais une erreur).
        // À l'état seuil : déjà vert — serde_yaml présente le scalaire `~` par `visit_unit`,
        // que `Dynamic` sait visiter ; l'attendu est de le rester par la voie Value.
        #[test]
        fn scenario_scalaire_null_decode_en_unite() {
            let e = engine();
            let t = e.eval::<Dynamic>(r#"type_of(yaml_decode("~"))"#).unwrap();
            assert_eq!(
                t.as_immutable_string_ref().unwrap().as_str(),
                "()",
                "`yaml_decode(\"~\")` devait rendre l'unité"
            );
            let v = e
                .eval::<Dynamic>(r#"yaml_decode("nothing: \n")["nothing"]"#)
                .unwrap();
            assert!(v.is_unit(), "`nothing:` devait rendre l'unité, obtenu {v:?}");
        }

        // Scenario « chaîne vide, `null` côté Rust comme côté script » (yaml.sdd l.351-355),
        // face script : `yaml_decode("")` rend l'unité sans erreur. À l'état seuil : ROUGE —
        // `Dynamic` ne visite pas le null d'un document vide (`invalid type: Option value`).
        #[test]
        fn scenario_chaine_vide_script_face_unite() {
            let e = engine();
            let v = e
                .eval::<Dynamic>(r#"yaml_decode("")"#)
                .expect("document vide jamais une erreur");
            assert!(
                v.is_unit(),
                "`yaml_decode(\"\")` devait rendre l'unité, obtenu {v:?}"
            );
        }

        // Scenario « commentaire seul, unité sans erreur » (yaml.sdd l.357-361) :
        // `yaml_decode` rend l'unité et `yaml_decode_multi` ne lève aucune erreur — contenu
        // figé à un élément unité, mesure de ce qu'énumère serde_yaml pour un commentaire
        // seul (Must l.79-86). À l'état seuil : ROUGE sur les deux faces (null non visité).
        #[test]
        fn scenario_commentaire_seul_unite_sans_erreur() {
            let e = engine();
            let v = e
                .eval::<Dynamic>(r##"yaml_decode("# rien sous le radar\n")"##)
                .expect("commentaire seul jamais une erreur");
            assert!(
                v.is_unit(),
                "commentaire seul devait rendre l'unité, obtenu {v:?}"
            );
            let docs = e
                .eval::<Dynamic>(r##"yaml_decode_multi("# rien sous le radar\n")"##)
                .expect("commentaire seul ne devait lever aucune erreur")
                .try_cast::<rhai::Array>()
                .expect("le multi rend un array");
            assert_eq!(
                docs.len(),
                1,
                "serde_yaml énumère un document pour un commentaire seul"
            );
            assert!(
                docs[0].is_unit(),
                "le document commentaire seul devait rendre l'unité"
            );
        }

        // Scenario « documents courts jamais ignorés » (yaml.sdd l.363-367) — LE test du seuil
        // `val.len() <= 5` (Must l.75-76 : « aucune entrée valide n'est ignorée, quelle que
        // soit sa taille »). À l'état seuil : ROUGE — les quatre premières entrées rendent
        // `[]`. L'entrée `ab` est celle de `test_yaml_decode_multi_short_string_returns_empty`,
        // supprimé avec le quirk (yaml.sdd l.238-243) — ce Scenario en tient lieu.
        #[test]
        fn scenario_documents_courts_jamais_ignores() {
            let e = engine();
            // `yaml_decode_multi("a: 1")` → [1, "array", "map", 1] : un élément, map #{a: 1}.
            let out = eval_array(
                &e,
                r#"let d = yaml_decode_multi("a: 1"); [d.len(), type_of(d), type_of(d[0]), d[0]["a"]]"#,
            );
            assert_eq!(out[0].as_int().unwrap(), 1, "`\"a: 1\"` devait rendre 1 document");
            assert_eq!(out[1].as_immutable_string_ref().unwrap().as_str(), "array");
            assert_eq!(out[2].as_immutable_string_ref().unwrap().as_str(), "map");
            assert_eq!(out[3].as_int().unwrap(), 1);

            // `yaml_decode_multi("42")` → un élément, l'entier 42.
            let out = eval_array(&e, r#"let d = yaml_decode_multi("42"); [d.len(), d[0]]"#);
            assert_eq!(out[0].as_int().unwrap(), 1, "`\"42\"` devait rendre 1 document");
            assert!(
                out[1].is_int(),
                "`42` devait rester un entier, obtenu {:?}",
                out[1]
            );
            assert_eq!(out[1].as_int().unwrap(), 42);

            // `yaml_decode_multi("ok: 1")` → un élément, map #{ok: 1}.
            let out = eval_array(&e, r#"let d = yaml_decode_multi("ok: 1"); [d.len(), d[0]["ok"]]"#);
            assert_eq!(
                out[0].as_int().unwrap(),
                1,
                "`\"ok: 1\"` devait rendre 1 document"
            );
            assert_eq!(out[1].as_int().unwrap(), 1);

            // `ab` : un élément, la chaîne `ab`.
            let out = eval_array(
                &e,
                r#"let d = yaml_decode_multi("ab"); [d.len(), type_of(d[0]), d[0]]"#,
            );
            assert_eq!(out[0].as_int().unwrap(), 1, "`\"ab\"` devait rendre 1 document");
            assert_eq!(out[1].as_immutable_string_ref().unwrap().as_str(), "string");
            assert_eq!(out[2].as_immutable_string_ref().unwrap().as_str(), "ab");

            // `yaml_decode_multi("---\n")` : pas d'erreur, contenu figé par mesure — serde_yaml
            // énumère UN document pour `---\n` (mesuré sur la voie directe, comptage de
            // `Deserializer::from_str("---\n")`), rendu unité (Must l.79-86, Scenario l.367).
            let docs = e
                .eval::<Dynamic>(r#"yaml_decode_multi("---\n")"#)
                .expect("`\"---\\n\"` ne devait pas rendre d'erreur")
                .try_cast::<rhai::Array>()
                .expect("`yaml_decode_multi` rend un array");
            assert_eq!(
                docs.len(),
                1,
                "nombre de documents énumérés par serde_yaml pour `\"---\\n\"` (mesuré)"
            );
            assert!(docs[0].is_unit(), "le document `---` devait rendre l'unité");
        }

        // Scenario « deux documents rendus par `yaml_decode_multi` » (yaml.sdd l.369-373) :
        // ancre du chemin déjà juste (> 5 octets), verrouillée contre toute régression du
        // nouveau chemin serde_yaml::Value → Dynamic.
        #[test]
        fn scenario_deux_documents() {
            let e = engine();
            let out = eval_array(
                &e,
                r#"let docs = yaml_decode_multi("key: first\n---\nkey: second\n"); [docs.len(), docs[1]["key"]]"#,
            );
            assert_eq!(out[0].as_int().unwrap(), 2);
            assert_eq!(out[1].as_immutable_string_ref().unwrap().as_str(), "second");

            let out = eval_array(&e, r#"let d = yaml_decode_multi("key: val\n"); [d.len()]"#);
            assert_eq!(out[0].as_int().unwrap(), 1);
        }

        // Scenario « multi-document refusé par `yaml_decode` » (yaml.sdd l.375-379) :
        // l'évaluation échoue et le texte porte le refus serde_yaml. Ancre : déjà vert à
        // l'état seuil, verrouillé à travers le changement de voie.
        #[test]
        fn scenario_multi_document_refuse_par_yaml_decode() {
            let e = engine();
            let err = e
                .eval::<Dynamic>(r#"yaml_decode("a: 1\n---\nb: 2\n")"#)
                .expect_err("`yaml_decode` doit refuser plus d'un document");
            assert!(
                err.to_string()
                    .contains("deserializing from YAML containing more than one document is not supported"),
                "le texte portait : {err}"
            );
        }

        // Scenario « document fautif au milieu du multi » (yaml.sdd l.381-385) : message
        // `YamlError: ` + refus serde_yaml (Must l.36-42 et l.71-74), et le document valide
        // déjà parcouru n'est restitué sous aucune forme (Must l.83-84 : les documents
        // rassemblés sont jetés avec l'erreur). Ancre : déjà vert à l'état seuil.
        #[test]
        fn scenario_document_fautif_au_milieu_du_multi() {
            let e = engine();
            let err = e
                .eval::<Dynamic>(r#"yaml_decode_multi("a: 1\n---\nbroken: [\n")"#)
                .expect_err("le second document fautif doit faire échouer le multi");
            let msg = err.to_string();
            let attendu = format!("YamlError: {}", refus_serde_yaml("a: 1\n---\nbroken: [\n", 2));
            assert!(
                msg.contains(&attendu),
                "message attendu `{attendu}`, obtenu : {msg}"
            );
            assert!(
                !msg.contains("a: 1"),
                "le document valide déjà parcouru ne devait être restitué nulle part : {msg}"
            );
        }

        // Must l.83-86 (« Un document null au milieu d'un flux rend l'unité à sa position ») :
        // flux de trois documents dont un `~` au milieu — la POSITION de l'unité est verrouillée,
        // pas seulement sa présence.
        #[test]
        fn scenario_flux_mixte_unite_a_sa_position() {
            let e = engine();
            let out = eval_array(
                &e,
                r#"let d = yaml_decode_multi("a: 1\n---\n~\n---\nb: 2\n"); [d.len(), d[0]["a"], type_of(d[1]), d[2]["b"]]"#,
            );
            assert_eq!(out[0].as_int().unwrap(), 3, "trois documents énumérés");
            assert_eq!(out[1].as_int().unwrap(), 1, "document 0 : #{{a: 1}}");
            assert_eq!(
                out[2].as_immutable_string_ref().unwrap().as_str(),
                "()",
                "document 1 (`~`) devait être l'unité À SA POSITION"
            );
            assert_eq!(out[3].as_int().unwrap(), 2, "document 2 : #{{b: 2}}");
        }

        // Scenario « un flux vide énumère un document unité » (yaml.sdd l.467-474) — les trois
        // longueurs à `1` et non `0`, l'élément de chacun unité (`type_of` rend `()`), aucun
        // sans erreur (Then + And), et `---\n---\n` à 2 éléments unité (second And). Comptage
        // mesuré de `serde_yaml::Deserializer::from_str`, décision actée (Must l.89-95).
        #[test]
        fn scenario_un_flux_vide_enumer_un_document_unite() {
            let e = engine();
            for (label, input) in [
                ("chaîne vide", ""),
                ("marqueur `---` seul", "---\n"),
                ("commentaires seuls", "# commentaires seuls\n"),
            ] {
                let out = eval_array(
                    &e,
                    &format!("let d = yaml_decode_multi({input:?}); [d.len(), type_of(d[0])]"),
                );
                assert_eq!(
                    out[0].as_int().unwrap(),
                    1,
                    "{label} : un document énuméré, pas zéro"
                );
                assert_eq!(
                    out[1].as_immutable_string_ref().unwrap().as_str(),
                    "()",
                    "{label} : l'élément unique devait être l'unité, sans erreur"
                );
            }
            let out = eval_array(
                &e,
                r#"let d = yaml_decode_multi("---\n---\n"); [d.len(), type_of(d[0]), type_of(d[1])]"#,
            );
            assert_eq!(
                out[0].as_int().unwrap(),
                2,
                "`\"---\\n---\\n\"` énumère 2 documents"
            );
            assert_eq!(
                out[1].as_immutable_string_ref().unwrap().as_str(),
                "()",
                "premier document `---` : unité"
            );
            assert_eq!(
                out[2].as_immutable_string_ref().unwrap().as_str(),
                "()",
                "second document `---` : unité"
            );
        }

        // Scenario « clés dupliquées refusées côté script, dernières gagnantes côté Rust »
        // (yaml.sdd l.476-487) — divergence contractée (Handles « Clés dupliquées », décision
        // actée) : le script refuse (`duplicate entry with key …`, contrôle de
        // `serde_yaml::Mapping` à la lecture), la face Rust garde la dernière (`serde_json`
        // écrase). NE PAS « réparer » : ce test rouge signale toute tentative d'alignement
        // d'une face sur l'autre, dans un sens comme dans l'autre.
        #[test]
        fn scenario_cles_dupliquees_refusees_cote_script_dernieres_gagnantes_cote_rust() {
            let e = engine();
            // When — `yaml_decode` simple : refus nommant la clé `a`.
            let err = e
                .eval::<Dynamic>(r#"yaml_decode("a: 1\na: 2\n")"#)
                .expect_err("le script doit refuser la clé dupliquée `a`");
            assert!(
                err.to_string().contains(r#"duplicate entry with key "a""#),
                "le texte devait porter le refus de la clé `a`, obtenu : {err}"
            );
            // When — map imbriquée : même forme sur la clé `b`.
            let err = e
                .eval::<Dynamic>(r#"yaml_decode("a:\n  b: 1\n  b: 2\n")"#)
                .expect_err("le script doit refuser la clé dupliquée imbriquée `b`");
            assert!(
                err.to_string().contains(r#"duplicate entry with key "b""#),
                "le texte devait porter le refus de la clé `b`, obtenu : {err}"
            );
            // When — `yaml_decode_multi` : la voie par document ne tolère pas le doublon non plus.
            let err = e
                .eval::<Dynamic>(r#"yaml_decode_multi("a: 1\na: 2\n")"#)
                .expect_err("`yaml_decode_multi` doit refuser le doublon comme `yaml_decode`");
            assert!(
                err.to_string().contains(r#"duplicate entry with key "a""#),
                "le multi devait porter le même refus, obtenu : {err}"
            );
            // But — face Rust : `{"a": 2}` sans erreur, last-wins ; la divergence est le contrat.
            assert_eq!(
                yaml_str_to_json("a: 1\na: 2\n")
                    .expect("la face Rust ne refuse pas les doublons (`serde_json` écrase)"),
                serde_json::json!({"a": 2}),
                "la face Rust doit rester last-wins, sans erreur"
            );
        }

        // Scenario « tag inconnu abandonné, contenu rendu » (yaml.sdd l.489-496) — le bras
        // `Tagged` de `dynamic_from_yaml_value` abandonne le tag et rend le contenu nu
        // (Handles « Tag inconnu », décision actée : la voie directe d'avant rendait
        // `invalid type: enum`). Le `And` sur `!!str 5` verrouille le bras reconnu, inchangé.
        #[test]
        fn scenario_tag_inconnu_abandonne_contenu_rendu() {
            let e = engine();
            // When — scalaire taggué : la chaîne `foo`, tag abandonné, sans erreur.
            let out = eval_array(&e, r#"let v = yaml_decode("!Custom\nfoo\n"); [type_of(v), v]"#);
            assert_eq!(
                out[0].as_immutable_string_ref().unwrap().as_str(),
                "string",
                "scalaire sous tag inconnu : contenu rendu nu"
            );
            assert_eq!(out[1].as_immutable_string_ref().unwrap().as_str(), "foo");
            // When — `a: !Custom` porteur du contenu nu (null → unité), la map rendue avec `b` à `1`.
            let out = eval_array(
                &e,
                r#"let m = yaml_decode("a: !Custom\nb: 1\n"); [type_of(m), m["b"], type_of(m["a"])]"#,
            );
            assert_eq!(
                out[0].as_immutable_string_ref().unwrap().as_str(),
                "map",
                "la map devait être rendue, sans erreur"
            );
            assert_eq!(out[1].as_int().unwrap(), 1, "`b` devait valoir `1`");
            assert_eq!(
                out[2].as_immutable_string_ref().unwrap().as_str(),
                "()",
                "contenu nu porté par `a` : null → unité"
            );
            // And — `!!str 5` : bras reconnu de `serde_yaml`, jamais le bras tag abandonné.
            let out = eval_array(&e, r#"let v = yaml_decode("!!str 5\n"); [type_of(v), v]"#);
            assert_eq!(
                out[0].as_immutable_string_ref().unwrap().as_str(),
                "string",
                "`!!str 5` devait rendre une chaîne"
            );
            assert_eq!(out[1].as_immutable_string_ref().unwrap().as_str(), "5");
        }

        // Scenario « clé de mapping non scalaire refusée sans forme Debug » (yaml.sdd l.498-507) —
        // LE changement de comportement de la tâche : `yaml_key_to_string` refuse séquence et
        // carte en erreur nommant la position, jamais leur représentation `Debug` (Handles
        // « Clé de mapping non scalaire », décision actée). ROUGE sur l'état actuel : le fallback
        // `Debug` rend encore une clé fabriquée. Le `But` verrouille les scalaires, qui ne se
        // font pas manger par le refus.
        #[test]
        fn scenario_cle_de_mapping_non_scalaire_refusee_sans_forme_debug() {
            let e = engine();
            // When — clé séquence (`? [1, 2]` puis `: v`) : échec nommément, aucune clé fabriquée.
            let err = e
                .eval::<Dynamic>(r#"yaml_decode("? [1, 2]\n: v\n")"#)
                .expect_err("une clé séquence doit être refusée, jamais rendue en forme Debug");
            let msg = err.to_string();
            assert!(
                msg.contains("YamlError: non-scalar mapping key at position"),
                "le refus devait nommer la position de la clé, obtenu : {msg}"
            );
            // Chiffre verrouillé — clé non scalaire SEULE dans le mapping : index d'entrée
            // 0-based, donc exactement `position 0` (tuerait un 1-based qui rendrait `1`).
            assert!(
                msg.contains("non-scalar mapping key at position 0"),
                "la clé séquence seule doit porter l'index d'entrée 0, obtenu : {msg}"
            );
            // And — AUCUNE chaîne de type Debug (`Sequence [Number` et ses cousins).
            assert!(
                !msg.contains("Sequence") && !msg.contains("Number"),
                "la représentation `Debug` d'une clé ne devient jamais un nom de clé : {msg}"
            );
            // When — clé carte (`? {a: 1}` puis `: v`) : échec de même nature.
            let err = e
                .eval::<Dynamic>(r#"yaml_decode("? {a: 1}\n: v\n")"#)
                .expect_err("une clé carte doit être refusée comme la clé séquence");
            let msg = err.to_string();
            assert!(
                msg.contains("YamlError: non-scalar mapping key at position"),
                "le refus de clé carte devait nommer la position, obtenu : {msg}"
            );
            assert!(
                !msg.contains("Mapping") && !msg.contains("SmartString"),
                "pas de forme Debug pour la clé carte non plus : {msg}"
            );
            // When — clé non scalaire PRÉCÉDÉE d'une entrée valide (`a: 1` puis `? [1, 2]` /
            // `: v`) : index d'entrée 1, verrouillé par le chiffre — un décompte 1-based
            // rendrait `position 2` et un compteur de lignes/colonnes rendrait autre chose.
            let err = e
                .eval::<Dynamic>(r#"yaml_decode("a: 1\n? [1, 2]\n: v\n")"#)
                .expect_err("une clé séquence précédée d'une entrée valide doit être refusée");
            let msg = err.to_string();
            assert!(
                msg.contains("YamlError: non-scalar mapping key at position 1"),
                "la deuxième entrée du mapping doit porter l'index 1, obtenu : {msg}"
            );
            // When — mapping imbriqué (`parent:` porteur d'un enfant dont la clé non scalaire
            // est la DEUXIÈME entrée) : position relative à l'enfant, pas au parent. Les deux
            // entrées `x`/`y` du parent avant `parent` tuent tout compteur global (qui rendrait
            // `position 4`).
            let err = e
                .eval::<Dynamic>(r#"yaml_decode("x: 1\ny: 2\nparent:\n  a: 1\n  ? [1, 2]\n  : v\n")"#)
                .expect_err("la clé non scalaire de l'enfant doit être refusée comme au niveau racine");
            let msg = err.to_string();
            assert!(
                msg.contains("YamlError: non-scalar mapping key at position 1"),
                "la position doit être relative au mapping enfant (2ᵉ entrée → 1), obtenu : {msg}"
            );
            // But — les scalaires restent des succès : `1:` et `true:` stringifiés en clés
            // chaîne `1` et `true`, seuls les scalaires sont stringifiés.
            let v = e
                .eval::<Dynamic>(r#"yaml_decode("1: un\n")["1"]"#)
                .expect("clé scalaire entière : succès attendu");
            assert_eq!(v.as_immutable_string_ref().unwrap().as_str(), "un");
            let v = e
                .eval::<Dynamic>(r#"yaml_decode("true: un\n")["true"]"#)
                .expect("clé scalaire booléenne : succès attendu");
            assert_eq!(v.as_immutable_string_ref().unwrap().as_str(), "un");
        }

        // Scenario « entier sous i64 refusé plutôt qu'inventé » (yaml.sdd l.509-515) — verrou
        // seul, rien n'est codé ici : c'est `serde_yaml` qui refuse sous `i64::MIN` (pas de
        // `f64` de secours) et bascule en `f64` au-dessus de `i64::MAX` (Handles « Nombre hors
        // de `i64` », décision actée — l'ancienne voie directe inventait silencieusement
        // `9223372036854775807`).
        #[test]
        fn scenario_entier_sous_i64_refuse_plutot_qu_invente() {
            let e = engine();
            // When/Then — sous `i64::MIN` : échec, et surtout pas la valeur inventée.
            let err = e
                .eval::<Dynamic>(r#"yaml_decode("-9223372036854775809\n")"#)
                .expect_err("sous i64::MIN, serde_yaml refuse : pas de f64 de secours");
            assert!(
                !err.to_string().contains("9223372036854775807"),
                "l'ancienne voie directe rendait i64::MAX inventé en dessous ; le refus ne doit \
                 pas la faire réapparaître : {err}"
            );
            // But — au-dessus de `i64::MAX` : un FLOTTANT (le type d'abord, verrou demandé).
            let v = e
                .eval::<Dynamic>(r#"yaml_decode("9223372036854775809\n")"#)
                .expect("au-dessus de i64::MAX, bascule en f64 sans erreur");
            assert!(
                v.is_float() && !v.is_int(),
                "au-dessus de i64::MAX, serde_yaml bascule en f64 : le rendu doit être un \
                 FLOTTANT et non un entier (la voie d'avant saturait à i64::MAX), obtenu \
                 type `{}` : {v:?}",
                v.type_name()
            );
        }
    }
}
