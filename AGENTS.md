# sqlab
This project is a SQL Editor written in rust, using gpui library (zed editor lib for graphics on GPU).

References:
- https://www.gpui.rs
- https://longbridge.github.io/gpui-component
- https://longbridge.github.io/gpui-component/gallery/

## Guidelines
- before finishing a task, always ensure there is no warning left and the compilation succeeds (`cargo clippy ...`), and the tests pass (`cargo test`) and it well formatted `cargo fmt`. You can execute the `.githooks/pre-commit` script to run these checks automatically.
- There is a fork of gpui-component locally under ../gpui-component. Feel free to make changes to it, just change the Cargo.toml to point there.
  - IMPORTANT: do not format (`cargo fmt`) the `gpui-component` folder
- When adding a new feature with shortcut, remember to add a option for it in the system menu.
