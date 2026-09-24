# Grenat — spécification v0.1 (brouillon)

> *La syntaxe de Ruby, la vitesse de Rust, les agents comme citoyens de première classe.*

Nom de travail : **Grenat** (pierre rouge, cousine du rubis). Extension : `.grn`. CLI : `grenat`.

---

## 1. Philosophie

1. **Ça se lit comme du Ruby** : `def … end`, blocs `do |x|`, `@ivars`, symboles, interpolation `"#{}"`, `unless`, `if` en suffixe, retour implicite.
2. **Ça tourne comme du Rust** : typage statique **inféré** (on n'écrit presque jamais de types dans le corps des fonctions), compilation native (Cranelift en dev, LLVM en release), aucun GC : **comptage de références Perceus** (comme Koka et Roc), avec réutilisation en place.
3. **Les agents sont des acteurs** : un agent est un processus léger isolé qui a une boîte aux lettres et qui est supervisé, comme en Erlang.
4. **Le LLM est un effet** : le compilateur sait quelles fonctions appellent un LLM, touchent le réseau ou le disque, ou demandent l'accord d'un humain.
5. **Tout ce qui sort d'un LLM est suspect** : c'est une donnée *teintée* `~T` qui ne peut pas atteindre un outil dangereux sans validation. La défense contre l'injection de prompt se fait **à la compilation**.
6. **Pas de couleur async** : pas de `async`/`await`. Tout est concurrent par défaut (fils verts M:N), comme en Go ou en Erlang.

### Ce qu'on retire de Ruby (le prix de la vitesse)

| Ruby | Grenat |
|---|---|
| Typage dynamique | Inférence Hindley-Milner + annotations aux frontières (`def`, `struct`) |
| `method_missing`, `send`, `eval`, `instance_eval` | ❌ remplacés par des **macros** évaluées à la compilation (v0.3) |
| Monkey-patching, classes ouvertes | ❌ extension seulement via `module` + `include` (traits statiques) |
| `nil` partout | Types optionnels explicites `String?`, `&.` et `||` conservés |
| Exceptions | Erreurs = valeurs (`Result`), avec le sucre `raise`/`rescue` et `?` |
| GC | Comptage de références déterministe, zéro pause |

Crystal a déjà montré qu'une syntaxe Ruby avec des types statiques et LLVM donne des performances de langage compilé. Grenat reprend cette voie et ajoute un runtime agentique et un système d'effets.

---

## 2. Les bases

```ruby
# Inférence : aucun type dans le corps
def fib(n: Int) -> Int
  return n if n < 2
  fib(n - 1) + fib(n - 2)
end

names = ["Ada", "Linus", "Matz"]            # Array(String) inféré
names.map { |n| n.upcase }.each { |n| puts n }

# Optionnels
def find_user(id: Int) -> User?
  users.find { |u| u.id == id }
end

email = find_user(42)&.email || "inconnu"
```

### Structs (valeurs), classes (références), modules (traits)

```ruby
struct Point
  x: Float
  y: Float

  def norm = Math.sqrt(x * x + y * y)      # méthode en une ligne
end

class Counter                              # référence, compteur RC
  @count: Int = 0
  def incr! = @count += 1
end

module Describable
  abstract def describe -> String          # méthode abstraite
  def shout = describe.upcase              # méthode par défaut
end

struct Invoice
  include Describable
  amount: Money
  def describe = "Facture de #{amount}"
end
```

### Enums algébriques et pattern matching

```ruby
enum Shape
  Circle(radius: Float)
  Rect(w: Float, h: Float)
end

def area(s: Shape) -> Float
  case s
  in Circle(r)  then 3.14159 * r * r
  in Rect(w, h) then w * h
  end                                      # exhaustivité vérifiée par le compilateur
end
```

### Erreurs

```ruby
def load_config(path: Path) -> Result(Config, IoError) uses fs.read
  text = File.read(path)?                  # ? propage l'erreur
  Config.parse(text)?
end

begin
  cfg = load_config("app.toml")?
rescue IoError => e
  warn "config absente : #{e.message}"
  cfg = Config.default
end
```

---

## 3. Effets et capacités

Chaque fonction a un ensemble d'**effets**. Ils sont **inférés** à l'intérieur d'un module et **déclarés** sur les fonctions publiques, les outils et `main`.

| Effet | Signification |
|---|---|
| `llm` | appelle un modèle (consomme du budget) |
| `net` / `net("api.github.com")` | réseau, éventuellement restreint à un hôte |
| `fs.read(path)` / `fs.write(path)` | disque, restreint à un préfixe de chemin |
| `shell` | processus externe, **toujours exécuté dans un bac à sable WASM** |
| `human` | attend une réponse humaine (approbation, saisie) |
| `time`, `random` | non-déterminisme (important pour les workflows durables) |

```ruby
def main uses llm, net, fs.read("./docs"), human
  # main est la racine des capacités : aucune fonction ne peut
  # faire plus que ce que main lui accorde.
end
```

Le compilateur **refuse** un appel dont l'effet n'est pas couvert par l'appelant. À l'exécution, les restrictions de chemin et d'hôte sont vérifiées une seconde fois (défense en profondeur).

---

## 4. Modèles et prompts typés

```ruby
model :fast,  provider: :anthropic, name: "claude-haiku-4-5",  temperature: 0.2
model :smart, provider: :anthropic, name: "claude-opus-5"
model :local, provider: :ollama,    name: "llama3.3"
```

Une **fonction `prompt`** est une fonction ordinaire dont l'implémentation est déléguée à un LLM. Son type de retour devient un **JSON Schema généré à la compilation**. La sortie est parsée, validée, et **relancée automatiquement** en cas de sortie mal formée.

```ruby
enum Sentiment
  Positive
  Neutral
  Negative
end

struct Summary
  title: String              ## Titre court, 8 mots maximum
  bullets: Array(String)     ## 3 à 5 points clés
  sentiment: Sentiment
end

## Résume un article de presse.
prompt summarize(article: String) -> ~Summary using :fast
  system "Tu es un analyste concis et factuel."
  user <<~P
    Résume cet article :
    #{article}
  P
end
```

Les commentaires `##` sont transmis au modèle : ils deviennent les descriptions du schéma et des outils. La doc du code est aussi le prompt.

### Le type teinté `~T`

`~Summary` signifie : *la structure est conforme, mais le contenu vient d'un LLM*.

- On peut **lire** un `~T` librement (l'afficher, le logguer, le passer à un autre prompt).
- On **ne peut pas** le passer à une fonction qui porte l'effet `shell`, `fs.write`, `net` ou `human`. Le compilateur l'interdit.
- Pour le « nettoyer », il faut le rendre explicite :

```ruby
s = summarize(article)

s.check { |x| x.bullets.size.between?(3, 5) }   # -> Result(Summary, CheckError)
s.approve(by: :human)                            # -> Summary (effet human)
s.trust!                                         # -> Summary (grep-able, signalé par le linter)
```

---

## 5. Outils

```ruby
## Lit un fichier texte du dossier de travail.
tool read_file(path: Path) -> String uses fs.read("./workspace")
  File.read(path)
end

## Exécute une commande dans un bac à sable (sans réseau).
tool run(cmd: String) -> Output uses shell
  Sandbox.exec(cmd, timeout: 30.s, net: false)
end

## Envoie un e-mail. Nécessite une approbation humaine.
tool send_email(to: Email, subject: String, body: String) -> Unit uses net("smtp.mail.com"), human
  approve! "Envoyer « #{subject} » à #{to} ?"
  Smtp.send(to:, subject:, body:)
end
```

Les arguments qu'un LLM passe à un outil arrivent **teintés**. Un `tool` est le seul endroit où Grenat accepte de les convertir en `T`, après une validation de schéma et une vérification des capacités. C'est la frontière de confiance.

---

## 6. Agents (acteurs)

```ruby
agent Researcher
  model :smart
  tools read_url, search_web, save_note
  budget tokens: 200_000, usd: 2.00, time: 10.min
  max_turns 30

  instructions <<~I
    Tu es un chercheur rigoureux. Cite toujours tes sources.
  I

  @notes: Array(Note) = []                 # état privé, jamais partagé

  on Research(topic: String) -> ~Report
    run "Enquête approfondie sur : #{topic}"
  end

  on AddNote(note: Note)
    @notes << note
  end
end
```

- `run` est la **boucle agentique intégrée** (LLM → outils → LLM …). Elle s'arrête quand le modèle produit le type de retour du handler (ici `Report`), ou quand le budget ou `max_turns` est épuisé.
- L'état `@…` est **isolé** : aucun autre agent ne peut y accéder. Les messages sont **déplacés** ou **gelés** (`Sendable`), donc pas de data race par construction.

```ruby
r = spawn Researcher

report = r.ask(Research(topic: "fusion nucléaire 2026"))    # attend la réponse
r.tell(AddNote(note: Note.new("à vérifier")))                # envoi sans attente

# Concurrence sans async/await
reports = topics.parallel_map(limit: 5) { |t| r.ask(Research(topic: t)) }

winner = race do
  a.ask(Solve(problem))
  b.ask(Solve(problem))
end                                        # le premier gagne, l'autre est annulé
```

### Budgets imbriqués

```ruby
within budget(usd: 1.00, time: 2.min) do
  reports = topics.parallel_map { |t| r.ask(Research(topic: t)) }
rescue BudgetExceeded => e
  warn "stoppé à #{e.spent}"
end
```

Un budget interne ne peut jamais dépasser le budget englobant. Le runtime **comptabilise chaque token** et coupe les appels en cours dès que la limite est atteinte.

### Supervision

```ruby
supervisor SupportTeam, strategy: :one_for_one, max_restarts: 3, within: 1.min
  child Triage
  child Researcher, count: 4               # pool de 4, répartition de charge
  child Writer
end
```

---

## 7. Workflows durables

Un `workflow` survit aux crashs, aux redéploiements et aux attentes humaines de plusieurs jours. Chaque `step` est **journalisé** (SQLite par défaut, Postgres en option). Au redémarrage, les steps terminés sont **rejoués depuis le journal** : aucun appel LLM n'est facturé deux fois.

```ruby
workflow publish_article(topic: String) -> Url
  report   = step(:research) { researcher.ask(Research(topic:)) }
  draft    = step(:draft)    { writer.ask(Draft(report:)) }
  approved = step(:review)   { draft.approve(by: :human, timeout: 3.days) }
  step(:publish) { Blog.publish(approved) }
end
```

**Règle vérifiée par le compilateur** : dans un `workflow`, tout effet non déterministe (`llm`, `net`, `time`, `random`, `human`) doit se trouver **à l'intérieur d'un `step`**. Le code hors step est donc rejouable à l'identique.

---

## 8. Tests et évaluations

```ruby
test "summarize respecte le format" do
  cassette "summaries/article_1" do        # enregistre puis rejoue (façon VCR)
    s = summarize(fixture("article_1.txt")).trust!
    assert s.bullets.size.between?(3, 5)
  end
end

test "l'agent refuse d'envoyer sans approbation" do
  mock :smart, replies: [call(:send_email, to: "x@y.z", subject: "hi", body: "…")]
  with_human(deny_all) do
    assert_raises ApprovalDenied { spawn(Mailer).ask(Handle(ticket)) }
  end
end

eval "qualité des résumés", dataset: "evals/articles.jsonl", threshold: 0.85 do |row|
  s = summarize(row.input)
  judge(:smart, "Le résumé est-il fidèle ?", s, row.input)   # LLM comme juge
end
```

`grenat test` est déterministe (cassettes et mocks). `grenat eval` appelle les vrais modèles et produit un rapport de score et de coût.

---

## 9. Architecture du compilateur (Rust)

```
grenat/
├── crates/
│   ├── grenat_lexer      # écrit à la main (modes : interpolation, heredocs) — tokens + commentaires
│   ├── grenat_ast        # arbre syntaxique typé, spans
│   ├── grenat_parser     # descente récursive + Pratt, récupération d'erreurs → AST
│   ├── grenat_hir        # résolution des noms, désucrage (blocs, &., ?, on/tool/prompt)
│   ├── grenat_types      # inférence HM bidirectionnelle + lignes d'effets + teinte ~T
│   ├── grenat_mir        # IR SSA, insertion RC Perceus, monomorphisation
│   ├── grenat_codegen    # Cranelift (dev, compile vite) → LLVM (release, exécute vite)
│   ├── grenat_runtime    # staticlib liée à chaque binaire :
│   │                     #   ordonnanceur M:N work-stealing, acteurs, supervision,
│   │                     #   clients LLM (Anthropic, OpenAI, Ollama), budgets,
│   │                     #   journal durable (SQLite), bac à sable wasmtime
│   ├── grenat_interp     # interpréteur HIR (phase 1, pour valider la sémantique)
│   └── grenat_cli        # grenat run | build | test | eval | fmt
└── std/                  # bibliothèque standard écrite en Grenat
```

Messages d'erreur visés : le niveau d'Elm et de Rust (crate `ariadne`).

```
error[E0412]: une valeur LLM teintée atteint un effet `shell`
  ┌─ agent.grn:14:9
  │
12│   plan = planner(goal)          # ~Plan
  │          ------------- produit ici par le modèle :fast
14│   run(plan.command)
  │       ^^^^^^^^^^^^ `run` exige `String`, reçu `~String`
  │
  = aide : validez d'abord avec `plan.check { … }` ou `plan.approve(by: :human)`
```

---

## 10. Distribution

Objectif : `brew install grenat` sur macOS, `yay -S grenat` sur Arch / Omarchy, sans aucune dépendance runtime autre que le linker système.

| Canal | Commande | Quand |
|---|---|---|
| Tap Homebrew (`mehdifarsi/homebrew-grenat`) | `brew install mehdifarsi/grenat/grenat` | dès la v0.1 |
| homebrew-core | `brew install grenat` | quand le projet est « notable » (~75 étoiles), release stable, build depuis les sources |
| AUR `grenat` (sources) et `grenat-bin` (précompilé) | `yay -S grenat` | dès la v0.1 |
| Dépôt Arch `extra` | `pacman -S grenat` | quand un packager Arch l'adopte |
| Installeur shell | `curl -fsSL https://grenat.dev/install.sh \| sh` | dès la v0.1 |

**Automatisation** : `dist` (ex cargo-dist) génère la GitHub Action déclenchée sur chaque tag `v*`. Elle compile les binaires macOS arm64/x86_64 et Linux x86_64/aarch64, crée la GitHub Release, met à jour la formule du tap et l'installeur shell. Le `PKGBUILD` de `grenat-bin` pointe vers ces mêmes artefacts.

**Contraintes de conception qui en découlent** :

| Contrainte | Décision |
|---|---|
| Grenat est un compilateur qui lie un runtime | `libgrenat_runtime.a` et `std/` sont cherchés **relativement à l'exécutable** (`<prefix>/bin/grenat` → `<prefix>/lib/grenat/`, `<prefix>/share/grenat/std/`), avec la surcharge `GRENAT_HOME`. Ça marche sous `/opt/homebrew`, `/usr` et `~/.cargo` |
| Linker | `cc` du système (Xcode CLT sur macOS, `gcc` sur Arch), seule dépendance runtime |
| Pas de dépendances système | `rustls` (pas d'OpenSSL), SQLite embarqué (`rusqlite`, feature `bundled`) |
| LLVM est lourd (~100 Mo) | **Cranelift par défaut**, embarqué et pur Rust. LLVM en feature optionnelle |
| Licence | MIT OR Apache-2.0 dès le premier commit |

Arborescence installée :

```
<prefix>/bin/grenat
<prefix>/lib/grenat/libgrenat_runtime.a
<prefix>/share/grenat/std/…
```

---

## 11. Feuille de route

| Phase | Contenu | Résultat |
|---|---|---|
| **0** ✅ | Lexer + parser + AST + `grenat check/parse/tokens` | on parse tous les exemples et tous les blocs de cette spec |
| **0.5** | `grenat fmt` (conservation des commentaires) | formateur canonique |
| **1** ✅ | Interpréteur, `prompt`, `tool`, agents, budgets, teinte, client Anthropic, `grenat run/test` | premier agent qui tourne |
| **2** | Inférence de types + effets + teinte `~T` **à la compilation** | les erreurs de sécurité avant l'exécution |
| **3** | Runtime d'acteurs (tokio), agents concurrents, `parallel_map`/`race` réels, supervision | multi-agents |
| **4** | Codegen Cranelift + RC Perceus | binaires natifs rapides |
| **5** | Workflows durables (journal des `step`), cassettes, `mock`, `eval` | prêt pour la production |
| **6** | LSP, LLVM release, macros, gestionnaire de paquets | écosystème |

### État de la phase 1

L'interpréteur exécute directement l'AST. Ce qui marche :

- le langage de base : fonctions, blocs et fermetures, `struct`, `class`, `module`/`include`, `enum`, `case/in`, `Result` et `?`, `rescue`/`ensure`, bibliothèque de base (`Array`, `Hash`, `String`, `File`, `Dir`, `Math`, `Json`, `Env`) ;
- `prompt` : le type de retour devient un JSON Schema (sortie structurée), les `##` deviennent les descriptions, la réponse est validée, relancée une fois si invalide, et renvoyée **teintée** ;
- `agent` : `spawn`, `ask`/`tell`, état `@…`, boucle `run` avec les `tool` déclarés et un outil `final_answer` typé par le retour du handler ;
- teinte `~T` : propagée par les accès, l'interpolation, les opérateurs et les blocs ; `TaintError` si elle atteint une fonction à effet `net`, `shell`, `fs.write` ou `human` ; `.check`, `.approve(by: :human)`, `.trust!` ;
- budgets `usd`/`tokens`/`time` (`within budget(…)`, directive `budget` des agents), coût calculé par modèle ;
- `Runtime.on_approval`, `approve!`, `grenat test` avec `assert`, `assert_equal`, `assert_raises`.

Simplifications provisoires, levées dans les phases suivantes :

| Aujourd'hui | Plus tard |
|---|---|
| Effets et teinte vérifiés **à l'exécution** | à la compilation (phase 2) |
| Agents exécutés de façon synchrone ; `parallel_map`, `race`, `spawn_pool` séquentiels | concurrence réelle (phase 3) |
| Superviseur : démarrage paresseux des enfants, pas de redémarrage | stratégies de supervision (phase 3) |
| `step` exécute son bloc sans journal | journal durable (phase 5) |
| Capacités `fs.read("./docs")`, `net("hôte")` non restreintes | vérifiées par le runtime (phase 2) |

Pour les modèles qui le recommandent (`claude-opus-5`, `claude-fable-5-1`), le client active le repli côté serveur (`fallbacks: "default"`) : une requête refusée par un classifieur est rejouée sur un autre modèle au lieu d'échouer. On le désactive avec `model :x, …, fallbacks: false`.

Objectif de performance : pour le code CPU, rester **entre 1× et 2× Rust**, comme Crystal ou Swift. Côté agents, supporter **100 000 agents concurrents** sur une seule machine (un acteur au repos ≈ 2 Ko).
