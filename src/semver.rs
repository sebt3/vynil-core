//! Semver parsing and bumping.
//!
//! `Semver` wraps `semver::Version` and preserves an optional leading `v`.
//! Rhai helpers (`semver_from`, `inc_major`/`inc_minor`/… + comparison ops) are registered by
//! `semver_rhai_register`.

use crate::{Error, Result};
#[cfg(feature = "rhai")] use crate::{RhaiRes, rhai_err};
#[cfg(feature = "rhai")] use rhai::Engine;
use semver::{BuildMetadata, Prerelease, Version};

/// Semver wrapper that remembers whether the original string had a leading `v`.
///
/// Implements `Display` so `to_string()` round-trips the `v` prefix. Equality, ordering and
/// hashing delegate to the inner `semver::Version`; the `v` flag is cosmetic (display only).
#[derive(Clone, Debug)]
pub struct Semver {
    /// The parsed semantic version.
    pub version: Version,
    /// Whether the original string had a leading `v`.
    pub use_v: bool,
}

impl Semver {
    /// Parses a semver string, optionally prefixed with `v`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Semver`] if the string is not a valid semantic version.
    pub fn parse(str: &str) -> Result<Self> {
        let use_v = str.starts_with('v');
        let version = if use_v {
            let mut chars = str.chars();
            chars.next();
            Version::parse(chars.as_str()).map_err(Error::Semver)?
        } else {
            Version::parse(str).map_err(Error::Semver)?
        };
        Ok(Self { version, use_v })
    }

    /// Parses a semver string, returning `None` instead of an error on invalid input.
    #[must_use]
    pub fn opt_parse(str: &str) -> Option<Self> {
        Self::parse(str).ok()
    }

    /// [`Semver::parse`] variant for Rhai, mapping errors to [`rhai::EvalAltResult`].
    ///
    /// # Errors
    ///
    /// Returns the stringified [`Error::Semver`] if the string is not a valid semantic version.
    #[cfg(feature = "rhai")]
    pub fn rhai_parse(str: &str) -> RhaiRes<Self> {
        Self::parse(str).map_err(rhai_err)
    }

    /// Bumps the major version, resetting minor and patch, clearing prerelease and build metadata.
    pub fn inc_major(&mut self) {
        self.version.major = self.version.major.saturating_add(1);
        self.version.minor = 0;
        self.version.patch = 0;
        self.version.pre = Prerelease::EMPTY;
        self.version.build = BuildMetadata::EMPTY;
    }

    /// Bumps the minor version, resetting patch and clearing prerelease and build metadata.
    pub fn inc_minor(&mut self) {
        self.version.minor = self.version.minor.saturating_add(1);
        self.version.patch = 0;
        self.version.pre = Prerelease::EMPTY;
        self.version.build = BuildMetadata::EMPTY;
    }

    /// Bumps the patch version and clears build metadata, or only clears the prerelease and
    /// build metadata when one is present.
    pub fn inc_patch(&mut self) {
        if self.version.pre.is_empty() {
            self.version.patch = self.version.patch.saturating_add(1);
        } else {
            self.version.pre = Prerelease::EMPTY;
        }
        self.version.build = BuildMetadata::EMPTY;
    }

    /// Bumps the `beta.N` prerelease counter and clears build metadata on success.
    ///
    /// From a stable or non-beta-prerelease version this bumps the patch and sets `beta.1`;
    /// from `beta.N` it increments `N`. On a counter error the version is left untouched,
    /// build metadata included.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Semver`] if the resulting prerelease string is invalid, and
    /// [`Error::ParseInt`] if the existing `beta.` suffix is not a number.
    pub fn inc_beta(&mut self) -> Result<()> {
        if self.version.pre.is_empty() || !self.version.pre.starts_with("beta.") {
            self.version.patch = self.version.patch.saturating_add(1);
            self.version.pre = Prerelease::new("beta.1").map_err(Error::Semver)?;
        } else {
            let str = self.version.pre.strip_prefix("beta.").unwrap_or_default();
            let beta = str.parse::<u32>()?;
            self.version.pre =
                Prerelease::new(&format!("beta.{}", beta.saturating_add(1))).map_err(Error::Semver)?;
        }
        self.version.build = BuildMetadata::EMPTY;
        Ok(())
    }

