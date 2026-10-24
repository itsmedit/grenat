# Grenat — records, migrations and search by meaning (`database`, `embed`, `nearest`)

Read this when a program keeps data in its own database (records, migrations) or searches texts by meaning (embeddings, vectors). It assumes the core reference, `llms.txt` (`Db.connect` for raw SQL is there).

```ruby
database Env.fetch("DATABASE_URL", "sqlite://desk.db")

struct Ticket
  table :tickets
  id: Int?
  subject: String
  status: String = "open"
end

migration "001_create_tickets" do |db|
  db.migrate("CREATE TABLE tickets (id #{db.primary_key}, subject TEXT NOT NULL, status TEXT NOT NULL)")
end

def close(id: Int) -> Bool uses db
  ticket = Ticket.find(id)  # Ticket?
  return false if ticket.nil?
  ticket.with(status: "closed").save
  true
end
```

- `database Env.fetch("DATABASE_URL")` declares the application's database (SQLite or PostgreSQL; a top-level `database` needs no `net`). A struct with `table :tickets` and `id: Int?` is a record: it gets `create`, `find` (a `T?`), `where(field: v)` (an `Array(T)`), `all`, `count`, `save` (insert without an id, update with it), `delete`, `with(field: v)`.
- Reads are `db.read`, writes `db.write` (`db` = both); a record written with an untrusted value is refused (E0412): check it first.
- `migration "001_x" do |db| db.migrate("CREATE TABLE …") end`, applied by `grenat migrate`, in order. `db.primary_key` is the id column's type on SQLite and PostgreSQL alike.
- In tests, each test gets a new in-memory SQLite database with every migration applied.

## Search by meaning

`embed(:docs, texts)` needs an embedding model (`kind: embedding` in `config/models.yml`, here `docs`); a record field `embedding: Vector(1024)` keeps a vector, `Doc.nearest(:embedding, vector, limit: 4)` finds the closest.

```ruby
struct Passage
  table :passages
  id: Int?
  text: String
  embedding: Vector(1024)
end

migration "002_create_passages" do |db|
  db.migrate("CREATE TABLE passages (id #{db.primary_key}, text TEXT NOT NULL, embedding #{db.vector(1024)} NOT NULL)")
end

def index(parts: Array(String)) uses llm, db
  vectors = embed(:docs, parts)  # Array(Array(Float)): one request for many texts
  parts.each_with_index { |text, i| Passage.create(text:, embedding: vectors[i]) }
end

def search(question: String) -> Array(Passage) uses llm, db.read
  Passage.nearest(:embedding, embed(:docs, question), limit: 4)  # embed of one text: Array(Float)
end
```

`nearest` also takes `where: {…}` and `max_distance:` (cosine distance, 0 the same direction).

## Tests

`mock_embed :docs` makes vectors from the texts' words, so texts sharing words are near.

```ruby
test "a ticket is closed" do
  t = Ticket.create(subject: "Bug")
  assert close(t.id || 0)
  assert_equal "closed", Ticket.find(t.id || 0)&.status
  assert_equal 1, Ticket.where(status: "closed").size
end

test "the passage about refunds is found" do
  mock_embed :docs
  index(["A refund is asked for within 30 days.", "Invoices export to CSV."])
  assert_equal "A refund is asked for within 30 days.", search("How do I get a refund?").first&.text
end
```
