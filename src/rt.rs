//! Internal async → sync bridge shared by the network-facing modules (`http`, `oci`,
//! `s3`, `k8s`), which expose synchronous functions to Rhai scripts while every
//! network call underneath is `tokio`-async. One bridge, one strategy table (see
//! [`block_on`]), no panic escaping towards the host.

use std::future::Future;

use tokio::{
    runtime::{Builder, Handle, RuntimeFlavor},
    task::block_in_place,
};

use crate::{Error, Result};

/// Drive `fut` to completion from a synchronous (script) caller and return its
/// output, without ever panicking the host.
///
/// Strategy, in this order, decided from [`Handle::try_current`]:
///
/// - a **multi-thread** tokio runtime is running on this thread: block the current
///   thread with [`block_in_place`] and drive `fut` through the live [`Handle`]
///   (the runtime's worker threads keep making progress meanwhile);
/// - **no runtime** on this thread: build a temporary `current_thread` runtime with
///   `enable_all`, drive `fut` on it, then drop the runtime on return. This path is
///   a *fallback* — it pays a runtime construction per call; in production scripts
///   run inside the consumer's own runtime and this branch is never reached;
/// - a **`current_thread`** runtime is running: [`block_in_place`] would panic
///   there, so `fut` is never polled and [`Error::Other`] “requires a multi-thread
///   tokio runtime” is returned instead.
///
/// `fut` need not be `Send`: it is always driven on the calling thread. If the
/// temporary runtime cannot be built (e.g. descriptor exhaustion), [`Error::Stdio`]
/// is returned. A panic raised by `fut` itself propagates untouched — it belongs to
/// the caller; this module never adds one.
pub(crate) fn block_on<F: Future>(fut: F) -> Result<F::Output> {
    if let Ok(handle) = Handle::try_current() {
        return match handle.runtime_flavor() {
            RuntimeFlavor::MultiThread => Ok(block_in_place(|| handle.block_on(fut))),
            // `CurrentThread` (and any hypothetical future non-multi-thread flavor)
            // cannot host `block_in_place`: refuse without touching the future.
            RuntimeFlavor::CurrentThread | _ => {
                Err(Error::Other("requires a multi-thread tokio runtime".to_string()))
            }
        };
    }
    // No runtime on this thread: one-off `current_thread` runtime, dropped on return.
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::Stdio)?;
    Ok(runtime.block_on(fut))
}

#[cfg(test)]
mod tests {
    use super::block_on;
    use crate::Error;
    use std::{cell::RefCell, rc::Rc};

    // ── Scenario « multi-thread, le futur s'exécute » ──
    #[tokio::test(flavor = "multi_thread")]
    async fn multi_thread_runs_the_future() {
        let out = block_on(async { 42_i32 });
        assert_eq!(out.unwrap(), 42);
    }

    // ── Scenario « sans runtime, un runtime temporaire sert » ──
    #[test]
    fn without_runtime_a_temporary_runtime_serves() {
        let out = block_on(async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            "ok"
        });
        assert_eq!(out.unwrap(), "ok");
    }

    // ── Scenario « current_thread rend une erreur explicite » ──
    // Le futur n'est pas exécuté : verrouillé par un flag observable, pas par un
    // simple `is_err()`.
    #[tokio::test(flavor = "current_thread")]
    async fn current_thread_returns_explicit_error_and_never_runs_the_future() {
        let ran = Rc::new(RefCell::new(false));
        let ran_in_fut = Rc::clone(&ran);
        let res = block_on(async move {
            *ran_in_fut.borrow_mut() = true;
            1_i32
        });
        match res {
            Err(Error::Other(msg)) => {
                assert!(
                    msg.contains("requires a multi-thread tokio runtime"),
                    "message attendu explicite, obtenu : {msg}"
                );
            }
            other => panic!("Error::Other attendu, rendu : {other:?}"),
        }
        assert!(
            !*ran.borrow(),
            "le futur ne doit pas être exécuté sur current_thread"
        );
    }

    // ── Scenario « appel imbriqué » ──
    // Le block_on intérieur tourne dans le runtime temporaire (current_thread) du
    // block_on extérieur : il doit rendre l'erreur explicite, sans paniquer, et sans
    // exécuter son futur (flag observable).
    #[test]
    fn nested_call_from_the_temporary_runtime_returns_the_error() {
        let inner_ran = Rc::new(RefCell::new(false));
        let flag = Rc::clone(&inner_ran);
        let outer = block_on(async move {
            // contexte : runtime temporaire current_thread du block_on extérieur
            block_on(async move {
                *flag.borrow_mut() = true;
            })
        });
        let inner_res = outer.expect("l'extérieur doit s'exécuter sur le runtime temporaire");
        match inner_res {
            Err(Error::Other(msg)) => {
                assert!(
                    msg.contains("requires a multi-thread tokio runtime"),
                    "message attendu explicite, obtenu : {msg}"
                );
            }
            other => panic!("Error::Other attendu côté intérieur, rendu : {other:?}"),
        }
        assert!(
            !*inner_ran.borrow(),
            "le futur intérieur ne doit pas être exécuté"
        );
    }

    // ── Scenario « futur non Send » ──
    // Le `Rc` est vivant à travers le point d'attente : le futur du bloc async n'est
    // pas `Send`. Aucune borne `Send` n'étant exigée, cela compile et s'exécute.
    #[test]
    fn non_send_future_compiles_and_runs() {
        let data = Rc::new(String::from("rc-held-across-await"));
        let held_len = block_on(async move {
            tokio::time::sleep(std::time::Duration::ZERO).await;
            data.len() // `data` (Rc, non Send) traverse le point d'attente
        });
        assert_eq!(held_len.unwrap(), "rc-held-across-await".len());
    }
}
