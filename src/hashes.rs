//! Hash helpers: `crc32_hash` (always), `bcrypt_hash` / `Argon` (feature `crypto`).
//!
//! Rhai bindings are registered by `hashes_rhai_register` / `crypto_hashes_rhai_register`.

#[cfg(feature = "crypto")] use crate::{Error, Result};
#[cfg(all(feature = "rhai", feature = "crypto"))]
use crate::{RhaiRes, rhai_err};
#[cfg(feature = "crypto")]
use argon2::{
    Argon2,
    password_hash::{
        PasswordHasher, SaltString,
        rand_core::{OsRng, RngCore},
    },
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
impl Argon {
    /// Creates a hasher with a freshly generated random 16-byte salt.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Other`] carrying `Entropy failure: ...` when the system entropy
    /// source fails (`OsRng::try_fill_bytes`) or when the salt bytes fail to encode as a
    /// [`SaltString`] (in practice unreachable for 16 bytes). Never panics.
    pub fn new() -> Result<Self> {
        let mut salt_bytes = [0_u8; 16];
        OsRng
            .try_fill_bytes(&mut salt_bytes)
            .map_err(|e| Error::Other(format!("Entropy failure: {e}")))?;
        let salt =
            SaltString::encode_b64(&salt_bytes).map_err(|e| Error::Other(format!("Entropy failure: {e}")))?;
        Ok(Self {
            salt,
            argon: Argon2::default(),
        })
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
        .register_fn("new_argon", || -> RhaiRes<Argon> {
            Argon::new().map_err(rhai_err)
        })
        .register_fn("hash", Argon::rhai_hash);
}

#[cfg(all(test, feature = "crypto"))]
mod tests {
    use super::{Argon, bcrypt_hash};
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

    /// Scenario « deux constructions d'Argon tirent deux sels », verrou « sel frais par
    /// instance » que la tâche de construction faillible touche de près : deux instances
    /// `Argon::new()` rendent deux hashes différents du même mot de passe. Rougit si le sel
    /// cesse d'être tiré à la construction (constante, partagé, ou figé par un `Default`).
    #[test]
    fn argon_new_draws_fresh_salt_per_instance() {
        let first = Argon::new()
            .expect("Argon::new must succeed on a system with working entropy")
            .hash("p".to_string())
            .expect("hashing with a valid internal salt must succeed");
        let second = Argon::new()
            .expect("second construction must succeed")
            .hash("p".to_string())
            .expect("hashing with a valid internal salt must succeed");
        assert_ne!(first, second, "two Argon instances must not share one salt");
    }

    /// Scenario « l'argon par défaut borne les champs PHC attendus » : l'encodage du sel par
    /// `SaltString::encode_b64` (16 octets remplis par `try_fill_bytes`) ne doit rien casser à
    /// la parité avec l'ancien `SaltString::generate` — préfixe PHC exact
    /// `$argon2id$v=19$m=19456,t=2,p=1$`, segment de sel de 22 caractères (16 octets en
    /// base64 sans padding), segment de hash de 43 caractères (32 octets).
    #[test]
    fn argon_phc_shape_pins_prefix_and_segment_lengths() {
        let hash = Argon::new()
            .expect("Argon::new must succeed on a system with working entropy")
            .hash("p".to_string())
            .expect("hashing with a valid internal salt must succeed");
        assert!(
            hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
            "expected the PHC prefix, got {hash}"
        );
        let segments: Vec<&str> = hash.split('$').collect();
        assert_eq!(segments.len(), 6, "expected 6 PHC segments, got {hash}");
        assert_eq!(
            segments[4].len(),
            22,
            "salt segment should be 22 chars: {}",
            segments[4]
        );
        assert_eq!(
            segments[5].len(),
            43,
            "hash segment should be 43 chars: {}",
            segments[5]
        );
    }
}

/// Scenarios du collage script `new_argon`, sous `crypto` + `rhai` : `Argon::new` rendu
/// faillible, l'enregistreur doit porter l'erreur par `crate::rhai_err` (voie déjà prise par
/// `bcrypt_hash` dans `crypto_hashes_rhai_register`) et le script doit recevoir un objet
/// `Argon` utilisable.
#[cfg(all(test, feature = "crypto", feature = "rhai"))]
mod rhai_tests {
    use crate::hashes::crypto_hashes_rhai_register;
    use rhai::Engine;

    #[test]
    fn new_argon_script_constructor_returns_usable_argon() {
        let mut engine = Engine::new();
        crypto_hashes_rhai_register(&mut engine);
        let type_name: String = engine
            .eval("let a = new_argon(); a.type_of()")
            .expect("type_of of a new_argon() object must succeed");
        assert_eq!(type_name, "Argon");
        let hash: String = engine
            .eval("let a = new_argon(); a.hash(\"p\")")
            .expect("new_argon().hash(\"p\") must succeed");
        assert!(
            hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
            "expected the PHC prefix, got {hash}"
        );
    }
}
