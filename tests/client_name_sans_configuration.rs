//! Seam `client_name.sdd` — Scenario « la lecture qui rate ne verrouille pas la configuration ».
//!
//! Binaire dédié : le Given exige un processus jamais configuré où la lecture a d'abord
//! paniqué, puis un `set_client_name` postérieur qui installe normalement l'identité.

use std::panic::{AssertUnwindSafe, catch_unwind};

#[test]
fn la_lecture_qui_rate_ne_verrouille_pas_la_configuration() {
    // Given : get_client_name panique d'abord faute de configuration (hook neutralisé le
    // temps du catch : le panic est le contrat, son payload est rattrapé).
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = catch_unwind(AssertUnwindSafe(vynil_core::get_client_name));
    std::panic::set_hook(previous_hook);
    assert!(
        outcome.is_err(),
        "Given du Scenario : la lecture non configurée doit d'abord paniquer"
    );

    // La lecture échouée n'a pas rempli CLIENT_NAME : l'état reste libre.
    assert!(
        !vynil_core::client_name::client_name_is_set(),
        "un get_client_name ayant paniqué ne doit pas consommer le OnceLock"
    );

    // When : la configuration arrive après le panic raté.
    vynil_core::client_name::set_client_name(|| "late-app.example.com".to_string());

    // Then : elle s'installe normalement et la lecture devient correcte.
    assert!(
        vynil_core::client_name::client_name_is_set(),
        "le set_client_name postérieur au panic doit être accepté"
    );
    assert_eq!(
        vynil_core::get_client_name(),
        "late-app.example.com",
        "la configuration tardive doit être lisible normalement"
    );
}
