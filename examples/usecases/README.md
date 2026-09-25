# The ten agent use cases — phase 7 acceptance

Ten realistic, nominal programs, one per kind of agent. They are what
phase 7 is measured against: each one checks and passes its tests today
(`grenat test examples/usecases/02_code_review_test.grn`), with the model
mocked, but the I/O Grenat's standard library lacks is faked, between
`═══ STUBS ═══` and `═══ END STUBS ═══`. **Phase 7 is done when every stub
block is gone** and each program runs against real services.

| # | Program | Logic | Stubs | Real today? | Needed |
|---|---|---:|---:|---|---|
| 1 | [`support_desk.grn`](../support_desk.grn) (customer support) | 113 | 19 | no | doc search, email |
| 2 | [`02_code_review.grn`](02_code_review.grn) | 40 | 7 | no | `Http` (GitHub), webhooks, sandboxed shell |
| 3 | [`03_research.grn`](03_research.grn) | 41 | 4 | no | web search and fetch |
| 4 | [`04_data.grn`](04_data.grn) (question → SQL) | 24 | 4 | no | `Db` (Postgres, SQLite) |
| 5 | [`05_documents.grn`](05_documents.grn) (invoices) | 33 | 3 | text only | PDF and image input, Batch API |
| 6 | [`06_weekly_digest.grn`](06_weekly_digest.grn) | 30 | 8 | partly | `Http` (feeds), email, `every` |
| 7 | [`07_sre.grn`](07_sre.grn) (alert → fix) | 47 | 10 | no | `Http`, sandboxed shell, tool timeouts |
| 8 | [`08_chat.grn`](08_chat.grn) (memory) | 39 | 0 | yes (terminal) | a chat connector |
| 9 | [`09_team.grn`](09_team.grn) (planner, writers, critic) | 40 | 0 | yes | — |
| 10 | [`10_mcp_tools.grn`](10_mcp_tools.grn) (Linear, Notion) | 25 | 25 | no | an MCP client |

Lines of code, without blank lines and comments. The stubs are shorter
than the real code they stand for (authentication, pagination), which
Grenat cannot express today: there is no network, database or process
primitive yet.
