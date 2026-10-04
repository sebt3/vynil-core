//! Seam `client_name.sdd` — Scenario « la closure est réévaluée à chaque lecture ».
//!
//! Binaire dédié : la closure à compteur est la première écriture de ce processus ; deux
//! lectures consécutives doivent rendre deux chaînes différentes (aucune mise en cache).

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn la_closure_est_reevaluee_a_chaque_lecture() {
    // Given : une closure qui retourne une valeur différente à chaque appel (compteur
    // interne, numéro de l'appel incrusté dans la valeur).
    let appels = Arc::new(AtomicUsize::new(0));
    let compteur = Arc::clone(&appels);
    vynil_core::client_name::set_client_name(move || {
        let n = compteur.fetch_add(1, Ordering::Relaxed);
        format!("reevaluee-{n}.example.com")
    });

    // When : deux lectures d'affilée.
    let premiere = vynil_core::client_name::get_client_name();
    let seconde = vynil_core::get_client_name();

    // Then : les deux chaînes diffèrent — le module ne met aucune valeur en cache, chaque
    // lecture consomme un appel frais de la closure.
    assert_ne!(
        premiere, seconde,
        "get_client_name doit réinvoquer la closure à chaque appel, jamais servir un cache"
    );
    assert_eq!(
        (premiere.as_str(), seconde.as_str()),
        ("reevaluee-0.example.com", "reevaluee-1.example.com"),
        "chaque lecture doit déclencher exactement un appel de la closure"
    );
    assert_eq!(
        appels.load(Ordering::Relaxed),
        2,
        "deux lectures = deux appels de la closure, ni plus ni moins"
    );
}
