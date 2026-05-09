# zql

A fast, native SQL editor built in Rust using [GPUI](https://www.gpui.rs) — the same GPU-accelerated UI framework that powers the Zed editor.

## Install

```bash
cargo install --git https://github.com/fhsgoncalves/zql
```

Requires [Rust](https://rustup.rs) and a working C++ toolchain.

## What is zql?

zql is a desktop SQL editor with a focus on performance and simplicity. It is written entirely in Rust, renders via GPU, and avoids the memory bloat common to Electron-based or JVM-based database tools.

- **No garbage collector** — predictable memory usage and no runtime pauses.
- **Native GPU rendering** — smooth scrolling and large result sets via GPUI.
- **Small footprint** — cold starts in under 5ms and a memory footprint under 20MB.
- **Minimal UI** — no 200-option settings menus; write queries, run them, see results.

## Features

| Feature | Status |
|---------|--------|
| SQL editor with syntax highlighting | ✅ |
| Query execution with result grid | ✅ |
| Connection panel with live schema tree | ✅ |
| Tabbed query files | ✅ |
| PostgreSQL driver | ✅ |
| Autocompletion | ✅ |
| Highlight active query selection | ✅ |

## Supported Databases

- **PostgreSQL** — fully supported via `tokio-postgres` and `rustls`.
- **MySQL** — planned.
- **SQLite** — planned.

## Roadmap

- [ ] Refactor connection panel layout (IntelliJ-style)
- [ ] Loading indicator on bottom bar, when connecting or running a query
- [ ] Autosave when window or panel focus is lost
- [ ] Improve autocompletion reliability
- [ ] Export results to more formats
- [ ] Generate DDL for tables, functions, indexes, and triggers from the connections panel
- [ ] Copy selected content as CSV / JSON
- [ ] Show type information on data table columns
- [ ] Editor toolbar, allowing run the query via button
- [ ] Cycle between open tabs
- [ ] File search
- [ ] Find text (using fuzzy search)
- [ ] Allow in-place editing in the data table

## Tech Stack

- **Rust** — core application logic
- **GPUI** — GPU-accelerated UI framework (from Zed)
- **gpui-component** — higher-level UI components
- **tree-sitter-sql** — syntax highlighting
- **tokio / tokio-postgres** — async PostgreSQL driver
- **rustls** — TLS for database connections

## License

MIT — see [LICENSE](./LICENSE).