    /// [`Semver::inc_beta`] variant for Rhai, mapping errors to [`rhai::EvalAltResult`].
    ///
    /// # Errors
    ///
    /// Returns the stringified error of [`Semver::inc_beta`].
    #[cfg(feature = "rhai")]
    pub fn rhai_inc_beta(&mut self) -> RhaiRes<()> {
        self.inc_beta().map_err(rhai_err)
    }

    /// Bumps the `alpha.N` prerelease counter and clears build metadata on success.
    ///
    /// From a stable or non-alpha-prerelease version this bumps the patch and sets `alpha.1`;
    /// from `alpha.N` it increments `N`. On a counter error the version is left untouched,
    /// build metadata included.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Semver`] if the resulting prerelease string is invalid, and
    /// [`Error::ParseInt`] if the existing `alpha.` suffix is not a number.
    pub fn inc_alpha(&mut self) -> Result<()> {
        if self.version.pre.is_empty() || !self.version.pre.starts_with("alpha.") {
            self.version.patch = self.version.patch.saturating_add(1);
            self.version.pre = Prerelease::new("alpha.1").map_err(Error::Semver)?;
        } else {
            let str = self.version.pre.strip_prefix("alpha.").unwrap_or_default();
            let alpha = str.parse::<u32>()?;
            self.version.pre =
                Prerelease::new(&format!("alpha.{}", alpha.saturating_add(1))).map_err(Error::Semver)?;
        }
        self.version.build = BuildMetadata::EMPTY;
        Ok(())
    }

    /// [`Semver::inc_alpha`] variant for Rhai, mapping errors to [`rhai::EvalAltResult`].
    ///
    /// # Errors
    ///
    /// Returns the stringified error of [`Semver::inc_alpha`].
    #[cfg(feature = "rhai")]
    pub fn rhai_inc_alpha(&mut self) -> RhaiRes<()> {
        self.inc_alpha().map_err(rhai_err)
    }
}

impl PartialEq for Semver {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
    }
}

impl Eq for Semver {}

impl PartialOrd for Semver {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Semver {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.version.cmp(&other.version)
    }
}

impl std::hash::Hash for Semver {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.version.hash(state);
    }
}

impl std::fmt::Display for Semver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        if self.use_v {
            write!(formatter, "v{}", self.version)
        } else {
            self.version.fmt(formatter)
        }
    }
}

