//! Glob matching helper for Rhai (`glob` function wrapping `wildmatch`).

use rhai::{Engine, ImmutableString};
use wildmatch::WildMatch;

// signature imposée par l'API Rhai (vyvil-core.sdd)
#[allow(clippy::needless_pass_by_value)]
fn glob_fn(text: ImmutableString, pattern: ImmutableString) -> bool {
    WildMatch::new(pattern.as_ref()).matches(text.as_ref())
}

/// Register the `glob(text, pattern) -> bool` Rhai helper.
pub fn glob_rhai_register(engine: &mut Engine) {
    engine.register_fn("glob", glob_fn);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhai::EvalAltResult;

    /// Engine préparé par la fonction d'enregistrement — le contrat du module
    /// (`glob.sdd`) est `glob_rhai_register`, jamais l'appel direct à `glob_fn`.
    fn engine_with_glob() -> Engine {
        let mut engine = Engine::new();
        glob_rhai_register(&mut engine);
        engine
    }

    // ── correspondance littérale et ancrage ──────────────────────────────────

    #[test]
    fn test_glob_literal_exact_match() {
        // Scenario: correspondance littérale exacte
        let engine = engine_with_glob();
        assert!(engine.eval::<bool>(r#"glob("cat", "cat")"#).unwrap());
    }

    #[test]
    fn test_glob_literal_mismatch() {
        // Scenario: désaccord en littéral
        let engine = engine_with_glob();
        assert!(!engine.eval::<bool>(r#"glob("dog", "cat")"#).unwrap());
    }

    #[test]
    fn test_glob_pattern_must_cover_whole_text() {
        // Scenario: motif non couvrant rejeté — le motif est ancré
        let engine = engine_with_glob();
        assert!(!engine.eval::<bool>(r#"glob("category", "cat")"#).unwrap());
    }

    // ── `*` et `?` ───────────────────────────────────────────────────────────

    #[test]
    fn test_glob_star_swallows_slashes() {
        // Scenario: l'étoile avale les slashes — `/` n'est pas spécial
        let engine = engine_with_glob();
        assert!(engine.eval::<bool>(r#"glob("a/b/c.txt", "*.txt")"#).unwrap());
    }

    #[test]
    fn test_glob_star_accepts_empty_sequence() {
        // Scenario: l'étoile accepte la suite vide
        let engine = engine_with_glob();
        assert!(engine.eval::<bool>(r#"glob(".txt", "*.txt")"#).unwrap());
    }

    #[test]
    fn test_glob_question_matches_one_char() {
        // Scenario: le point d'interrogation pour un caractère
        let engine = engine_with_glob();
        assert!(engine.eval::<bool>(r#"glob("cat", "c?t")"#).unwrap());
    }

    #[test]
    fn test_glob_question_rejects_zero_or_two_chars() {
        // Scenario: le point d'interrogation refuse zéro ou deux caractères
        let engine = engine_with_glob();
        // And: zéro caractère
        assert!(!engine.eval::<bool>(r#"glob("ct", "c?t")"#).unwrap());
        // And: deux caractères
        assert!(!engine.eval::<bool>(r#"glob("coat", "c?t")"#).unwrap());
    }

    // ── ordre des arguments (contrat) ────────────────────────────────────────

    #[test]
    fn test_glob_argument_order_not_reversible() {
        // Scenario: ordre des arguments non inversible — texte d'abord, motif ensuite
        let engine = engine_with_glob();
        // Then: texte `*.txt`, motif `a.txt` → faux
        assert!(!engine.eval::<bool>(r#"glob("*.txt", "a.txt")"#).unwrap());
        // And: le sens contractuel inverse → vrai
        assert!(engine.eval::<bool>(r#"glob("a.txt", "*.txt")"#).unwrap());
    }

    // ── littéralité stricte : crochets, backslash, casse ─────────────────────

    #[test]
    fn test_glob_brackets_not_a_char_class() {
        // Scenario: crochet n'est pas une classe de caractères
        let engine = engine_with_glob();
        // Then: `[abc]` n'est pas la classe {a,b,c}
        assert!(!engine.eval::<bool>(r#"glob("b", "[abc]")"#).unwrap());
        // And: `[abc]` n'est que le littéral exact
        assert!(engine.eval::<bool>(r#"glob("[abc]", "[abc]")"#).unwrap());
    }

    #[test]
    fn test_glob_backslash_stays_literal() {
        // Scenario: le backslash reste littéral — un seul backslash réel dans le texte
        //
        // Double lecture d'échappement vérifiée : le source Rust brut
        // `r#"glob("\\abc", "\\*")"#` transmet `glob("\\abc", "\\*")` au script rhai,
        // et le lexer rhai débouble `\\` en UN backslash littéral. Le garde `.len()`
        // ci-dessous le prouve : `"\\abc"` fait 4 caractères (`\abc`) côté script,
        // pas 5 (`\\abc`). Le motif devient donc `\*` = backslash littéral + étoile
        // (aucun caractère d'échappement chez wildmatch, `glob.sdd`).
        let engine = engine_with_glob();
        // Garde : vérifier la voie d'échappement (4 = un seul backslash réel)
        assert_eq!(
            engine.eval::<i64>(r#""\\abc".len()"#).unwrap(),
            4,
            "le lexer rhai doit debuffer \\abc en un seul backslash reel"
        );
        // Then: texte `\abc`, motif `\*` → vrai
        assert!(engine.eval::<bool>(r#"glob("\\abc", "\\*")"#).unwrap());
        // But: sans le backslash, le motif littéral `\*` ne couvre plus `abc`
        assert!(!engine.eval::<bool>(r#"glob("abc", "\\*")"#).unwrap());
    }

    #[test]
    fn test_glob_case_sensitive() {
        // Scenario: comparaison sensible à la casse
        let engine = engine_with_glob();
        assert!(!engine.eval::<bool>(r#"glob("Cat", "cat")"#).unwrap());
    }

    // ── vides et aberrants : jamais d'exception ──────────────────────────────

    #[test]
    fn test_glob_empty_texts_and_patterns() {
        // Scenario: textes et motifs vides
        let engine = engine_with_glob();
        // Then: vide contre vide
        assert!(engine.eval::<bool>(r#"glob("", "")"#).unwrap());
        // And: texte non vide contre motif vide
        assert!(!engine.eval::<bool>(r#"glob("a", "")"#).unwrap());
        // And: `*` réduit le texte vide à rien
        assert!(engine.eval::<bool>(r#"glob("", "*")"#).unwrap());
    }

    #[test]
    fn test_glob_absurd_pattern_no_error() {
        // Scenario: motif aberrant sans erreur — renvoie un bool, ne lève jamais
        let engine = engine_with_glob();
        // Then: eval::<bool> réussi (Ok) EST l'absence d'exception
        assert!(!engine.eval::<bool>(r#"glob("x", "][")"#).unwrap());
        // And: le motif aberrant correspond à son propre littéral
        assert!(engine.eval::<bool>(r#"glob("][", "][")"#).unwrap());
    }

    // ── enregistrement ───────────────────────────────────────────────────────

    #[test]
    fn test_glob_register_on_fresh_engine() {
        // Scenario: enregistrement sur un engine nu
        let mut engine = Engine::new();
        glob_rhai_register(&mut engine);
        assert!(engine.eval::<bool>(r#"glob("a", "a")"#).unwrap());
    }

    #[test]
    fn test_glob_helper_absent_before_register() {
        // Scenario: helper absent tant que non enregistré
        let mut engine = Engine::new();
        let err = engine
            .eval::<bool>(r#"glob("a", "a")"#)
            .expect_err("glob ne doit pas exister avant enregistrement");
        // Then: erreur rhai de fonction inconnue (résolution d'appel, pas glob_fn)
        assert!(
            matches!(&*err, EvalAltResult::ErrorFunctionNotFound(sig, _) if sig.starts_with("glob")),
            "attendu ErrorFunctionNotFound sur glob, obtenu: {err}"
        );
        // And: l'absence vient du non-enregistrement, pas de glob_fn qui ne lève
        // jamais — sur le même engine, après enregistrement, l'évaluation réussit
        glob_rhai_register(&mut engine);
        assert!(engine.eval::<bool>(r#"glob("a", "a")"#).unwrap());
    }

    #[test]
    fn test_glob_new_bare_injects_helper() {
        // Scenario: new_bare injecte le helper — l'unique point d'appel de
        // glob_rhai_register (engine.rs:198). Verrouillé ici plutôt que dans
        // engine.rs pour garder le test au plus près de glob.sdd ; l'import
        // crate::engine::Script est propre sous la porte rhai, seule requise.
        let script = crate::engine::Script::new_bare(vec![]);
        assert!(script.engine.eval::<bool>(r#"glob("f.txt", "*.txt")"#).unwrap());
    }
}
