//! Private key generation (RSA / ed25519 via OpenSSL).
//!
//! Feature `crypto` only. Rhai bindings are registered by `key_rhai_register`.

use crate::{Error, Result};
use openssl::{pkey::PKey, rsa::Rsa};
#[cfg(feature = "rhai")] use rhai::Engine;

/// Default RSA key size when `bits` is not specified.
pub const DEFAULT_RSA_BITS: u32 = 4096;

/// Generate a PEM-encoded private key. `algo` is `rsa` or `ed25519` (case-insensitive).
/// `bits` is only used for RSA. Returns `Error::UnsupportedKeyAlgorithm` for unknown algos.
///
/// # Errors
///
/// Returns [`Error::UnsupportedKeyAlgorithm`] for an unknown `algo`, [`Error::OpenSSL`] when
/// key generation or PEM encoding fails, [`Error::UTF8`] if the PEM is not valid UTF-8.
pub fn gen_private_key(algo: &str, bits: u32) -> Result<String> {
    let pem = match algo.to_ascii_lowercase().as_str() {
        "ed25519" => PKey::generate_ed25519()?.private_key_to_pem_pkcs8()?,
        "rsa" => PKey::from_rsa(Rsa::generate(bits)?)?.private_key_to_pem_pkcs8()?,
        _ => return Err(Error::UnsupportedKeyAlgorithm(algo.to_string())),
    };
    String::from_utf8(pem).map_err(Error::UTF8)
}

