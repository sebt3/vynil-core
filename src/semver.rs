//! Semver parsing and bumping.
//!
//! `Semver` wraps `semver::Version` and preserves an optional leading `v`.
//! Rhai helpers (`semver_from`, `inc_major`/`inc_minor`/… + comparison ops) are registered by
//! `semver_rhai_register`.

use crate::{Error, Result};
#[cfg(feature = "rhai")] use crate::{RhaiRes, rhai_err};
#[cfg(feature = "rhai")] use rhai::Engine;
use semver::{Prerelease, Version};

/// Semver wrapper that remembers whether the original string had a leading `v`.
///
/// Implements `Display` so `to_string()` round-trips the `v` prefix. Ordering is delegated to
/// the inner `semver::Version`.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
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

    /// Bumps the major version, resetting minor, patch and prerelease.
    pub fn inc_major(&mut self) {
        self.version.major = self.version.major.saturating_add(1);
        self.version.minor = 0;
        self.version.patch = 0;
        self.version.pre = Prerelease::EMPTY;
    }

    /// Bumps the minor version, resetting patch and prerelease.
    pub fn inc_minor(&mut self) {
        self.version.minor = self.version.minor.saturating_add(1);
        self.version.patch = 0;
        self.version.pre = Prerelease::EMPTY;
    }

    /// Bumps the patch version, or only clears the prerelease when one is present.
    pub fn inc_patch(&mut self) {
        if self.version.pre.is_empty() {
            self.version.patch = self.version.patch.saturating_add(1);
        } else {
            self.version.pre = Prerelease::EMPTY;
        }
    }

    /// Bumps the `beta.N` prerelease counter.
    ///
    /// From a stable or non-beta-prerelease version this bumps the patch and sets `beta.1`;
    /// from `beta.N` it increments `N`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Semver`] if the resulting prerelease string is invalid, and
    /// [`Error::Other`] if the existing `beta.` suffix is not a number.
    pub fn inc_beta(&mut self) -> Result<()> {
        if self.version.pre.is_empty() || !self.version.pre.starts_with("beta.") {
            self.version.patch = self.version.patch.saturating_add(1);
            self.version.pre = Prerelease::new("beta.1").map_err(Error::Semver)?;
        } else {
            let str = self.version.pre.strip_prefix("beta.").unwrap_or_default();
            let beta = str
                .parse::<u32>()
                .map_err(|e| Error::Other(format!("invalid beta counter '{str}': {e}")))?;
            self.version.pre =
                Prerelease::new(&format!("beta.{}", beta.saturating_add(1))).map_err(Error::Semver)?;
        }
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

    /// Bumps the `alpha.N` prerelease counter.
    ///
    /// From a stable or non-alpha-prerelease version this bumps the patch and sets `alpha.1`;
    /// from `alpha.N` it increments `N`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Semver`] if the resulting prerelease string is invalid, and
    /// [`Error::Other`] if the existing `alpha.` suffix is not a number.
    pub fn inc_alpha(&mut self) -> Result<()> {
        if self.version.pre.is_empty() || !self.version.pre.starts_with("alpha.") {
            self.version.patch = self.version.patch.saturating_add(1);
            self.version.pre = Prerelease::new("alpha.1").map_err(Error::Semver)?;
        } else {
            let str = self.version.pre.strip_prefix("alpha.").unwrap_or_default();
            let alpha = str
                .parse::<u32>()
                .map_err(|e| Error::Other(format!("invalid alpha counter '{str}': {e}")))?;
            self.version.pre =
                Prerelease::new(&format!("alpha.{}", alpha.saturating_add(1))).map_err(Error::Semver)?;
        }
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
}
