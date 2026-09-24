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
- Exemple complet : [`examples/support_desk.grn`](examples/support_desk.grn)

## État

**Phase 0 — syntaxe** : lexer, parser et AST complets pour toute la grammaire de la spec.
L'exécution arrive en phase 1 (voir la feuille de route dans la spec).

```sh
cargo build
target/debug/grenat check examples/*.grn     # vérifie la syntaxe
target/debug/grenat parse  examples/support_desk.grn   # affiche l'AST
target/debug/grenat tokens examples/support_desk.grn   # affiche les tokens
cargo test
```

## Organisation

| Crate | Rôle |
|---|---|
| `grenat_lexer` | tokens, interpolation, heredocs, commentaires `##` |
| `grenat_ast` | arbre syntaxique |
| `grenat_parser` | descente récursive + Pratt, diagnostics avec récupération |
| `grenat_cli` | binaire `grenat` |

Aucune dépendance externe pour l'instant.

## Licence

Au choix : [MIT](LICENSE-MIT) ou [Apache 2.0](LICENSE-APACHE).
