#[cfg(feature = "crypto")] use crate::{Error, Result};
#[cfg(all(feature = "rhai", feature = "crypto"))]
use crate::{RhaiRes, rhai_err};
#[cfg(feature = "crypto")]
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, SaltString, rand_core::OsRng},
};
#[cfg(feature = "crypto")] use bcrypt::{DEFAULT_COST, hash};
#[cfg(feature = "rhai")] use rhai::{Engine, ImmutableString};

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
    #[must_use]
    pub fn new() -> Self {
        Self {
            salt: SaltString::generate(&mut OsRng),
            argon: Argon2::default(),
        }
    }

    pub fn hash(&self, password: String) -> Result<String> {
        Ok(self
            .argon
            .hash_password(password.as_bytes(), &self.salt)
            .map_err(Error::Argon2hash)?
            .to_string())
    }

    #[cfg(feature = "rhai")]
    pub fn rhai_hash(&mut self, password: String) -> RhaiRes<String> {
        self.hash(password).map_err(rhai_err)
    }
}

#[cfg(feature = "crypto")]
pub fn bcrypt_hash(password: String) -> Result<String> {
    hash(&password, DEFAULT_COST).map_err(Error::BcryptError)
}
pub fn crc32_hash(text: String) -> u32 {
    crc32fast::hash(text.as_bytes())
}

#[cfg(feature = "rhai")]
pub fn hashes_rhai_register(engine: &mut Engine) {
    engine.register_fn("crc32_hash", |s: ImmutableString| {
        crate::hashes::crc32_hash(s.to_string())
    });
}

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
