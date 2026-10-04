//! Seam `client_name.sdd` — Scenario « configurations concurrentes, un seul gagnant ».
//!
//! Binaire dédié : dix threads se précipitent sur la première écriture du `OnceLock`
//! (départ synchronisé par `std::sync::Barrier`), puis la présence et la stabilité de la
//! lecture sont vérifiées dans le thread principal.

/// Les dix noms soumis, distincts — l'identité gagnante doit être l'un d'eux.
const NOMS: [&str; 10] = [
    "node-0.example.com",
    "node-1.example.com",
    "node-2.example.com",
    "node-3.example.com",
    "node-4.example.com",
    "node-5.example.com",
    "node-6.example.com",
    "node-7.example.com",
    "node-8.example.com",
    "node-9.example.com",
];

#[test]
fn configurations_concurrentes_un_seul_gagnant() {
    // Given : dix threads, un nom distinct chacun, départ simultané sur la barrière.
    let barriere = std::sync::Barrier::new(NOMS.len() + 1);
    let barriere = &barriere;
    std::thread::scope(|s| {
        for name in NOMS {
            s.spawn(move || {
                barriere.wait();
                vynil_core::client_name::set_client_name(move || name.to_string());
            });
        }
        barriere.wait();
        // Le scope joint tous les threads à sa sortie : « les dix threads ont terminé ».
    });

    // Then : présence installée…
    assert!(
        vynil_core::client_name::client_name_is_set(),
        "dix set_client_name concurrents doivent laisser l'identité configurée"
    );

    // … et lecture stable sur toutes les relectures, égale à l'un des dix noms soumis.
    let premiere_lecture = vynil_core::get_client_name();
    for relecture in 1..=5 {
        assert_eq!(
            vynil_core::get_client_name(),
            premiere_lecture,
            "relecture {relecture} : le gagnant départagé par le OnceLock doit être stable"
        );
    }
    assert!(
        NOMS.contains(&premiere_lecture.as_str()),
        "l'identité gagnante doit être l'un des dix noms soumis, obtenu : {premiere_lecture}"
    );
}
