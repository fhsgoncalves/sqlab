# sqlab
This project is a SQL Editor written in rust, using gpui library (zed editor lib for graphics on GPU).

References:
- https://www.gpui.rs
- https://longbridge.github.io/gpui-component
- https://longbridge.github.io/gpui-component/gallery/

## Guidelines
- before finishing a task, always ensure there is no warning left and the compilation succeeds (`cargo check`), and the tests pass (`cargo test`) and it well formatted `cargo fmt`.
- When adding a new feature with shortcut, remember to add a option for it in the system menu.
