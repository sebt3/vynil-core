//! Hash helpers: `crc32_hash` (always), `bcrypt_hash` / `Argon` (feature `crypto`).
//!
//! Rhai bindings are registered by `hashes_rhai_register` / `crypto_hashes_rhai_register`.

#[cfg(feature = "crypto")] use crate::{Error, Result};
#[cfg(all(feature = "rhai", feature = "crypto"))]
use crate::{RhaiRes, rhai_err};
#[cfg(feature = "crypto")]
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, SaltString, rand_core::OsRng},
};
#[cfg(feature = "crypto")] use bcrypt::{DEFAULT_COST, non_truncating_hash};
#[cfg(feature = "rhai")] use rhai::{Engine, ImmutableString};

/// Argon2 hasher with a random per-instance salt.
///
/// Feature `crypto` only.
#[cfg(feature = "crypto")]
#[derive(Clone, Debug)]
pub struct Argon {
    salt: SaltString,
    argon: Argon2<'static>,
}
#[cfg(feature = "crypto")]
impl Default for Argon {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "crypto")]
impl Argon {
    /// Creates a hasher with a freshly generated random salt.
    #[must_use]
    pub fn new() -> Self {
        Self {
            salt: SaltString::generate(&mut OsRng),
            argon: Argon2::default(),
        }
    }

    /// Hashes `password` with this instance's salt.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Argon2hash`] when Argon2 hashing fails.
    pub fn hash(&self, password: String) -> Result<String> {
        Ok(self
            .argon
            .hash_password(&password.into_bytes(), &self.salt)
            .map_err(Error::Argon2hash)?
            .to_string())
    }

    /// Rhai binding of [`Argon::hash`].
    ///
    /// # Errors
    ///
    /// Returns a Rhai error wrapping [`Error::Argon2hash`] when Argon2 hashing fails.
    #[cfg(feature = "rhai")]
    pub fn rhai_hash(&mut self, password: String) -> RhaiRes<String> {
        self.hash(password).map_err(rhai_err)
    }
}

/// Hash `password` with bcrypt (cost [`bcrypt::DEFAULT_COST`]). Feature `crypto` only.
///
/// # Errors
///
/// Returns [`Error::BcryptError`] when bcrypt hashing fails.
#[cfg(feature = "crypto")]
pub fn bcrypt_hash(password: String) -> Result<String> {
    non_truncating_hash(password, DEFAULT_COST).map_err(Error::BcryptError)
}
/// CRC32 (IEEE) hash of `text`.
#[must_use]
pub fn crc32_hash(text: String) -> u32 {
    crc32fast::hash(&text.into_bytes())
}

/// Registers the always-available hash helpers (`crc32_hash`) on a Rhai `engine`.
#[cfg(feature = "rhai")]
pub fn hashes_rhai_register(engine: &mut Engine) {
    engine.register_fn("crc32_hash", |s: ImmutableString| {
        crate::hashes::crc32_hash(s.to_string())
    });
}

/// Registers the `crypto`-feature hash helpers (`bcrypt_hash`, `Argon`) on a Rhai `engine`.
#[cfg(all(feature = "rhai", feature = "crypto"))]
pub fn crypto_hashes_rhai_register(engine: &mut Engine) {
    engine
        .register_fn("bcrypt_hash", |s: ImmutableString| {
            crate::hashes::bcrypt_hash(s.to_string()).map_err(rhai_err)
        })
        .register_type_with_name::<Argon>("Argon")
        .register_fn("new_argon", Argon::new)
        .register_fn("hash", Argon::rhai_hash);
}

#[cfg(all(test, feature = "crypto"))]
mod tests {
    use super::bcrypt_hash;
    use crate::Error;

    /// Scenario « bcrypt refuse cent octets », clause 100 octets : refus en
    /// `Error::BcryptError` portant la variante interne `BcryptError::Truncation`, avec la
    /// chaîne exacte visible par le consommateur. Meurt sous la voie `bcrypt::hash`
    /// actuelle : le 100 octets rend un `Ok` silencieux.
    #[test]
    fn bcrypt_refuses_100_bytes_with_truncation() {
        let err = bcrypt_hash("x".repeat(100)).expect_err("100 bytes must be refused");
        assert!(
            matches!(err, Error::BcryptError(bcrypt::BcryptError::Truncation(_))),
            "expected Error::BcryptError(BcryptError::Truncation), got {err:?}"
        );
        assert_eq!(
            err.to_string(),
            "Bcrypt hash error Expected 72 bytes or fewer; found 101 bytes"
        );
    }

    /// Scenario « bcrypt refuse cent octets », verrou de borne utile `71` : `71` octets
    /// est encore accepté (`Ok` de 60 caractères en `$2b$12$`), `72` octets est déjà
    /// refusé en `BcryptError::Truncation(73)` — l'octet de fin compte. Une implémentation
    /// qui accepterait `72` rougit ; une qui refuserait `71` rougit aussi.
    #[test]
    fn bcrypt_boundary_accepts_71_rejects_72() {
        let hash = bcrypt_hash("x".repeat(71)).expect("71 bytes must still be accepted");
        assert_eq!(hash.len(), 60);
        assert!(hash.starts_with("$2b$12$"));

        let err = bcrypt_hash("x".repeat(72)).expect_err("72 bytes must be refused");
        assert!(
            matches!(err, Error::BcryptError(bcrypt::BcryptError::Truncation(73))),
            "expected Error::BcryptError(BcryptError::Truncation(73)), got {err:?}"
        );
    }
}
