//! Seam `client_name.sdd` — Scenario « l'API est présente sans aucune feature ».
//!
//! Aucun `#[cfg(...)]` n'apparaît dans ce fichier : il compile et s'exécute sous toutes
//! les portes de la batterie, dont `--no-default-features` (porte où ce Scenario a son
//! sens). Ce que ce binaire ajoute par rapport au seam `surface_without_features` de
//! `src/lib.rs` (dispatch n°2, inline lib) :
//!
//! - le seam inline **référence** les trois fonctions (`black_box`) sous
//!   `--no-default-features` mais ne les appelle pas : dans le binaire d'unit-tests de la
//!   lib, remplir le `OnceLock` rendrait l'ordre des tests sensible ;
//! - ici, processus dédié dont la première écriture est libre : les trois fonctions
//!   sont **appelées**
//!   (séquence complète `is_set` faux → `set` → `is_set` vrai → lecture), **depuis la
//!   racine** `vynil_core::` (la vue du consommateur, hors de la crate) en plus du module
//!   `vynil_core::client_name::`, et l'identité des alias racine ↔ module est verrouillée
//!   par `fn_addr_eq` (réexport sans wrapper, clause `Must` de la spec).

#[test]
fn api_presente_sans_aucune_feature() {
    // Réexport sans wrapper ni renommage : les alias racine sont les mêmes items que les
    // versions module (mêmes adresses de fonction).
    let set_racine: fn(fn() -> String) = vynil_core::set_client_name;
    let set_module: fn(fn() -> String) = vynil_core::client_name::set_client_name;
    assert!(
        std::ptr::fn_addr_eq(set_racine, set_module),
        "vynil_core::set_client_name doit être l'item exact de client_name::set_client_name"
    );
    let est_installe_racine: fn() -> bool = vynil_core::client_name_is_set;
    let est_installe_module: fn() -> bool = vynil_core::client_name::client_name_is_set;
    assert!(
        std::ptr::fn_addr_eq(est_installe_racine, est_installe_module),
        "vynil_core::client_name_is_set doit être l'item exact de client_name::client_name_is_set"
    );
    let lecture_racine: fn() -> String = vynil_core::get_client_name;
    let lecture_module: fn() -> String = vynil_core::client_name::get_client_name;
    assert!(
        std::ptr::fn_addr_eq(lecture_racine, lecture_module),
        "vynil_core::get_client_name doit être l'item exact de client_name::get_client_name"
    );

    // séquence d'appel complète, vue consommateur (racine) — processus neuf, aucune autre
    // écriture n'a couru ici.
    assert!(
        !vynil_core::client_name_is_set(),
        "aucun set_client_name n'a couru dans ce processus : is_set doit être false"
    );
    vynil_core::set_client_name(|| "surface.example.com".to_string());
    assert!(
        vynil_core::client_name_is_set(),
        "le set appelé depuis la racine doit installer l'identité"
    );
    assert_eq!(
        vynil_core::client_name::get_client_name(),
        "surface.example.com",
        "la lecture par le chemin module doit voir l'identité installée par le chemin racine"
    );
}
