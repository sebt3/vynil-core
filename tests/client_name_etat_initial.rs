//! Seam `client_name.sdd` — Scenario « état avant toute configuration ».
//!
//! Binaire dédié parce que `CLIENT_NAME` est un `OnceLock` de processus : ce processus-ci
//! ne remplit jamais le statique, seul `client_name_is_set` est appelé.

#[test]
fn etat_avant_toute_configuration() {
    // Given : aucun set_client_name n'a jamais été appelé dans ce processus.
    assert!(
        !vynil_core::client_name::client_name_is_set(),
        "avant toute configuration, client_name_is_set doit renvoyer false dans un processus neuf"
    );
}
