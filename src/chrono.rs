//! Date/time helper for Rhai.
//!
//! Provides `DateTimeHandler` (`date_now` + `format`) via `chrono_rhai_register`.

use std::fmt::Write as _;

use chrono::{DateTime, Local};
#[cfg(feature = "rhai")] use rhai::{Engine, ImmutableString};

/// Local date-time handle. Create with [`DateTimeHandler::now`] then [`DateTimeHandler::format`].
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct DateTimeHandler {
    /// The wrapped local date-time value.
    pub date: DateTime<Local>,
}
impl DateTimeHandler {
    /// Current local date-time.
    #[must_use]
    pub fn now() -> Self {
        Self { date: Local::now() }
    }

    /// Formats the date with a `chrono` format string.
    ///
    /// The rendering is written through [`std::fmt::Write::write_fmt`]: a `chrono`
    /// `Display` failure (unknown specifier, trailing `%`) becomes a
    /// [`Error::Other`](crate::Error::Other) carrying `Invalid date format '<fmt>'`,
    /// never a panic. The output partially produced before the failing item is lost,
    /// with no fallback value.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Other`](crate::Error::Other) when `chrono` cannot render a
    /// format item of `fmt`.
    // `#[must_use]` conservé (src/chrono.sdd) ; la raison explicite évite
    // `clippy::double_must_use` sur le retour `Result` (harnais de /tooling.sdd).
    #[must_use = "the rendered string is the only output of formatting"]
    pub fn format(&self, fmt: &str) -> crate::Result<String> {
        let mut out = String::new();
        write!(out, "{}", self.date.format(fmt))
            .map(|()| out)
            .map_err(|_| crate::Error::Other(format!("Invalid date format '{fmt}'")))
    }

    /// Rhai binding of [`DateTimeHandler::format`]: an invalid format string becomes
    /// a catchable script error, never a host panic.
    ///
    /// # Errors
    ///
    /// Returns the [`Error::Other`](crate::Error::Other) of
    /// [`DateTimeHandler::format`] carried as a Rhai error (via
    /// [`rhai_err`](crate::rhai_err)), catchable by a script `try`.
    // signature imposée par l'API Rhai (vyvil-core.sdd)
    #[allow(clippy::needless_pass_by_value)]
    #[cfg(feature = "rhai")]
    pub fn rhai_format(&mut self, fmt: String) -> crate::RhaiRes<ImmutableString> {
        self.format(&fmt)
            .map(std::convert::Into::into)
            .map_err(crate::rhai_err)
    }
}

/// Registers the `DateTimeHandler` type and its date helpers on a Rhai `engine`.
#[cfg(feature = "rhai")]
pub fn chrono_rhai_register(engine: &mut Engine) {
    engine
        .register_type_with_name::<DateTimeHandler>("DateTimeHandler")
        .register_fn("date_now", DateTimeHandler::now)
        .register_fn("format", DateTimeHandler::rhai_format);
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    // Handler figé sur l'heure locale `2024-05-06 07:08:09`, sans nanoseconde.
    fn handler_fixed() -> DateTimeHandler {
        DateTimeHandler {
            date: Local.with_ymd_and_hms(2024, 5, 6, 7, 8, 9).unwrap(),
        }
    }

    // Scenario « spécificateur inconnu, erreur côté Rust » : `%Q` devient un
    // `Error::Other` sans panic, et aucune chaîne n'est retournée — ni vide, ni le motif brut.
    #[test]
    fn scenario_specificateur_inconnu_erreur_cote_rust() {
        match handler_fixed().format("%Q") {
            Ok(s) => panic!("aucune chaîne ne doit être retournée, ni vide ni le motif brut, obtenu {s:?}"),
            Err(crate::Error::Other(msg)) => assert!(
                msg.contains("Invalid date format '%Q'"),
                "le message doit porter `Invalid date format '%Q'`, obtenu {msg:?}"
            ),
            Err(other) => panic!("`Error::Other` attendu, obtenu {other:?}"),
        }
    }

    // Scenario « pourcentage final, erreur sans sortie de secours » : `%Y-%m-%d %` devient
    // un `Error::Other` sans panic, et la sortie partiellement produite (`2024-05-06 `) est
    // perdue — aucune chaîne débutant par ce préfixe n'est produite, et elle ne fuit pas
    // davantage dans l'erreur.
    #[test]
    fn scenario_forcentage_final_erreur_sans_sortie_de_secours() {
        match handler_fixed().format("%Y-%m-%d %") {
            Ok(s) => panic!(
                "aucune chaîne de repli ne doit être produite, en particulier aucune débutant \
                 par `2024-05-06 `, obtenu {s:?}"
            ),
            Err(crate::Error::Other(msg)) => {
                assert!(
                    msg.contains("Invalid date format '%Y-%m-%d %'"),
                    "le message doit porter `Invalid date format '%Y-%m-%d %'`, obtenu {msg:?}"
                );
                assert!(
                    !msg.contains("2024-05-06"),
                    "la sortie partielle doit être perdue, pas enfouie dans l'erreur, obtenu {msg:?}"
                );
            }
            Err(other) => panic!("`Error::Other` attendu, obtenu {other:?}"),
        }
    }

    #[cfg(feature = "rhai")]
    mod script {
        use super::*;

        // Engine fraîchement enregistré par `chrono_rhai_register`.
        fn engine() -> Engine {
            let mut e = Engine::new();
            crate::chrono::chrono_rhai_register(&mut e);
            e
        }

        // Scenario « un motif invalide dans un script devient une erreur rattrapable » :
        // `try` retourne `rattrape` ; sans `try`, l'évaluation échoue en erreur `rhai`
        // portant `Invalid date format '%Q'`, sans panic.
        #[test]
        fn scenario_motif_invalide_script_erreur_rattrapable() {
            let e = engine();
            let mut scope = rhai::Scope::new();
            scope.push("d", handler_fixed());
            // `try` est une instruction `rhai` de valeur `()` : le résultat est propagé
            // par la variable de sortie, le contrat observable reste « retourne rattrape ».
            let out = e
                .eval_with_scope::<rhai::ImmutableString>(
                    &mut scope,
                    r#"let r = ""; try { r = d.format("%Q"); } catch (e) { r = "rattrape"; } r"#,
                )
                .expect("le `try` du script doit rattraper l'erreur du motif invalide");
            assert_eq!(out.as_str(), "rattrape");
            let err = e
                .eval_with_scope::<rhai::ImmutableString>(&mut scope, r#"d.format("%Q")"#)
                .expect_err("sans `try`, l'évaluation doit échouer sans rendre de chaîne");
            let text = err.to_string();
            assert!(
                text.contains("Invalid date format '%Q'"),
                "l'erreur `rhai` doit porter `Invalid date format '%Q'`, obtenu {text}"
            );
        }
    }
}