/// Registers the `gen_private_key` helpers on a Rhai `engine`.
#[cfg(feature = "rhai")]
pub fn key_rhai_register(engine: &mut Engine) {
    engine
        .register_fn("gen_private_key", |algo: &str| -> crate::RhaiRes<String> {
            gen_private_key(algo, DEFAULT_RSA_BITS).map_err(|e| format!("{e}").into())
        })
        .register_fn(
            "gen_private_key",
            |algo: &str, bits: i64| -> crate::RhaiRes<String> {
                let Ok(bits) = u32::try_from(bits) else {
                    return Err(format!("unsupported key size: {bits}").into());
                };
                gen_private_key(algo, bits).map_err(|e| format!("{e}").into())
            },
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use openssl::pkey::Id;

    #[test]
    fn ed25519_produces_valid_pkcs8_pem() {
        let pem = gen_private_key("ed25519", 0).unwrap();
        assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----"));
        let key = PKey::private_key_from_pem(pem.as_bytes()).unwrap();
        assert_eq!(key.id(), Id::ED25519);
    }

    #[test]
    fn rsa_produces_valid_pkcs8_pem() {
        let pem = gen_private_key("rsa", 2048).unwrap();
        assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----"));
        let key = PKey::private_key_from_pem(pem.as_bytes()).unwrap();
        assert_eq!(key.id(), Id::RSA);
        assert_eq!(key.bits(), 2048);
    }

    // ── Scenario « reconnaissance d'algorithme insensible à la casse » (Then sur `ED25519`) ──
    // Refuse : un `ED25519` qui partirait sur la voie rsa, ou sur toute voie rendant un PEM
    // relisable mais pas ed25519 — le « PEM ed25519 valide » est verrouillé par la relecture
    // (en-tête + @openssl::pkey::PKey::private_key_from_pem + id()), voie déjà en place dans
    // les verrous voisins du module. Les deux `And` (`Ed25519`, `Rsa` 2048) jouent dans
    // `comparison_stays_case_insensitive`, déjà complet.
    #[test]
    fn algorithm_is_case_insensitive() {
        let pem = gen_private_key("ED25519", 0).unwrap();
        assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----"));
        let key = PKey::private_key_from_pem(pem.as_bytes()).unwrap();
        assert_eq!(
            key.id(),
            Id::ED25519,
            "un `ED25519` majuscule doit rendre ed25519"
        );
    }

    #[test]
    fn unknown_algorithm_is_an_error() {
        assert!(gen_private_key("dsa", 0).is_err());
    }

    // ── Scenario « deux générations consécutives diffèrent » ──
    // Refuse : une génération figée (deux PEM égaux) et, par l'`And`, une voie ed25519 qui
    // rendrait des PEM distincts mais non relisibles ou d'un autre algorithme — chacun se
    // relit en clé ed25519, même voie de relecture que les verrous voisins.
    #[test]
    fn two_generations_differ() {
        let a = gen_private_key("ed25519", 0).unwrap();
        let b = gen_private_key("ed25519", 0).unwrap();
        assert_ne!(a, b);
        for pem in [&a, &b] {
            let key = PKey::private_key_from_pem(pem.as_bytes()).unwrap();
            assert_eq!(key.id(), Id::ED25519, "chaque PEM rendu se relit en clé ed25519");
        }
    }

    #[test]
    fn unknown_algorithm_keeps_original_case() {
        let err = gen_private_key("DSA", 0).expect_err("DSA must be rejected");
        let crate::Error::UnsupportedKeyAlgorithm(carried) = &err else {
            panic!("expected UnsupportedKeyAlgorithm, got {err:?}");
        };
        assert_eq!(carried, "DSA", "the carried text must keep the original case");
        assert_eq!(err.to_string(), "KEY-ALGO-001 Unsupported key algorithm: DSA");
    }

    #[test]
    fn comparison_stays_case_insensitive() {
        let pem = gen_private_key("Ed25519", 0).unwrap();
        let key = PKey::private_key_from_pem(pem.as_bytes()).unwrap();
        assert_eq!(key.id(), Id::ED25519);
        let pem = gen_private_key("Rsa", 2048).unwrap();
        let key = PKey::private_key_from_pem(pem.as_bytes()).unwrap();
        assert_eq!(key.id(), Id::RSA);
        assert_eq!(key.bits(), 2048);
    }

    // ── Scenario « l'algo vide est refusé avec un texte vide » ──
    // Refuse : une chaîne vide qui tomberait sur une voie de génération (Ok rendu), qui
    // sortirait du module sous une autre variante (OpenSSL, Other…), ou dont l'affichage
    // rognerait le séparateur final — attendu exact avec l'espace qui termine le Display.
    #[test]
    fn scenario_l_algo_vide_est_refuse_avec_un_texte_vide() {
        let err = gen_private_key("", 0).expect_err("la chaîne vide doit être refusée");
        let crate::Error::UnsupportedKeyAlgorithm(carried) = &err else {
            panic!("expected UnsupportedKeyAlgorithm, got {err:?}");
        };
        assert_eq!(
            carried, "",
            "le texte porté est l'entrée vide, non une substitution"
        );
        assert_eq!(err.to_string(), "KEY-ALGO-001 Unsupported key algorithm: ");
    }

    // ── Scenario « Rust ignore les tailles aberrantes pour ed25519 » ──
    // Refuse : tout pré-filtre de `bits` devant la voie ed25519 (un garde sur 7 ou
    // u32::MAX rendrait Err, souvent en Error::OpenSSL) et toute taille qui détournerait
    // l'algorithme — oracle indépendant de l'appel : DER relu par OpenSSL (`id()`),
    // en-tête PEM, et non comparaison des deux PEM entre eux.
    #[test]
    fn scenario_rust_ignore_les_tailles_aberrantes_pour_ed25519() {
        for bits in [7_u32, u32::MAX] {
            let pem = gen_private_key("ed25519", bits)
                .unwrap_or_else(|e| panic!("`bits` = {bits} doit être ignoré, obtenu {e:?}"));
            assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----"));
            let key = PKey::private_key_from_pem(pem.as_bytes()).unwrap();
            assert_eq!(
                key.id(),
                Id::ED25519,
                "la taille {bits} ne doit pas changer l'algo"
            );
        }
    }

    // ── Scenario « sans rhai la face Rust reste et la porte script ferme » ──
    // Joué uniquement sur la porte `--no-default-features --features crypto` : verrou de
    // compilation — `key_rhai_register` n'existe pas dans ce graphe (sa propre porte
    // `#[cfg(feature = "rhai")]` fait défaut, rien à appeler pour l'affirmer) et si
    // `gen_private_key` ou `DEFAULT_RSA_BITS` passaient un jour derrière `rhai`, ce test
    // ne compilerait plus sur cette porte. Aligné sur `surface_without_features` de
    // ./lib.rs et sur `scenario_api_rust_vivante_sans_la_feature_rhai` de ./chrono.rs.
    // Le Scenario jumeau « sans crypto le module et ses helpers disparaissent » n'a pas
    // d'équivalent ici : sous `--no-default-features` le module entier n'est pas compilé,
    // clause tenue par la batterie (voir src/key.sdd).
    #[cfg(not(feature = "rhai"))]
    #[test]
    fn scenario_sans_rhai_la_face_rust_reste_et_la_porte_script_ferme() {
        let pem = gen_private_key("ed25519", 0).unwrap();
        assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----"));
        let key = PKey::private_key_from_pem(pem.as_bytes()).unwrap();
        assert_eq!(key.id(), Id::ED25519);
        assert_eq!(
            DEFAULT_RSA_BITS, 4096,
            "la constante contractuelle reste appelée sans rhai"
        );
    }

    #[cfg(feature = "rhai")]
    mod script {
        use super::*;
        use crate::Script;
        use openssl::pkey::Private;
        use rhai::ImmutableString;

        // Engine fraîchement enregistré par `key_rhai_register`.
        fn engine() -> Engine {
            let mut e = Engine::new();
            crate::key::key_rhai_register(&mut e);
            e
        }

        // Évalue un script censé rendre un PEM et décode la chaîne rendue : oracle
        // indépendant de l'appel testé (DER relu par OpenSSL sous un autre nom, en-tête
        // PEM), jamais comparaison de la sortie à elle-même ni à une seconde génération.
        fn eval_key(e: &Engine, src: &str) -> PKey<Private> {
            let out = e
                .eval::<ImmutableString>(src)
                .unwrap_or_else(|err| panic!("`{src}` doit réussir, obtenu {err:?}"));
            assert!(
                out.starts_with("-----BEGIN PRIVATE KEY-----"),
                "`{src}` doit rendre un PEM PKCS8, obtenu {out}"
            );
            PKey::private_key_from_pem(out.as_bytes())
                .unwrap_or_else(|err| panic!("le PEM rendu par `{src}` doit se relire, obtenu {err}"))
        }

        // Le garde du collage binaire est la seule variante d'erreur du refus sans
        // génération : ErrorRuntime porteur du texte du garde, jamais le Display d'un
        // @crate::Error (préfixes KEY-…). Un appel qui aurait dispatché avant d'échouer
        // rendrait soit un Ok (ed25519 ignore `bits`), soit un texte KEY-OPENSSL-001.
        fn assert_guard_rejection(e: &Engine, src: &str, expected: &str) {
            let err = e
                .eval::<rhai::Dynamic>(src)
                .expect_err(&format!("`{src}` doit échouer avant toute génération"));
            let rhai::EvalAltResult::ErrorRuntime(msg, _) = err.as_ref() else {
                panic!("attendu l'erreur runtime du garde, obtenu {err:?}");
            };
            assert!(msg.is_string(), "le garde rend un texte brut, obtenu {msg:?}");
            let text = msg.to_string();
            assert_eq!(
                text, expected,
                "message exact du garde, aucun texte d'Error toléré"
            );
            assert!(
                !text.contains("KEY-ALGO-001") && !text.contains("KEY-OPENSSL-001"),
                "le message reçu ne doit porter aucun préfixe d'@crate::Error : {text}"
            );
        }

        // ── Scenario « la surcharge unaire de script génère ed25519 » ──
        // Refuse : unaire absente ou branchée sur rsa (id() divergerait), une sortie non
        // chaîne (`eval::<ImmutableString>` ou `type_of` rougiraient), un rendu non décodable.
        #[test]
        fn scenario_la_surcharge_unaire_de_script_genere_ed25519() {
            let e = engine();
            let key = eval_key(&e, r#"gen_private_key("ed25519")"#);
            assert_eq!(key.id(), Id::ED25519);
            let ty = e
                .eval::<ImmutableString>(r#"type_of(gen_private_key("ed25519"))"#)
                .expect("le PEM doit être vu comme un `string` par `type_of`");
            assert_eq!(ty.as_str(), "string");
        }

        // ── Scenario « la surcharge unaire de script applique 4096 sur rsa » ──
        // Seule génération 4096 de toute la suite, donc sa plus coûteuse. Refuse : unaire
        // branchée autre part que DEFAULT_RSA_BITS (bits() lu dans
        // le PEM décodé — voie indépendante — cesserait de valoir 4096) et une constante
        // dérivée de la valeur testée (4096 est posé littéral, DEFAULT_RSA_BITS vérifié
        // égal en plus, non l'inverse).
        #[test]
        fn scenario_la_surcharge_unaire_de_script_applique_4096_sur_rsa() {
            let e = engine();
            let key = eval_key(&e, r#"gen_private_key("rsa")"#);
            assert_eq!(key.id(), Id::RSA);
            assert_eq!(
                key.bits(),
                4096,
                "l'unaire doit appliquer la taille contractuelle 4096"
            );
            assert_eq!(DEFAULT_RSA_BITS, 4096);
        }

        // ── Scenario « la surcharge binaire de script dimensionne rsa » ──
        // Refuse : binaire absente, taille ignorée (bits() vaudrait 4096 de la constante
        // ou 0 du refus) ou appliquée hors bornes u32.
        #[test]
        fn scenario_la_surcharge_binaire_de_script_dimensionne_rsa() {
            let e = engine();
            let key = eval_key(&e, r#"gen_private_key("rsa", 2048)"#);
            assert_eq!(key.id(), Id::RSA);
            assert_eq!(key.bits(), 2048, "le 2048 du script doit dimensionner la clef");
        }

        // ── Scenario « le nombre de bits négatif est refusé sans génération » ──
        // Refuse : une implémentation qui génère avant de refuser — ed25519 ignorant
        // `bits`, un dispatch aurait rendu Ok ; pour rsa, un dispatch (par exemple un
        // cast tronquant -1 en u32) aurait rendu un texte KEY-OPENSSL-001. La variante
        // exacte ErrorRuntime au message du garde, sans préfixe KEY-, est la preuve
        // observable de la coupure avant dispatch (garde posé devant tout dispatch).
        #[test]
        fn scenario_le_nombre_de_bits_negatif_est_refuse_sans_generation() {
            let e = engine();
            assert_guard_rejection(
                &e,
                r#"gen_private_key("ed25519", -1)"#,
                "unsupported key size: -1",
            );
            assert_guard_rejection(&e, r#"gen_private_key("rsa", -1)"#, "unsupported key size: -1");
        }

        // ── Scenario « la taille au-delà de u32 est refusé sans génération » ──
        // Même preuve : ed25519 aurait rendu Ok sous dispatch ; rsa sous cast tronquant
        // (4294967296 → 0) aurait rendu KEY-OPENSSL-001, non le texte du garde. Le message
        // porte l'i64 reçu tel quel, non sa troncature.
        #[test]
        fn scenario_la_taille_au_dela_de_u32_est_refuse_sans_generation() {
            let e = engine();
            assert_guard_rejection(
                &e,
                r#"gen_private_key("ed25519", 4294967296)"#,
                "unsupported key size: 4294967296",
            );
            assert_guard_rejection(
                &e,
                r#"gen_private_key("rsa", 4294967296)"#,
                "unsupported key size: 4294967296",
            );
        }

        // ── Scenario « la binaire admet une taille aberrante pour ed25519 » ──
        // Refuse : un garde qui filtrerait les tailles avant le dispatch même pour
        // ed25519 (7 est licité en u32, doit passer et être ignorée).
        #[test]
        fn scenario_la_binaire_admet_une_taille_aberrante_pour_ed25519() {
            let e = engine();
            let key = eval_key(&e, r#"gen_private_key("ed25519", 7)"#);
            assert_eq!(key.id(), Id::ED25519);
        }

        // ── Scenario « la taille zéro sur rsa laisse OpenSSL décider » ──
        // Refuse : un pré-filtre du module sur `0` (le message serait `unsupported key
        // size: 0`, variante du garde) ; la suite du texte est laissée à openssl et
        // jamais verrouillée (seul le préfixe KEY-OPENSSL-001 est contractuel).
        #[test]
        fn scenario_la_taille_zero_sur_rsa_laisse_openssl_decider() {
            let e = engine();
            let err = e
                .eval::<rhai::Dynamic>(r#"gen_private_key("rsa", 0)"#)
                .expect_err("0 bit hors bornes usuelles, OpenSSL doit trancher par le refus");
            // Le Display de l'`EvalAltResult` préfixe « Runtime error: » (mesuré sous la
            // version de rhai verrouillée) : on lit le texte porté par la variante, seule
            // voie qui verrouille le `format!("{e}")` de la fermeture, pas l'affichage
            // de la harness.
            let text = match *err {
                rhai::EvalAltResult::ErrorRuntime(msg, _) => msg.to_string(),
                other => panic!("attendu l'erreur runtime remontée par la fermeture, obtenu {other:?}"),
            };
            assert!(
                text.starts_with("KEY-OPENSSL-001 OpenSSL error "),
                "attendu le préfixe KEY-OPENSSL-001 de la remontée OpenSSL, obtenu {text}"
            );
            assert_ne!(
                text, "unsupported key size: 0",
                "0 est dans la plage du garde, il ne filtre pas"
            );
        }

        // ── Scenario « l'engine sans enregistrement ne connaît aucun des deux noms » ──
        // Refuse : un enregistrement qui fuiterait sur tout `Engine::new` sans passer par
        // `key_rhai_register` (les deux appels n'échoueraient pas en ErrorFunctionNotFound).
        // Le même engine, enregistré dans la foulée, doit voir les deux appels réussir.
        #[test]
        fn scenario_l_engine_sans_enregistrement_ne_connet_aucun_des_deux_noms() {
            let mut fresh = Engine::new();
            for src in [r#"gen_private_key("ed25519")"#, r#"gen_private_key("rsa", 2048)"#] {
                let err = fresh.eval::<rhai::Dynamic>(src).expect_err(&format!(
                    "`{src}` doit être une fonction inconnue sans enregistrement"
                ));
                assert!(
                    matches!(err.as_ref(), rhai::EvalAltResult::ErrorFunctionNotFound(..)),
                    "attendu une erreur de fonction inconnue, obtenu {err:?}"
                );
            }
            crate::key::key_rhai_register(&mut fresh);
            let out = fresh
                .eval::<ImmutableString>(r#"gen_private_key("ed25519")"#)
                .expect("après enregistrement, l'unaire doit réussir");
            assert!(out.starts_with("-----BEGIN PRIVATE KEY-----"));
            let key = eval_key(&fresh, r#"gen_private_key("rsa", 2048)"#);
            assert_eq!(key.id(), Id::RSA);
        }

        // ── Scenario « new_bare livre les deux surcharges » ──
        // Refuse : un `Script::new_bare` qui n'appellerait pas `key_rhai_register` (le
        // câblage `#[cfg(feature = "crypto")]` dans ./engine.rs sauterait) ou n'en
        // brancherait qu'une — chaque chaîne rendue doit de plus être un PEM PKCS8.
        #[test]
        fn scenario_new_bare_livre_les_deux_surcharges() {
            let script = Script::new_bare(vec![]);
            for src in [r#"gen_private_key("ed25519")"#, r#"gen_private_key("rsa", 2048)"#] {
                let out = script
                    .engine
                    .eval::<ImmutableString>(src)
                    .unwrap_or_else(|err| panic!("`new_bare` doit livrer `{src}`, obtenu {err:?}"));
                assert!(
                    out.starts_with("-----BEGIN PRIVATE KEY-----"),
                    "`{src}` via `new_bare` doit rendre un PEM PKCS8"
                );
            }
        }

        // ── Scenario « aucune coercition scalaire sur les arguments de script » ──
        // Refuse : toute conversion implicite d'un scalaire (entier, booléen, flottant,
        // unité, chaîne en position deux) vers &str ou i64, et toute arité autre que unaire
        // ou binaire. Le refus de type se distingue de la génération ratée par la variante
        // : ErrorFunctionNotFound est émis par rhai au dispatch, avant que la fermeture
        // s'exécute — un texte KEY-… ou `unsupported key size:` dans l'erreur prouverait
        // que gen_private_key a tourné, d'où l'affirmation négative en plus du matches!.
        #[test]
        fn scenario_aucune_coercition_scalaire_sur_les_arguments_de_script() {
            let e = engine();
            for src in [
                r"gen_private_key(1)",
                r"gen_private_key(true)",
                r"gen_private_key(1.5)",
                r"gen_private_key(())",
                r#"gen_private_key("rsa", "2048")"#,
                r#"gen_private_key("rsa", true)"#,
                r#"gen_private_key("rsa", 2048.0)"#,
                r#"gen_private_key("rsa", 2048, 0)"#,
            ] {
                let err = e
                    .eval::<rhai::Dynamic>(src)
                    .expect_err(&format!("`{src}` doit échouer au dispatch, sans coercition"));
                assert!(
                    matches!(err.as_ref(), rhai::EvalAltResult::ErrorFunctionNotFound(..)),
                    "`{src}` : refus de type attendu (`ErrorFunctionNotFound` émis avant l'appel), obtenu {err:?}"
                );
                let text = err.to_string();
                assert!(
                    !text.contains("KEY-ALGO-001")
                        && !text.contains("KEY-OPENSSL-001")
                        && !text.contains("unsupported key size"),
                    "aucune exécution de gen_private_key tolérée pour `{src}` : {text}"
                );
            }
        }

        // ── Scenario « le réenregistrement est sans effet visible » ──
        // Refuse : une seconde `key_rhai_register` qui cassero la dispatch (fonction
        // devenue inconnue ou ambiguë), perdrait le garde, ou brancherait une autre
        // taille — binaire à taille licite et garde du négatif rejoués après doublon.
        #[test]
        fn scenario_le_reenregistrement_est_sans_effet_visible() {
            let mut e = engine();
            crate::key::key_rhai_register(&mut e);
            let key = eval_key(&e, r#"gen_private_key("ed25519", 2048)"#);
            assert_eq!(key.id(), Id::ED25519, "la binaire doit survivre au doublon");
            assert_guard_rejection(
                &e,
                r#"gen_private_key("ed25519", -1)"#,
                "unsupported key size: -1",
            );
        }
    }
}
