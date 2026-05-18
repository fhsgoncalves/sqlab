# sq/lab

A fast, native SQL editor built in Rust using [GPUI](https://www.gpui.rs) — the same GPU-accelerated UI framework that powers the Zed editor.

> **Note:** This is a hobby project and it is actively being built using **Spec-Driven Development** powered by coding agents.

The long-term goal is to provide a fully open-source alternative to DBeaver and DataGrip, supporting any databases with great performance and awesome experience.

## Install

```bash
cargo install --git https://github.com/fhsgoncalves/sqlab
```

Requires [Rust](https://rustup.rs) and a working C++ toolchain.

## What is sq/lab?

sq/lab is a desktop SQL editor with a focus on performance and simplicity. It is written entirely in Rust, renders via GPU, and avoids the memory bloat common to Electron-based or JVM-based database tools.

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
| Autocompletion on steroids | ✅ |
| Highlight active query selection | ✅ |
| Passwords stored securely | ✅ |
| Auto save on focus lost | ✅ |
| Terminal panel (supports coding agents) | ✅ |
| File search | ✅ |

## Supported Databases

- **PostgreSQL** — fully supported via `tokio-postgres` and `rustls`.
- **MySQL** — planned / coming soon.
- **SQLite** — planned / coming soon.
- **DuckDB** — planned / coming soon.
- **Databend** — planned / coming soon.

## Roadmap

- [ ] App distribution
- [ ] Better marketing (focus on query detector + file editor + terminal for git versioning and coding agents)
- [ ] Export results to more formats
- [ ] Copy selected content as CSV / JSON
- [ ] Allow in-place editing in the data table
- [ ] Refactor connection panel layout (IntelliJ-style)
- [ ] Support diagrams
- [ ] Support MySQL
- [ ] Support SQLite
- [ ] Support duckdb
- [ ] Support Databend

## Tech Stack

- **Rust** — core application logic
- **GPUI** — GPU-accelerated UI framework (from Zed)
- **gpui-component** — higher-level UI components
- **tree-sitter-sql** — syntax highlighting
- **tokio / tokio-postgres** — async PostgreSQL driver
- **rustls** — TLS for database connections

## Alternative projects

- [dbflux](https://github.com/0xErwin1/dbflux)
- [zqlz](https://github.com/samurmaykrr/zqlz)

## License

MIT — see [LICENSE](./LICENSE).
