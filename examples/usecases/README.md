# The ten agent use cases — phase 7 acceptance

Ten realistic, nominal programs, one per kind of agent. They are what
phase 7 is measured against: each one checks and passes its tests today
(`grenat test examples/usecases/02_code_review_test.grn`), with the model
mocked, but the I/O Grenat's standard library lacks is faked, between
`═══ STUBS ═══` and `═══ END STUBS ═══`. **Phase 7 is done when every stub
block is gone** and each program runs against real services.

| # | Program | Lines | of which stubs | Real today? | Still needed |
|---|---|---:|---:|---|---|
| 1 | [`support_desk.grn`](../support_desk.grn) (customer support) | 140 | 0 | **yes** | — |
| 2 | [`02_code_review.grn`](02_code_review.grn) (GitHub API and webhook) | 77 | 0 | **yes** | — |
| 3 | [`03_research.grn`](03_research.grn) (Brave Search API) | 56 | 0 | **yes** | HTML to text |
| 4 | [`04_data.grn`](04_data.grn) (question → SQL, Postgres or SQLite) | 27 | 0 | **yes** | — |
| 5 | [`05_documents.grn`](05_documents.grn) (PDF invoices, one batch) | 33 | 0 | **yes** | — |
| 6 | [`06_weekly_digest.grn`](06_weekly_digest.grn) (GitHub releases, every Monday, email) | 44 | 0 | **yes** | — |
| 7 | [`07_sre.grn`](07_sre.grn) (Loki, Prometheus, kubectl) | 61 | 0 | **yes** | — |
| 8 | [`08_chat.grn`](08_chat.grn) (a Conversation, saved between sessions) | 11 | 0 | yes (terminal) | a chat connector |
| 9 | [`09_team.grn`](09_team.grn) (planner, writers, critic) | 40 | 0 | yes | — |
| 10 | [`10_mcp_tools.grn`](10_mcp_tools.grn) (Linear, Notion through MCP) | 27 | 0 | **yes** | — |
| 11 | [`11_knowledge_base.grn`](11_knowledge_base.grn) (documentation embedded, searched by meaning) | 51 | 0 | **yes** | — |
| 12 | [`12_meeting_minutes.grn`](12_meeting_minutes.grn) (a recording transcribed, then minutes and tasks) | 41 | 0 | **yes** | — |
| 13 | [`13_support_inbox.grn`](13_support_inbox.grn) (a support mailbox read over IMAP, emails triaged with their attachments) | 36 | 0 | **yes** | — |

Case 11 came with phase 13 (embeddings and vector search): where the
support desk's `search_docs` counts the words a page shares with a query,
it finds passages by meaning; its tests fake the vectors (`mock_embed`).
Case 12 came with audio: a meeting's recording, a file or a link, is
transcribed (`transcribe`), then a prompt writes its minutes and tasks; its
tests fake the transcript (`mock_transcribe`) and the download (`mock_http`).
Case 13 came with email in (`on_email`): `grenat serve` reads the support
mailbox (Gmail, Microsoft 365, any IMAP server), a prompt triages each new
email with its PDFs and images, an urgent one is told to its team, and the
message is moved to a folder; its tests hand it emails (`deliver_email`).

Lines of code, without blank lines and comments. A stub is shorter than
the real code it stands for: with `Http`, cases 2 and 3 grew from 47 to 70
and from 45 to 56 lines (authentication, JSON, query strings). The tests
stub the services (`mock_http`, `mock_shell`, `mock_mcp`) and need no
variable set: tokens come from `Credentials` (stand-in secrets in tests),
and the environment a test reads is the one `mock_env` gives.
