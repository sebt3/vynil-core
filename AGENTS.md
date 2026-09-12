# AGENTS.md — vyvil-core

Before working on this project, read `.specdd/bootstrap.md`, then `.specdd/bootstrap.project.md`.

Assume the role, rules, workflow, and implementation constraints described in SpecDD. Treat
SpecDD specs as source-adjacent development contracts, not optional documentation. Adhere to
SpecDD rules unless explicitly instructed otherwise.

## Projet

Crate Rust générique (Rhai + Handlebars, handlers K8s/OCI/S3 optionnels), extraite du
workspace [vynil](https://github.com/sebt3/vynil) pour être réutilisable par d'autres projets
(kuberest, kydah, …). Publiée sur crates.io, API instable (< 1.0). Licence BSD 3-Clause.

## Flux de travail : SpecDD + test-first

`Spec approuvée → tests depuis les Scenario → implémentation minimale → validation → [x]`.
Détail complet dans `.specdd/bootstrap.project.md`. Règles clés :

- Une spec `.sdd` par fichier de `src/` (groupe 2–3 fichiers seulement si un seul contrat —
  un `*_mock.rs` avec sa source), décrivant tout le comportement du fichier.
- CI et harnais outil ont leurs propres specs : `.github/workflows/workflows.sdd`,
  `tooling.sdd`, spec racine `vyvil-core.sdd`.
- Jamais de tâche `[x]` sans synthèse du `validator` au vert.

## Harnais clippy

`Cargo.toml` `[lints]` porte le harnais (contractualisé par `tooling.sdd`) : `unsafe_code`
interdit, `missing_docs` warn, `pedantic` + `cargo` et la famille stricte
(`unwrap_used`, `expect_used`, `panic`, `unreachable`, `dbg_macro`, `todo`, `unimplemented`,
`print_stdout`, `print_stderr`, `arithmetic_side_effects`) en `deny` depuis la purge de dette.
Seul `multiple_crate_versions` (doublons de versions des dépendances amont) est `allow`.
En production : aucun
`unwrap`/`expect`/`panic!`/`todo!`/`unimplemented!`/`dbg!`/`println!`.

## Agents (`.opencode/agents/`)

| Agent | Rôle |
|---|---|
| `spec-dd` (primary) | Rédige les specs avec Sébastien, orchestre les sous-agents, ne code jamais directement |
| `spec-reverse` | Reverse-engineering : rédige la spec la plus complète possible d'un fichier source existant, sans jamais toucher le code |
| `implementer` | Réalise une spec précise : tests d'abord depuis les `Scenario`, puis implémentation minimale |
| `validator` | Vérifie implémentation ↔ spec + batterie tests/clippy/fmt, remonte une synthèse si ce n'est pas bon |

## Commandes

Avant toute `Tasks` `[x]`, la batterie complète (définie par `validator`, reprise ici) :

```bash
cargo test                                            # default
cargo test --all-features
cargo test --no-default-features --features k8s       # matrice de features
cargo test --no-default-features --features oci
cargo test --no-default-features --features s3
cargo test --no-default-features --features k8s,oci,s3
cargo clippy --all-features --all-targets -- -D warnings   # harnais (zéro warning toléré)
cargo +nightly fmt -- --check                         # voir rustfmt.toml
```

Contraintes de features à préserver (voir racine `vyvil-core.sdd`) : `k8s`/`oci`/`s3`/`http`
impliquent `rhai` (leurs types cœur sont des API Rhai directes) ; `hbs-scripting` doit rester
isolable de `hbs` — `hbs` seul ne doit jamais réintroduire `handlebars/script_helper` (donc
`rhai`/`smartstring`) dans le graphe — voir <https://github.com/sebt3/vynil-core/issues/9>.

## Git

- Pas de `Co-Authored-By` dans les messages de commit.
- Spec, code et tests dans la même PR ; tâches `[x]` uniquement après vérifications vertes.
