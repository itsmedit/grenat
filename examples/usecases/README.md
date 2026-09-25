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
| 5 | [`05_documents.grn`](05_documents.grn) (PDF invoices) | 33 | 0 | **yes** | the Batch API (half the price) |
| 6 | [`06_weekly_digest.grn`](06_weekly_digest.grn) (GitHub releases, every Monday, email) | 44 | 0 | **yes** | — |
| 7 | [`07_sre.grn`](07_sre.grn) (Loki, Prometheus, kubectl) | 61 | 0 | **yes** | — |
| 8 | [`08_chat.grn`](08_chat.grn) (memory) | 39 | 0 | yes (terminal) | a chat connector |
| 9 | [`09_team.grn`](09_team.grn) (planner, writers, critic) | 40 | 0 | yes | — |
| 10 | [`10_mcp_tools.grn`](10_mcp_tools.grn) (Linear, Notion through MCP) | 27 | 0 | **yes** | — |

Lines of code, without blank lines and comments. A stub is shorter than
the real code it stands for: with `Http`, cases 2 and 3 grew from 47 to 70
and from 45 to 56 lines (authentication, JSON, query strings). The tests
stub the services (`mock_http`, `mock_shell`, `mock_mcp`) and need
`GITHUB_TOKEN`, `GITHUB_WEBHOOK_SECRET`, `BRAVE_API_KEY`, `LINEAR_TOKEN`,
`NOTION_TOKEN` and `SMTP_URL` set, to any value.
