//! Seam `client_name.sdd` — Scenario « lecture non configurée, message de panic actionnable ».
//!
//! Binaire dédié : la lecture qui panique ne remplit pas le `OnceLock` mais exige un
//! processus jamais configuré pour être observée. Le panic est rattrapé par
//! `std::panic::catch_unwind` et son message est verrouillé caractère par caractère
//! (tiret long « — », parenthèses et ponctuation compris).

use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
};

/// Message verrouillé du contrat (spec `client_name.sdd`, clause `Raises`) : copié ici
/// caractère par caractère depuis le `expect` de `src/client_name.rs`.
const NOT_CONFIGURED: &str = concat!(
    "vynil_core: client name not configured — call ",
    "vynil_core::set_client_name(...) before performing HTTP or Kubernetes requests",
);

/// Rendu texte d'un payload de panic : les panics Rust portent un `Box<dyn Any + Send>`,
/// presque toujours un `&'static str` (`panic!` littéral) ou un `String` (message formaté,
/// dont celui d'`Option::expect`). `None` = payload d'un autre type, donc typé.
fn panic_text(payload: &(dyn Any + Send)) -> Option<String> {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        Some((*s).to_string())
    } else {
        payload.downcast_ref::<String>().cloned()
    }
}

#[test]
fn lecture_non_configuree_message_de_panic_actionnable() {
    // Le panic étant le contrat, on neutralise le hook pendant la fenêtre de catch pour
    // garder une sortie de test propre (le payload, lui, est rattrapé et asserté).
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = catch_unwind(AssertUnwindSafe(vynil_core::get_client_name));
    std::panic::set_hook(previous_hook);

    match outcome {
        Err(payload) => {
            // « aucun crate::Error ni valeur rattrapable n'est produit » : le payload est
            // verrouillé comme texte exact, ce qui exclut déjà tout type typé (seuls
            // &str/String rendent un Some ci-dessous). La double vérification explicite
            // porte sur crate::Error, le seul « rattrapable typé » que la spec nomme.
            assert!(
                payload.downcast_ref::<vynil_core::Error>().is_none(),
                "le payload ne doit jamais être un crate::Error, obtenu : {:?}",
                (*payload).type_id()
            );
            assert_eq!(
                panic_text(&*payload).as_deref(),
                Some(NOT_CONFIGURED),
                "le panic doit porter exactement le message verrouillé de la spec \
                 (payload non texte ou message divergent)"
            );
        }
        Ok(name) => assert_eq!(
            name.as_str(),
            "valeur retournée alors que la lecture devait paniquer",
            "get_client_name non configuré doit paniquer, pas retourner une valeur"
        ),
    }
}
