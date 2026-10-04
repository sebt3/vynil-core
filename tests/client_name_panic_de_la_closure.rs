//! Seam `client_name.sdd` — Scenario « le panic de la closure se propage tel quel ».
//!
//! Binaire dédié : la closure qui panique est la première écriture de ce processus ;
//! `catch_unwind` doit rattraper `boom`, jamais le message de non-configuration.

use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
};

/// Message de non-configuration (`client_name.sdd`) : le payload rattrapé ne doit surtout
/// pas être celui-là, preuve que le panic de la closure n'est ni capturé ni remplacé.
const NOT_CONFIGURED: &str = concat!(
    "vynil_core: client name not configured — call ",
    "vynil_core::set_client_name(...) before performing HTTP or Kubernetes requests",
);

/// Rendu texte d'un payload de panic (`Box<dyn Any + Send>` : `&'static str` pour un
/// `panic!` littéral, `String` pour un message formaté). `None` = payload typé.
fn panic_text(payload: &(dyn Any + Send)) -> Option<String> {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        Some((*s).to_string())
    } else {
        payload.downcast_ref::<String>().cloned()
    }
}

#[test]
fn le_panic_de_la_closure_se_propage_tel_quel() {
    // Given : une closure installée (première écriture de ce processus) qui panique avec
    // `boom`.
    #[allow(clippy::panic)]
    // le panic « boom » est l'objet même du Scenario, pas du code de production (client_name.sdd)
    vynil_core::client_name::set_client_name(|| panic!("boom"));

    // Hook neutralisé le temps du catch : le panic est le contrat, son payload est asserté.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = catch_unwind(AssertUnwindSafe(vynil_core::get_client_name));
    std::panic::set_hook(previous_hook);

    match outcome {
        Err(payload) => {
            let texte = panic_text(&*payload);
            assert_eq!(
                texte.as_deref(),
                Some("boom"),
                "le panic de la closure doit se propager tel quel, payload texte attendu : « boom »"
            );
            assert_ne!(
                texte.as_deref(),
                Some(NOT_CONFIGURED),
                "le panic de la closure ne doit jamais être remplacé par le message de non-configuration"
            );
        }
        Ok(name) => assert_eq!(
            name.as_str(),
            "valeur retournée alors que la closure devait paniquer",
            "get_client_name sur closure panicale doit propager le panic, pas retourner une valeur"
        ),
    }
}
