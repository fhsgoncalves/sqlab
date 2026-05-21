use std::{path::PathBuf, sync::Arc};

use gpui::{
    AppContext, Bounds, KeyBinding, Menu, MenuItem, QuitMode, WindowBounds, WindowOptions, px, size,
};
use gpui_component::Root;
use gpui_component::dock::ClosePanel;

mod app_theme;
mod assets;
mod credentials;
mod drivers;
mod schema_cache;
mod ui;
mod workspace;

use ui::panels::bottom_panel::ToggleBottomPanelMode;
use ui::panels::file_editor::{
    ConfirmSelectedQuery, CycleTabBackward, CycleTabForward, ExecuteQuery, FormatQuery, SaveFile,
    SelectNextQuery, SelectPreviousQuery, ToggleEditorReplace, ToggleEditorSearch,
};
use ui::panels::file_search::ToggleFileSearch;
use ui::panels::project_search::ToggleProjectSearch;
use ui::panels::result::{
    CopyResultSelection, CycleTabBackward as ResultCycleTabBackward,
    CycleTabForward as ResultCycleTabForward, EditResultCell, ExtendResultSelectionDown,
    ExtendResultSelectionLeft, ExtendResultSelectionRight, ExtendResultSelectionUp,
};
use ui::panels::terminal::{
    CopyTerminalSelection, CycleTabBackward as TerminalCycleTabBackward,
    CycleTabForward as TerminalCycleTabForward, NewTerminalTab, Paste,
};
use workspace::{OpenFolder, ToggleSearchReplace, Workspace};

fn app_icon() -> Option<Arc<image::RgbaImage>> {
    let bytes = include_bytes!("../assets/app-icon.png");
    match image::load_from_memory_with_format(bytes, image::ImageFormat::Png) {
        Ok(image) => Some(Arc::new(image.to_rgba8())),
        Err(error) => {
            eprintln!("Failed to load app icon: {}", error);
            None
        }
    }
}

fn set_app_menus(cx: &mut gpui::App) {
    cx.set_menus(vec![
        Menu::new("sq/lab").items(vec![app_theme::themes_menu_item(cx)]),
        Menu::new("File").items(vec![MenuItem::action("Open Folder...", OpenFolder)]),
        Menu::new("Edit\u{200B}").items(vec![
            MenuItem::action("Find", ToggleEditorSearch),
            MenuItem::action("Find in Files...", ToggleProjectSearch),
            MenuItem::separator(),
            MenuItem::action("Replace", ToggleEditorReplace),
            MenuItem::separator(),
            MenuItem::action("Format Query", FormatQuery),
            MenuItem::separator(),
            MenuItem::action("Copy Result Selection", CopyResultSelection),
            MenuItem::action("Copy Terminal Selection", CopyTerminalSelection),
            MenuItem::action("Edit Result Cell", EditResultCell),
            MenuItem::separator(),
            MenuItem::action("Save", SaveFile),
        ]),
    ]);
}

fn main() {
    let app = gpui_platform::application().with_assets(assets::AppAssets);

    let args: Vec<String> = std::env::args().collect();
    let (root_path, initial_file) = if let Some(arg) = args.get(1) {
        let path = PathBuf::from(arg);
        if path.is_file() {
            (
                path.parent().unwrap_or(&path).to_path_buf(),
                Some(path.clone()),
            )
        } else {
            (path, None)
        }
    } else {
        (
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            None,
        )
    };

    app.run(move |cx| {
        gpui_component::init(cx);
        ui::panels::file_tree::init(cx);
        ui::panels::file_editor::editor::init(cx);
        ui::panels::file_search::init(cx);
        ui::panels::project_search::init(cx);

        app_theme::init(cx);

        cx.on_action(|switch: &app_theme::SwitchTheme, cx| {
            let theme_name = switch.0.to_string();
            if app_theme::apply_theme_by_name(&theme_name, None, cx) {
                app_theme::persist_selected_theme(&theme_name);
                set_app_menus(cx);
            }
        });

        cx.bind_keys(vec![
            KeyBinding::new("cmd-w", ClosePanel, None),
            KeyBinding::new("cmd-e", ToggleFileSearch, None),
            KeyBinding::new("cmd-f", ToggleEditorSearch, Some("Input")),
            KeyBinding::new("cmd-shift-f", ToggleProjectSearch, None),
            KeyBinding::new("cmd-shift-h", ToggleSearchReplace, None),
            KeyBinding::new("cmd-enter", ExecuteQuery, Some("Input")),
            KeyBinding::new("cmd-s", SaveFile, Some("Input")),
            KeyBinding::new("cmd-alt-l", FormatQuery, Some("Input")),
            KeyBinding::new("shift-up", ExtendResultSelectionUp, Some("results_panel")),
            KeyBinding::new(
                "shift-down",
                ExtendResultSelectionDown,
                Some("results_panel"),
            ),
            KeyBinding::new(
                "shift-left",
                ExtendResultSelectionLeft,
                Some("results_panel"),
            ),
            KeyBinding::new(
                "shift-right",
                ExtendResultSelectionRight,
                Some("results_panel"),
            ),
            KeyBinding::new("shift-up", ExtendResultSelectionUp, Some("DataTable")),
            KeyBinding::new("shift-down", ExtendResultSelectionDown, Some("DataTable")),
            KeyBinding::new("shift-left", ExtendResultSelectionLeft, Some("DataTable")),
            KeyBinding::new("shift-right", ExtendResultSelectionRight, Some("DataTable")),
            KeyBinding::new("enter", EditResultCell, Some("DataTable")),
            KeyBinding::new("cmd-j", ToggleBottomPanelMode, None),
            KeyBinding::new("cmd-t", NewTerminalTab, Some("terminal_panel")),
            KeyBinding::new("cmd-c", CopyTerminalSelection, Some("terminal_panel")),
            KeyBinding::new("cmd-v", Paste, Some("terminal_panel")),
            KeyBinding::new("cmd-c", CopyResultSelection, None),
            KeyBinding::new("ctrl-tab", CycleTabForward, None),
            KeyBinding::new("ctrl-shift-tab", CycleTabBackward, None),
            KeyBinding::new("ctrl-tab", TerminalCycleTabForward, Some("terminal_panel")),
            KeyBinding::new(
                "ctrl-shift-tab",
                TerminalCycleTabBackward,
                Some("terminal_panel"),
            ),
            KeyBinding::new("ctrl-tab", ResultCycleTabForward, Some("results_panel")),
            KeyBinding::new(
                "ctrl-shift-tab",
                ResultCycleTabBackward,
                Some("results_panel"),
            ),
            KeyBinding::new("up", SelectPreviousQuery, None),
            KeyBinding::new("down", SelectNextQuery, None),
            KeyBinding::new("enter", ConfirmSelectedQuery, None),
        ]);
        set_app_menus(cx);
        cx.activate(true);
        cx.set_quit_mode(QuitMode::LastWindowClosed);

        let window_size = size(px(1400.0), px(900.0));
        let window_bounds = Bounds::centered(None, window_size, cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(window_bounds)),
                titlebar: Some(gpui_component::TitleBar::title_bar_options()),
                window_min_size: Some(gpui::Size {
                    width: px(800.),
                    height: px(600.),
                }),
                icon: app_icon(),
                ..Default::default()
            },
            |window, cx| {
                let workspace =
                    cx.new(|cx| Workspace::new(root_path, initial_file.clone(), window, cx));
                cx.new(|cx| Root::new(workspace, window, cx))
            },
        )
        .expect("Failed to open window");
    });
}
