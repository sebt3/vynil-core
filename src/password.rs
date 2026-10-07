//! Password generation with per-class minimums.
//!
//! `generate` is the core Rust API; Rhai bindings (`gen_password`, `gen_password_alphanum`)
//! are gated behind the `password` feature.

use crate::{Error, Result};
use rand::{
    rng,
    seq::{IndexedRandom, SliceRandom},
};
#[cfg(feature = "rhai")] use rhai::{Engine, Map};

const LOWER: &[char] = &[
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u',
    'v', 'w', 'x', 'y', 'z',
];
const UPPER: &[char] = &[
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U',
    'V', 'W', 'X', 'Y', 'Z',
];
const DIGITS: &[char] = &['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];
const SYMBOLS: &[char] = &['!', '#', '%', '*', '+', '-', '.', ':', '=', '?', '@', '_'];

/// Generate a random password of `length` chars with at least `lower`/`upper`/`digits`/`symbols`
/// characters from each class. Symbols are `!#%*+-.:=?@_` (config-safe).
///
/// Returns `Error::PasswordSpec` if minimums exceed `length` or no class is enabled.
///
/// # Errors
///
/// Returns [`Error::PasswordSpec`] when the sum of class minimums exceeds `length`
/// (`PWD-SPEC-001`) or when no character class is enabled (`PWD-SPEC-002`).
pub fn generate(length: usize, lower: usize, upper: usize, digits: usize, symbols: usize) -> Result<String> {
    let classes: [(&[char], usize); 4] = [
        (LOWER, lower),
        (UPPER, upper),
        (DIGITS, digits),
        (SYMBOLS, symbols),
    ];
    let total_min: usize = classes.iter().map(|(_, m)| *m).sum();
    if total_min > length {
        return Err(Error::PasswordSpec(format!(
            "PWD-SPEC-001 sum of class minimums ({total_min}) exceeds requested length ({length})"
        )));
    }
    let pool: Vec<char> = classes
        .iter()
        .filter(|(_, m)| *m > 0)
        .flat_map(|(set, _)| set.iter().copied())
        .collect();
    if pool.is_empty() {
        return Err(Error::PasswordSpec(
            "PWD-SPEC-002 at least one character class must be enabled".into(),
        ));
    }
    let mut rng = rng();
    let mut chars: Vec<char> = Vec::with_capacity(length);
    for (set, m) in &classes {
        // Static alphabets, never empty; the skip branches are unreachable in practice.
        for _ in 0..*m {
            if let Some(c) = set.choose(&mut rng) {
                chars.push(*c);
            }
        }
    }
    while chars.len() < length {
        let Some(c) = pool.choose(&mut rng) else {
            break;
        };
        chars.push(*c);
    }
    chars.shuffle(&mut rng);
    Ok(chars.into_iter().collect())
}

/// Reads the minimum of `key` from a script spec map.
///
/// An absent key defaults to `1`; a present integer is clamped to `0` when
/// negative (excluding the class) and saturates to `usize::MAX` when it does
/// not fit. A present value that is not an integer is a script error
/// (`PWD-SPEC-003`, naming the faulty key) — never a silent fallback.
#[cfg(feature = "rhai")]
fn class_min(spec: &Map, key: &str) -> Result<usize> {
    match spec.get(key) {
        Some(v) => {
            let i = v.as_int().map_err(|_| {
                Error::PasswordSpec(format!("PWD-SPEC-003 spec key '{key}' must be an integer"))
            })?;
            Ok(usize::try_from(i.max(0)).unwrap_or(usize::MAX))
        }
        None => Ok(1),
    }
}

/// Registers the `gen_password` / `gen_password_alphanum` helpers on a Rhai `engine`.
#[cfg(feature = "rhai")]
pub fn password_rhai_register(engine: &mut Engine) {
    engine
        .register_fn("gen_password", |len: i64| -> crate::RhaiRes<String> {
            let length = usize::try_from(len.max(0)).unwrap_or(usize::MAX);
            generate(length, 1, 1, 1, 1).map_err(|e| format!("{e}").into())
        })
        .register_fn("gen_password", |len: i64, spec: Map| -> crate::RhaiRes<String> {
            let length = usize::try_from(len.max(0)).unwrap_or(usize::MAX);
            let read_min = |key: &str| -> crate::RhaiRes<usize> {
                class_min(&spec, key).map_err(|e| format!("{e}").into())
            };
            let lower = read_min("lower")?;
            let upper = read_min("upper")?;
            let digits = read_min("digits")?;
            let symbols = read_min("symbols")?;
            generate(length, lower, upper, digits, symbols).map_err(|e| format!("{e}").into())
        })
        .register_fn("gen_password_alphanum", |len: i64| -> crate::RhaiRes<String> {
            let length = usize::try_from(len.max(0)).unwrap_or(usize::MAX);
            generate(length, 1, 1, 1, 0).map_err(|e| format!("{e}").into())
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(s: &str, f: impl Fn(char) -> bool) -> usize {
        s.chars().filter(|c| f(*c)).count()
    }

    #[test]
    fn length_is_respected() {
        assert_eq!(generate(24, 1, 1, 1, 1).unwrap().chars().count(), 24);
    }

    #[test]
    fn guarantees_minimum_per_class() {
        let p = generate(32, 3, 2, 4, 5).unwrap();
        assert!(count(&p, |c| c.is_ascii_lowercase()) >= 3);
        assert!(count(&p, |c| c.is_ascii_uppercase()) >= 2);
        assert!(count(&p, |c| c.is_ascii_digit()) >= 4);
        assert!(count(&p, |c| SYMBOLS.contains(&c)) >= 5);
    }

    #[test]
    fn symbols_zero_yields_alphanumeric() {
        let p = generate(40, 1, 1, 1, 0).unwrap();
        assert!(p.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn single_class_only() {
        let p = generate(16, 0, 0, 1, 0).unwrap();
        assert!(p.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn symbol_set_is_config_safe() {
        let p = generate(200, 1, 1, 1, 50).unwrap();
        for bad in ['"', '\'', '\\', '`', '$'] {
            assert!(!p.contains(bad), "unsafe symbol {bad} leaked into password");
        }
    }

    #[test]
    fn errors_when_minimums_exceed_length() {
        assert!(generate(3, 1, 1, 1, 1).is_err());
    }

    #[test]
    fn errors_when_no_class_enabled() {
        assert!(generate(10, 0, 0, 0, 0).is_err());
    }

    #[test]
    fn two_passwords_differ() {
        assert_ne!(
            generate(24, 1, 1, 1, 1).unwrap(),
            generate(24, 1, 1, 1, 1).unwrap()
        );
    }

    // Le chemin `PWD-SPEC-003` naît de la lecture d'une `rhai::Map` : il n'est
    // observable que depuis un script, donc sous `rhai` seule (la glue de ce
    // module n'est pas gatée `password`).
    #[cfg(feature = "rhai")]
    mod script {
        use super::*;

        // Engine fraîchement enregistré par `password_rhai_register`.
        fn engine() -> Engine {
            let mut e = Engine::new();
            crate::password::password_rhai_register(&mut e);
            e
        }

        // Le collage binaire propage l'erreur par `format!("{e}").into()` : un
        // `ErrorRuntime` porteur du texte brut de `Error::PasswordSpec`
        // (affichage `#[error("{0}")]`), sans préfixe de crate ni source.
        fn assert_rejection(e: &Engine, src: &str, expected: &str) {
            let err = e
                .eval::<rhai::Dynamic>(src)
                .expect_err(&format!("`{src}` doit échouer"));
            let rhai::EvalAltResult::ErrorRuntime(msg, _) = err.as_ref() else {
                panic!("attendu l'erreur runtime du collage, obtenu {err:?}");
            };
            assert!(msg.is_string(), "le collage rend un texte brut, obtenu {msg:?}");
            assert_eq!(
                msg.to_string(),
                expected,
                "message exact citant la clé fautive, aucun repli toléré"
            );
        }

        // ── Scenario « une clé de spec non entière est une erreur de script » ──
        // Refuse : le repli silencieux sur `1` du `class_min` actuel — chaque
        // évaluation ci-dessous rendrait un Ok(password) au lieu du refus
        // `PWD-SPEC-003` citant sa propre clé.

        #[test]
        fn scenario_spec_key_chaine_est_une_erreur() {
            let e = engine();
            assert_rejection(
                &e,
                r#"gen_password(20, #{ lower: "5" })"#,
                "PWD-SPEC-003 spec key 'lower' must be an integer",
            );
        }

        #[test]
        fn scenario_spec_key_flottant_est_une_erreur() {
            let e = engine();
            assert_rejection(
                &e,
                "gen_password(20, #{ upper: 3.0 })",
                "PWD-SPEC-003 spec key 'upper' must be an integer",
            );
        }

        #[test]
        fn scenario_spec_key_booleen_est_une_erreur() {
            let e = engine();
            assert_rejection(
                &e,
                "gen_password(20, #{ digits: true })",
                "PWD-SPEC-003 spec key 'digits' must be an integer",
            );
        }

        #[test]
        fn scenario_spec_key_tableau_est_une_erreur() {
            let e = engine();
            assert_rejection(
                &e,
                "gen_password(20, #{ symbols: [] })",
                "PWD-SPEC-003 spec key 'symbols' must be an integer",
            );
        }

        #[test]
        fn scenario_spec_key_map_est_une_erreur() {
            let e = engine();
            assert_rejection(
                &e,
                "gen_password(20, #{ lower: #{ a: 1 } })",
                "PWD-SPEC-003 spec key 'lower' must be an integer",
            );
        }

        #[test]
        fn scenario_spec_key_unite_est_une_erreur() {
            let e = engine();
            assert_rejection(
                &e,
                "gen_password(20, #{ lower: () })",
                "PWD-SPEC-003 spec key 'lower' must be an integer",
            );
        }

        // Discrimination absente/présente verrouillée contre le rendu faillible :
        // la clé absente (`#{}`) reste le défaut `1` sans erreur, la clé présente
        // à `0` exclut la classe au lieu de retomber sur `1`.

        #[test]
        fn scenario_cle_absente_vaut_defaut_un() {
            let e = engine();
            let p: rhai::ImmutableString = e
                .eval("gen_password(40, #{})")
                .expect("la map vide équivaut à la surcharge unaire et doit réussir");
            assert_eq!(p.chars().count(), 40);
            assert!(
                count(&p, |c| c.is_ascii_lowercase()) >= 1,
                "clé absente : défaut `1`"
            );
            assert!(
                count(&p, |c| c.is_ascii_uppercase()) >= 1,
                "clé absente : défaut `1`"
            );
            assert!(count(&p, |c| c.is_ascii_digit()) >= 1, "clé absente : défaut `1`");
            assert!(
                count(&p, |c| SYMBOLS.contains(&c)) >= 1,
                "clé absente : défaut `1`"
            );
        }

        #[test]
        fn scenario_cle_presente_nulle_exclut_la_classe() {
            let e = engine();
            let p: rhai::ImmutableString = e
                .eval("gen_password(30, #{ lower: 0 })")
                .expect("une clé présente à `0` doit réussir, ce n'est pas une clé non entière");
            assert_eq!(
                count(&p, |c| c.is_ascii_lowercase()),
                0,
                "`lower: 0` exclut la classe, aucun repli sur le défaut `1`"
            );
            assert!(
                count(&p, |c| c.is_ascii_uppercase()) >= 1,
                "clé absente : défaut `1`"
            );
            assert!(count(&p, |c| c.is_ascii_digit()) >= 1, "clé absente : défaut `1`");
            assert!(
                count(&p, |c| SYMBOLS.contains(&c)) >= 1,
                "clé absente : défaut `1`"
            );
        }
    }
}
