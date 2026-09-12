# SpecDD project specific overrides

## Flux de développement : SpecDD + test-first (TTD)

Unité de travail : une spec `.sdd`. Règle absolue : pas de code sans spec approuvée par
Sébastien, pas d'implémentation avant les tests dérivés de la spec.

1. **Spec** — la spec de la cible existe et est revue : rédigée en session avec l'agent
   `spec-dd`, ou produite en reverse-engineering par `spec-reverse` puis relue
   attentivement par Sébastien (le comportement accidentel du code ne devient un contrat
   qu'après cette relecture).
2. **Test d'abord** — l'agent `implementer` convertit d'abord chaque `Scenario` de la spec
   en test, et les fait compiler et échouer. Il ne touche pas encore au comportement de
   production.
3. **Implémentation** — le minimum de code qui fait passer les tests, strictement dans le
   périmètre `Owns` / `Can modify` de la spec.
4. **Validation** — l'agent `validator` relance la batterie complète (features, harnais
   clippy, fmt) et produit une synthèse.
5. **Clôture** — les `Tasks` passent à `[x]` seulement après synthèse verte ; spec, code et
   tests avancent ensemble, sans `[x]` décoratif.

## Règle : une spec par fichier source

- Un fichier de `src/` = une spec `.sdd` du même nom dans le même répertoire
  (`src/foo.rs` ↔ `src/foo.sdd`), couvrant **tout** son comportement observable :
  `Exposes`, `Accepts`, `Returns`, `Raises`, `Handles`, `Must`, `Must not`, `Scenario`.
- Regrouper 2–3 fichiers dans une spec n'est admis que si ils forment un seul contrat :
  cas visé, un `*_mock.rs` vit dans la spec de sa source (`src/http.sdd` owns `src/http.rs`
  + `src/http_mock.rs`, idem k8s/oci/s3) — le mock n'a pas de contrat indépendant du
  contrat mocké. La justification du regroupement va dans `Purpose` de la spec.
- Hors `src/` : la CI a sa spec (`/.github/workflows/workflows.sdd`), le harnais de toolchain
  a la sienne (`/tooling.sdd` : rustfmt + lints clippy/rustc).
- Une spec sans fichier source correspondant est légitime pour les artefacts de
  configuration ; l'inverse (source sans spec) est interdit d'amendement direct : on crée la
  spec d'abord.

## Harnais clippy (guidage des agents)

- La source de vérité du harnais est `Cargo.toml` `[lints]`, contractualisée par `/tooling.sdd` :
  `unsafe_code` interdit, `missing_docs` warn, `pedantic`, `cargo`, et la famille stricte
  (`unwrap_used`, `expect_used`, `panic`, `unreachable`, `dbg_macro`, `todo`, `unimplemented`,
  `print_stdout`, `print_stderr`, `arithmetic_side_effects`).
- Dans tout fichier touché par une tâche : nouveau et nouveau code à **zéro warning** clippy
  (strict + pedantic) et `missing_docs`. La dette préexistante ailleurs est tracée dans
  `/tooling.sdd`, ne doit pas grossir, et se purge module par module.
- Code de production : jamais `unwrap()` / `expect()` / `panic!` / `todo!` / `unimplemented!` /
  `dbg!` / `println!` — propager `crate::Error` ou un `RhaiRes`.
- `#[allow(...)]` des lints du harnais : uniquement sous `cfg(test)` (exemption globale portée
  par `src/lib.rs`) ou avec commentaire de justification d'une ligne citant la spec.

## Rôle de Sébastien

Sébastien est la source de vérité sur l'intention. Toute ambiguïté de contrat, de sécurité,
de frontière ou de permission d'édition : on s'arrête et on demande, on ne suppose pas.
