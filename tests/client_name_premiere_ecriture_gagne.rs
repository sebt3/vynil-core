//! Seam `client_name.sdd` — Scenario « la première configuration gagne sans bruit ».
//!
//! Binaire dédié : ce processus installe `first.example.com` comme première écriture
//! (le Given du Scenario), puis vérifie qu'un second appel est ignoré silencieusement.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn la_premiere_configuration_gagne_sans_bruit() {
    // Given : l'identité est déjà configurée avec `first.example.com` (première écriture
    // de ce processus).
    vynil_core::client_name::set_client_name(|| "first.example.com".to_string());
    assert_eq!(
        vynil_core::get_client_name(),
        "first.example.com",
        "Given du Scenario : l'identité installée doit être lisible avant le second appel"
    );

    // When : un second set_client_name tente d'installer `second.example.com`.
    // Le compteur partagé verrouille la clause la plus forte : la closure écartée n'est
    // jamais appelée, ni à l'enregistrement ni aux lectures (pas seulement « la valeur
    // lue ne change pas »).
    let secondes_appels = Arc::new(AtomicUsize::new(0));
    let compteur = Arc::clone(&secondes_appels);
    // Then (partiel) : `()` est retourné même quand l'appel est ignoré — verrouillé à la
    // compilation par la coercition de l'item en pointeur de fonction à retour unitaire.
    let _: fn(fn() -> String) = vynil_core::client_name::set_client_name;
    vynil_core::client_name::set_client_name(move || {
        compteur.fetch_add(1, Ordering::Relaxed);
        "second.example.com".to_string()
    });

    // Then : la lecture reste sur la première identité et la closure écartée n'a jamais
    // tourné.
    assert_eq!(
        vynil_core::get_client_name(),
        "first.example.com",
        "le second set_client_name doit être ignoré sans bruit : la closure d'origine reste seule installée"
    );
    assert_eq!(
        secondes_appels.load(Ordering::Relaxed),
        0,
        "la closure écartée ne doit jamais être appelée (ni à l'enregistrement, ni à la lecture)"
    );
}
