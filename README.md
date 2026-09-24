# Grenat

> La syntaxe de Ruby, la vitesse de Rust, les agents comme citoyens de première classe.

Grenat est un langage de programmation compilé pour construire des systèmes d'agents IA :
prompts typés, outils, agents-acteurs supervisés, budgets, workflows durables, et un
système d'effets qui fait de l'injection de prompt une **erreur de compilation**.

```ruby
prompt summarize(article: String) -> ~Summary using :fast
  user "Résume : #{article}"
end

agent Researcher
  model :smart
  tools search_web, read_url
  budget usd: 2.00, time: 10.min

  on Research(topic: String) -> ~Report
    run "Enquête sur #{topic}"
  end
end
```

- Spécification : [`SPEC.md`](SPEC.md)
- Exemples : [`bases.grn`](examples/bases.grn), [`explorateur.grn`](examples/explorateur.grn) (agent réel), [`support_desk.grn`](examples/support_desk.grn) (multi-agents, approbation humaine)

## État

**Phase 3 — agents concurrents** : les agents sont des acteurs (un message à la fois,
interblocages détectés, supervision avec redémarrage), `parallel_map` et `race` sont
réellement parallèles. Avant d'exécuter quoi que ce soit, `grenat` vérifie les noms, les
types, les effets et la teinte : une réponse de modèle non validée qui part vers le réseau
est une **erreur de compilation**.

```sh
cargo build
target/debug/grenat run examples/bases.grn            # le langage de base, sans LLM

export ANTHROPIC_API_KEY=sk-ant-…
target/debug/grenat run --log examples/explorateur.grn crates/grenat_parser    # un vrai agent
target/debug/grenat run examples/support_desk.grn examples/tickets.jsonl       # multi-agents + approbation

target/debug/grenat check examples/*.grn     # noms, types, effets, teinte
target/debug/grenat test mon_fichier.grn     # blocs `test "…" do … end`
cargo test                                   # ~140 tests : unitaires, intégration, CLI, client HTTP
```

## Organisation

| Crate | Rôle |
|---|---|
| `grenat_lexer` | tokens, interpolation, heredocs, commentaires `##` |
| `grenat_ast` | arbre syntaxique |
| `grenat_parser` | descente récursive + Pratt, diagnostics avec récupération |
| `grenat_llm` | client de l'API Claude (sortie structurée, outils, repli), fournisseur scripté pour les tests |
| `grenat_types` | vérificateur : noms, types, effets, teinte `~T` (E0100–E0500) |
| `grenat_interp` | interpréteur : valeurs, évaluation, prompts, agents, budgets, teinte, capacités |
| `grenat_cli` | binaire `grenat` |

Deux dépendances externes seulement : `ureq` (HTTP + rustls) et `serde_json`.

## Licence

Au choix : [MIT](LICENSE-MIT) ou [Apache 2.0](LICENSE-APACHE).
