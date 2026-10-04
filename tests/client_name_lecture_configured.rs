//! Seam `client_name.sdd` — Scenario « lecture du nom configuré ».
//!
//! Binaire dédié : ce processus installe `my-app.example.com` comme première (et seule)
//! écriture du `OnceLock`, puis vérifie lecture et test de présence.

#[test]
fn lecture_du_nom_configure() {
    // Given : configuration par une closure retournant `my-app.example.com`.
    vynil_core::client_name::set_client_name(|| "my-app.example.com".to_string());

    // Then : la lecture restitue la valeur telle quelle et la présence est vraie.
    assert_eq!(
        vynil_core::client_name::get_client_name(),
        "my-app.example.com",
        "get_client_name doit restituer la valeur produite par la closure installée"
    );
    assert!(
        vynil_core::client_name::client_name_is_set(),
        "client_name_is_set doit renvoyer true après un set_client_name accepté"
    );
}
