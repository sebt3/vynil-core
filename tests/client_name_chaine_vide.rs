//! Seam `client_name.sdd` — Scenario « la chaîne vide est une configuration comme une autre ».
//!
//! Binaire dédié : la chaîne vide est la première écriture de ce processus ; le module
//! doit la traiter comme n'importe quelle valeur (pass-through strict, aucun rejet).

#[test]
fn la_chaine_vide_est_une_configuration_comme_une_autre() {
    // Given : configuration par une closure retournant `""`.
    vynil_core::client_name::set_client_name(String::new);

    // When/Then : présence vraie, lecture vide — aucune validation, trim ni rejet.
    assert!(
        vynil_core::client_name::client_name_is_set(),
        "la chaîne vide est une configuration acceptée : client_name_is_set doit être true"
    );
    assert_eq!(
        vynil_core::client_name::get_client_name(),
        "",
        "la valeur vide doit être restituée telle quelle, sans normalisation"
    );
}