/// Registers the `Semver` type and its helpers (`semver_from`, `inc_major`, `inc_minor`,
/// `inc_patch`, `inc_beta`, `inc_alpha`, comparison operators, `to_string`) on `engine`.
#[cfg(feature = "rhai")]
pub fn semver_rhai_register(engine: &mut Engine) {
    engine
        .register_type_with_name::<Semver>("Semver")
        .register_fn("semver_from", Semver::rhai_parse)
        .register_fn("inc_major", Semver::inc_major)
        .register_fn("inc_minor", Semver::inc_minor)
        .register_fn("inc_patch", Semver::inc_patch)
        .register_fn("inc_beta", Semver::rhai_inc_beta)
        .register_fn("inc_alpha", Semver::rhai_inc_alpha)
        .register_fn("==", |a: Semver, b: Semver| a == b)
        .register_fn("!=", |a: Semver, b: Semver| a != b)
        .register_fn("<", |a: Semver, b: Semver| a < b)
        .register_fn(">", |a: Semver, b: Semver| a > b)
        .register_fn("<=", |a: Semver, b: Semver| a <= b)
        .register_fn(">=", |a: Semver, b: Semver| a >= b)
        .register_fn("to_string", |s: &mut Semver| s.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_without_v_prefix() {
        let sv = Semver::parse("1.2.3").unwrap();
        assert_eq!(sv.version.major, 1);
        assert_eq!(sv.version.minor, 2);
        assert_eq!(sv.version.patch, 3);
        assert!(!sv.use_v);
    }

    #[test]
    fn test_parse_with_v_prefix() {
        let sv = Semver::parse("v1.2.3").unwrap();
        assert_eq!(sv.version.major, 1);
        assert_eq!(sv.version.minor, 2);
        assert_eq!(sv.version.patch, 3);
        assert!(sv.use_v);
    }

    #[test]
    fn test_to_string_preserves_v_prefix() {
        let sv = Semver::parse("v1.2.3").unwrap();
        assert_eq!(Semver::to_string(&sv), "v1.2.3");
    }

    #[test]
    fn test_to_string_without_v_prefix() {
        let sv = Semver::parse("1.2.3").unwrap();
        assert_eq!(Semver::to_string(&sv), "1.2.3");
    }

    #[test]
    fn test_comparison_lt() {
        let v1 = Semver::parse("1.2.3").unwrap();
        let v2 = Semver::parse("1.2.4").unwrap();
        assert!(v1 < v2);
        assert!(v2 > v1);
    }

    #[test]
    fn test_comparison_eq() {
        let v1 = Semver::parse("1.2.3").unwrap();
        let v2 = Semver::parse("1.2.3").unwrap();
        assert_eq!(v1, v2);
        assert!(v1 <= v2);
        assert!(v1 >= v2);
    }

    #[test]
    fn test_comparison_major_beats_minor() {
        let v1 = Semver::parse("2.0.0").unwrap();
        let v2 = Semver::parse("1.99.99").unwrap();
        assert!(v1 > v2);
    }

    #[test]
    fn test_comparison_v_prefix_transparent() {
        let v1 = Semver::parse("v1.2.3").unwrap();
        let v2 = Semver::parse("2.0.0").unwrap();
        assert!(v1 < v2);
    }

    #[test]
    fn test_inc_major_resets_minor_and_patch() {
        let mut sv = Semver::parse("1.2.3").unwrap();
        sv.inc_major();
        assert_eq!(sv.version.major, 2);
        assert_eq!(sv.version.minor, 0);
        assert_eq!(sv.version.patch, 0);
        assert!(sv.version.pre.is_empty());
    }

    #[test]
    fn test_inc_minor_resets_patch() {
        let mut sv = Semver::parse("1.2.3").unwrap();
        sv.inc_minor();
        assert_eq!(sv.version.minor, 3);
        assert_eq!(sv.version.patch, 0);
        assert!(sv.version.pre.is_empty());
    }

    #[test]
    fn test_inc_patch_stable() {
        let mut sv = Semver::parse("1.2.3").unwrap();
        sv.inc_patch();
        assert_eq!(sv.version.patch, 4);
    }

    #[test]
    fn test_inc_patch_clears_prerelease_without_bumping_patch() {
        let mut sv = Semver::parse("1.2.3-beta.1").unwrap();
        sv.inc_patch();
        assert_eq!(sv.version.patch, 3);
        assert!(sv.version.pre.is_empty());
    }

    #[test]
    fn test_inc_beta_from_stable_bumps_patch() {
        let mut sv = Semver::parse("1.2.3").unwrap();
        sv.inc_beta().unwrap();
        assert_eq!(sv.version.patch, 4);
        assert_eq!(sv.version.pre.as_str(), "beta.1");
    }

    #[test]
    fn test_inc_beta_from_existing_beta_increments_counter() {
        let mut sv = Semver::parse("1.2.4-beta.1").unwrap();
        sv.inc_beta().unwrap();
        assert_eq!(sv.version.patch, 4);
        assert_eq!(sv.version.pre.as_str(), "beta.2");
    }

    #[test]
    fn test_inc_alpha_from_stable_bumps_patch() {
        let mut sv = Semver::parse("1.2.3").unwrap();
        sv.inc_alpha().unwrap();
        assert_eq!(sv.version.patch, 4);
        assert_eq!(sv.version.pre.as_str(), "alpha.1");
    }

    #[test]
    fn test_inc_alpha_from_existing_alpha_increments_counter() {
        let mut sv = Semver::parse("1.2.4-alpha.2").unwrap();
        sv.inc_alpha().unwrap();
        assert_eq!(sv.version.pre.as_str(), "alpha.3");
    }

    #[test]
    fn test_parse_invalid_returns_error() {
        assert!(Semver::parse("not-semver").is_err());
        assert!(Semver::parse("1.2").is_err());
        assert!(Semver::parse("").is_err());
    }

    #[test]
    fn test_opt_parse_invalid_returns_none() {
        assert!(Semver::opt_parse("not-semver").is_none());
    }

    #[test]
    fn test_opt_parse_valid_returns_some() {
        assert!(Semver::opt_parse("1.0.0").is_some());
    }

    #[test]
    fn test_prerelease_is_less_than_stable() {
        let pre = Semver::parse("1.2.3-alpha.1").unwrap();
        let stable = Semver::parse("1.2.3").unwrap();
        assert!(pre < stable);
    }

    #[test]
    fn test_inc_major_clears_build() {
        let mut sv = Semver::parse("1.2.3-alpha.1+sha.5114f85").unwrap();
        sv.inc_major();
        assert_eq!(Semver::to_string(&sv), "2.0.0");
    }

    #[test]
    fn test_inc_minor_clears_build() {
        let mut sv = Semver::parse("1.2.3-alpha.1+sha.5114f85").unwrap();
        sv.inc_minor();
        assert_eq!(Semver::to_string(&sv), "1.3.0");
    }

    #[test]
    fn test_inc_patch_clears_build() {
        let mut sv = Semver::parse("1.2.3-alpha.1+sha.5114f85").unwrap();
        sv.inc_patch();
        assert_eq!(Semver::to_string(&sv), "1.2.3");
    }

    #[test]
    fn test_inc_beta_from_prerelease_clears_build() {
        let mut sv = Semver::parse("1.2.3-alpha.1+sha.5114f85").unwrap();
        sv.inc_beta().unwrap();
        assert_eq!(Semver::to_string(&sv), "1.2.4-beta.1");
    }

    #[test]
    fn test_inc_beta_from_stable_clears_build() {
        let mut sv = Semver::parse("1.2.3+sha.5114f85").unwrap();
        sv.inc_beta().unwrap();
        assert_eq!(Semver::to_string(&sv), "1.2.4-beta.1");
    }

    #[test]
    fn test_inc_alpha_counter_branch_clears_build() {
        let mut sv = Semver::parse("1.2.3-alpha.1+sha.5114f85").unwrap();
        sv.inc_alpha().unwrap();
        assert_eq!(Semver::to_string(&sv), "1.2.3-alpha.2");
    }

    #[test]
    fn test_inc_beta_rejected_counter_keeps_build() {
        let mut sv = Semver::parse("1.2.3-beta.rc+sha.5114f85").unwrap();
        assert!(sv.inc_beta().is_err());
        assert_eq!(sv.version.major, 1);
        assert_eq!(sv.version.minor, 2);
        assert_eq!(sv.version.patch, 3);
        assert_eq!(sv.version.pre.as_str(), "beta.rc");
        assert_eq!(sv.version.build.as_str(), "sha.5114f85");
        assert!(!sv.use_v);
    }

    // ── Résidu de la conversion ParseInt (Scenario « un compteur non numérique erreur et
    // laisse la version intacte ») : l'ancre ci-dessus ne verrouillait que l'échec et les
    // champs ; l'affichage du rejet est verrouillé ici. On verrouille le texte porté par la
    // crate — le préfixe `ParseIntError `, affichage de @crate::Error::ParseInt (table
    // Display de ./lib.rs) — et non le suffixe : il vient de @std::num::ParseIntError et
    // appartient à la dépendance, comme les textes openssl (key.sdd) ou rhai. ──
    #[test]
    fn test_inc_beta_rejected_counter_displays_parse_int_error() {
        let mut sv = Semver::parse("1.2.3-beta.rc").unwrap();
        let err = sv.inc_beta().unwrap_err();
        assert!(
            matches!(err, Error::ParseInt(_)),
            "la variante typée est la seule voie, ni Other ni Semver : {err}"
        );
        assert!(
            err.to_string().starts_with("ParseIntError "),
            "l'affichage contracté est `ParseIntError ` + le texte de ParseIntError : {err}"
        );
    }

    #[test]
    fn test_inc_alpha_rejected_counter_displays_parse_int_error() {
        let mut sv = Semver::parse("1.2.3-alpha.rc").unwrap();
        let err = sv.inc_alpha().unwrap_err();
        assert!(
            matches!(err, Error::ParseInt(_)),
            "la variante typée est la seule voie, ni Other ni Semver : {err}"
        );
        assert!(
            err.to_string().starts_with("ParseIntError "),
            "l'affichage contracté est `ParseIntError ` + le texte de ParseIntError : {err}"
        );
    }

    // ── Face script de la même conversion (la tâche prévoit cette face dans CE module ;
    // l'inventaire met les autres verrous script côté ./engine.rs). Voix retenue : @rhai::Engine
    // local enregistré par semver_rhai_register, comme le Scenario « l'objet Rhai connaît ses
    // méthodes » de la spec. Seul le texte porté est contractuel : on verrouille la mention
    // `ParseIntError ` dans l'erreur remontée, jamais le préfixe d'affichage complet de rhai
    // (enveloppe interne), ni le suffixe de @std::num::ParseIntError. ──
    #[cfg(feature = "rhai")]
    #[test]
    fn test_script_inc_beta_rejected_counter_carries_parse_int_error() {
        let mut engine = Engine::new();
        semver_rhai_register(&mut engine);
        let err = engine
            .eval::<()>(r#"let w = semver_from("1.2.3-beta.rc"); w.inc_beta();"#)
            .expect_err("un compteur non numérique doit faire échouer le script");
        assert!(
            err.to_string().contains("ParseIntError "),
            "l'erreur de script doit porter `ParseIntError ` : {err}"
        );
    }

    // ── Scenario « l'ordre suit semver, le v n'y compte pas » (l'égalité et l'ordre à
    // version égale, lacune nommé par l'inventaire des Tasks). Les trois verrous Rust verrouillent
    // le `Must` « use_v n'entre ni dans l'égalité, ni dans l'ordre, ni dans le Hash » :
    // à version égale et drapeaux opposés, @std::cmp::PartialEq vrai, `1.2.3 < v1.2.3` et
    // `v1.2.3 > 1.2.3` faux (les deux sens nommés par le Then), et même @std::hash::Hash.
    // Le premier Then du Scenario (`2.0.0` domine `1.99.99`, `1.2.3-alpha.1` sous `1.2.3`)
    // est déjà tenu par `test_comparison_major_beats_minor` et
    // `test_prerelease_is_less_than_stable` : non rejoué. État d'avant mesuré : le derive
    // sur le tuple (version, use_v) rend `1.2.3 != v1.2.3` et `1.2.3 < v1.2.3` (false < true). ──
    #[test]
    fn test_v_flag_not_in_equality() {
        let plain = Semver::parse("1.2.3").unwrap();
        let prefixed = Semver::parse("v1.2.3").unwrap();
        assert_eq!(
            plain, prefixed,
            "à version égale et drapeaux opposés, l'égalité ne doit rien au `v`"
        );
    }

    #[test]
    fn test_v_flag_not_in_ordering() {
        let plain = Semver::parse("1.2.3").unwrap();
        let prefixed = Semver::parse("v1.2.3").unwrap();
        // Verrouillés par variable intermédiaire : le harnais pedantic (`nonminimal_bool`)
        // refuse `!(a < b)` sous une autre forme, et ce sont bien `<` et `>` que le Then nomme.
        let lt = plain < prefixed;
        assert!(
            !lt,
            "`1.2.3 < v1.2.3` doit être faux : le drapeau n'entre pas dans l'ordre"
        );
        let gt = prefixed > plain;
        assert!(
            !gt,
            "`v1.2.3 > 1.2.3` doit être faux : le drapeau n'entre pas dans l'ordre"
        );
    }

    #[test]
    fn test_v_flag_not_in_hash() {
        use std::{
            collections::hash_map::DefaultHasher,
            hash::{Hash, Hasher},
        };
        let hash_of = |s: &Semver| {
            let mut hasher = DefaultHasher::new();
            s.hash(&mut hasher);
            hasher.finish()
        };
        let plain = Semver::parse("1.2.3").unwrap();
        let prefixed = Semver::parse("v1.2.3").unwrap();
        assert_eq!(
            hash_of(&plain),
            hash_of(&prefixed),
            "deux valeurs égales doivent avoir le même hash"
        );
    }

    // ── Face script du même Scenario : « ces verdicts valent aussi en script, `==` et `<`
    // empruntant les fermetures enregistrées ». Voix retenue comme chez les trois verrous
    // voisins : @rhai::Engine local enregistré par semver_rhai_register. ──
    #[cfg(feature = "rhai")]
    #[test]
    fn test_script_v_flag_transparent_to_eq_and_lt() {
        let mut engine = Engine::new();
        semver_rhai_register(&mut engine);
        assert!(
            engine
                .eval::<bool>(r#"semver_from("1.2.3") == semver_from("v1.2.3")"#)
                .unwrap(),
            "le `==` enregistré doit rendre vrai à version égale et drapeaux opposés"
        );
        assert!(
            !engine
                .eval::<bool>(r#"semver_from("1.2.3") < semver_from("v1.2.3")"#)
                .unwrap(),
            "le `<` enregistré doit rendre faux à version égale et drapeaux opposés"
        );
    }
}
