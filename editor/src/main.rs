use std::{path::PathBuf, sync::Arc};

use gpui::{
    App, AppContext, Bounds, KeyBinding, Menu, MenuItem, QuitMode, WindowBounds, WindowOptions, px,
    size,
};
use gpui_component::dock::ClosePanel;
use gpui_component::input::{
    Backspace, Copy, Cut, Delete, DeleteToBeginningOfLine, DeleteToEndOfLine, DeleteToNextWordEnd,
    DeleteToPreviousWordStart, Enter, Escape as InputEscape, Indent, IndentInline, MoveDown,
    MoveEnd, MoveHome, MoveLeft, MovePageDown, MovePageUp, MoveRight, MoveToEnd, MoveToEndOfLine,
    MoveToNextWord, MoveToPreviousWord, MoveToStart, MoveToStartOfLine, MoveUp, Outdent,
    OutdentInline, Paste as InputPaste, Redo, SelectAll, SelectToEnd, SelectToEndOfLine,
    SelectToNextWordEnd, SelectToPreviousWordStart, SelectToStart, SelectToStartOfLine, Undo,
};
use gpui_component::{GlobalState, Root};

mod app_theme;
mod assets;
mod credentials;
mod drivers;
mod query_session;
mod schema_cache;
mod shortcuts;
mod ui;
mod workspace;

use shortcuts::{CustomKeymap, ShortcutAction, load_custom_keymap};
use ui::panels::bottom_panel::ToggleBottomPanelMode;
use ui::panels::file_editor::{
    CloseActiveTab as EditorCloseActiveTab, ConfirmSelectedQuery, CutEditorLine, CycleTabBackward,
    CycleTabForward, ExecuteQuery, FormatQuery, IndentLines, NavigateBack, NavigateForward,
    OutdentLines, SaveFile, SelectNextQuery, SelectPreviousQuery, ToggleCommentLines,
    ToggleEditorReplace, ToggleEditorSearch,
};
use ui::panels::file_search::ToggleFileSearch;
use ui::panels::keymap::ToggleKeymap;
use ui::panels::project_search::ToggleProjectSearch;
use ui::panels::result::{
    CloseActiveTab as ResultCloseActiveTab, CopyResultSelection,
    CycleTabBackward as ResultCycleTabBackward, CycleTabForward as ResultCycleTabForward,
    EditResultCell, ExtendResultSelectionDown, ExtendResultSelectionLeft,
    ExtendResultSelectionRight, ExtendResultSelectionUp, SelectResultCellDown,
    SelectResultCellLeft, SelectResultCellRight, SelectResultCellUp, SelectResultFirstCellColumn,
    SelectResultLastCellColumn,
};
use ui::panels::terminal::{
    CloseActiveTab as TerminalCloseActiveTab, CopyTerminalSelection,
    CycleTabBackward as TerminalCycleTabBackward, CycleTabForward as TerminalCycleTabForward,
    NewTerminalTab, Paste,
};
use workspace::{
    CloseRecentFolders, ConfirmRecentFolder, ConfirmSelectedConnection, OpenFolder,
    OpenRecentFolders, SelectNextConnection, SelectNextRecentFolder, SelectPreviousConnection,
    SelectPreviousRecentFolder, ToggleLeftDock, ToggleResultsPanel, ToggleRightDock,
    ToggleSearchReplace, ToggleTerminal, Workspace, load_recent_folders,
};

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

fn register_component_bindings(cx: &mut App) {
    const INPUT: Option<&str> = Some("Input");
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, INPUT),
        KeyBinding::new("shift-backspace", Backspace, INPUT),
        KeyBinding::new("delete", Delete, INPUT),
        KeyBinding::new("shift-delete", Delete, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-backspace", Backspace, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-backspace", DeleteToBeginningOfLine, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-delete", DeleteToEndOfLine, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-backspace", DeleteToPreviousWordStart, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-delete", DeleteToNextWordEnd, INPUT),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-backspace", DeleteToPreviousWordStart, INPUT),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-delete", DeleteToNextWordEnd, INPUT),
        KeyBinding::new(
            "enter",
            Enter {
                secondary: false,
                shift: false,
            },
            INPUT,
        ),
        KeyBinding::new(
            "shift-enter",
            Enter {
                secondary: false,
                shift: true,
            },
            INPUT,
        ),
        KeyBinding::new("escape", InputEscape, INPUT),
        KeyBinding::new("up", MoveUp, INPUT),
        KeyBinding::new("down", MoveDown, INPUT),
        KeyBinding::new("left", MoveLeft, INPUT),
        KeyBinding::new("right", MoveRight, INPUT),
        KeyBinding::new("pageup", MovePageUp, INPUT),
        KeyBinding::new("pagedown", MovePageDown, INPUT),
        KeyBinding::new("home", MoveHome, INPUT),
        KeyBinding::new("end", MoveEnd, INPUT),
        KeyBinding::new("tab", IndentInline, INPUT),
        KeyBinding::new("shift-tab", OutdentInline, INPUT),
        KeyBinding::new("shift-home", SelectToStartOfLine, INPUT),
        KeyBinding::new("shift-end", SelectToEndOfLine, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-]", Indent, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-[", Outdent, INPUT),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-]", Indent, INPUT),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-[", Outdent, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-a", MoveHome, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-e", MoveEnd, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-shift-a", SelectToStartOfLine, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-shift-e", SelectToEndOfLine, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-left", MoveToStartOfLine, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-right", MoveToEndOfLine, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("shift-cmd-left", SelectToStartOfLine, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("shift-cmd-right", SelectToEndOfLine, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-up", MoveToStart, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-down", MoveToEnd, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-shift-up", SelectToStart, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-shift-down", SelectToEnd, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-left", MoveToPreviousWord, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-right", MoveToNextWord, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-shift-left", SelectToPreviousWordStart, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-shift-right", SelectToNextWordEnd, INPUT),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-left", MoveToPreviousWord, INPUT),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-right", MoveToNextWord, INPUT),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-left", SelectToPreviousWordStart, INPUT),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-right", SelectToNextWordEnd, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-a", SelectAll, INPUT),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-a", SelectAll, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-c", Copy, INPUT),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-c", Copy, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-x", Cut, INPUT),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-x", Cut, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-v", InputPaste, INPUT),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-v", InputPaste, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-z", Undo, INPUT),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-shift-z", Redo, INPUT),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-z", Undo, INPUT),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-y", Redo, INPUT),
    ]);

    // Panel-specific key bindings also wiped by clear_key_bindings.
    ui::panels::file_tree::init(cx);
    ui::panels::file_editor::editor::init(cx);
    ui::panels::file_search::init(cx);
    ui::panels::project_search::init(cx);
    ui::panels::keymap::init(cx);
}

pub fn bind_all_keys(cx: &mut App, custom: &CustomKeymap) {
    macro_rules! key {
        ($action:ident) => {
            custom.key_for(ShortcutAction::$action)
        };
    }
    macro_rules! ctx {
        ($action:ident) => {
            custom.context_for(ShortcutAction::$action)
        };
    }
    macro_rules! bind {
        ($action:ident, $handler:expr) => {{
            let key = key!($action);
            let context = ctx!($action);
            KeyBinding::new(&key, $handler, context.as_deref())
        }};
        ($action:ident, $handler:expr, $context:expr) => {{
            let key = key!($action);
            KeyBinding::new(&key, $handler, $context)
        }};
    }

    cx.clear_key_bindings();
    register_component_bindings(cx);
    cx.bind_keys(vec![
        bind!(CloseActiveTab, ClosePanel),
        bind!(CloseActiveTab, EditorCloseActiveTab, Some("editor_tabs")),
        bind!(
            CloseActiveTab,
            TerminalCloseActiveTab,
            Some("terminal_panel")
        ),
        bind!(CloseActiveTab, ResultCloseActiveTab, Some("results_panel")),
        bind!(OpenRecentFolders, OpenRecentFolders),
        bind!(OpenFolder, OpenFolder),
        bind!(ToggleFileSearch, ToggleFileSearch),
        bind!(ToggleEditorSearch, ToggleEditorSearch),
        bind!(ToggleProjectSearch, ToggleProjectSearch),
        bind!(ToggleEditorReplace, ToggleSearchReplace),
        bind!(ExecuteQuery, ExecuteQuery),
        bind!(SaveFile, SaveFile),
        bind!(FormatQuery, FormatQuery),
        bind!(ToggleCommentLines, ToggleCommentLines),
        bind!(IndentLines, IndentLines),
        bind!(OutdentLines, OutdentLines),
        bind!(CutEditorLine, CutEditorLine),
        bind!(
            ExtendResultSelectionUp,
            ExtendResultSelectionUp,
            Some("results_panel")
        ),
        bind!(
            ExtendResultSelectionDown,
            ExtendResultSelectionDown,
            Some("results_panel")
        ),
        bind!(
            ExtendResultSelectionLeft,
            ExtendResultSelectionLeft,
            Some("results_panel")
        ),
        bind!(
            ExtendResultSelectionRight,
            ExtendResultSelectionRight,
            Some("results_panel")
        ),
        bind!(ExtendResultSelectionUp, ExtendResultSelectionUp),
        bind!(ExtendResultSelectionDown, ExtendResultSelectionDown),
        bind!(ExtendResultSelectionLeft, ExtendResultSelectionLeft),
        bind!(ExtendResultSelectionRight, ExtendResultSelectionRight),
        bind!(SelectResultCellLeft, SelectResultCellLeft),
        bind!(SelectResultCellRight, SelectResultCellRight),
        bind!(SelectResultFirstCellColumn, SelectResultFirstCellColumn),
        bind!(SelectResultLastCellColumn, SelectResultLastCellColumn),
        bind!(SelectResultCellUp, SelectResultCellUp),
        bind!(SelectResultCellDown, SelectResultCellDown),
        bind!(EditResultCell, EditResultCell),
        bind!(ToggleBottomPanel, ToggleBottomPanelMode),
        bind!(NewTerminalTab, NewTerminalTab),
        bind!(CopyTerminalSelection, CopyTerminalSelection),
        bind!(PasteTerminal, Paste),
        bind!(CopyResultSelection, CopyResultSelection),
        bind!(CycleTabForward, CycleTabForward),
        bind!(CycleTabBackward, CycleTabBackward),
        bind!(NavigateBack, NavigateBack),
        bind!(NavigateForward, NavigateForward),
        bind!(
            CycleTabForward,
            TerminalCycleTabForward,
            Some("terminal_panel")
        ),
        bind!(
            CycleTabBackward,
            TerminalCycleTabBackward,
            Some("terminal_panel")
        ),
        bind!(
            CycleTabForward,
            ResultCycleTabForward,
            Some("results_panel")
        ),
        bind!(
            CycleTabBackward,
            ResultCycleTabBackward,
            Some("results_panel")
        ),
        bind!(SelectPreviousQuery, SelectPreviousQuery),
        bind!(SelectNextQuery, SelectNextQuery),
        bind!(ConfirmSelectedQuery, ConfirmSelectedQuery),
        bind!(SelectPreviousConnection, SelectPreviousConnection),
        bind!(SelectNextConnection, SelectNextConnection),
        bind!(ConfirmSelectedConnection, ConfirmSelectedConnection),
        bind!(SelectPreviousRecentFolder, SelectPreviousRecentFolder),
        bind!(SelectNextRecentFolder, SelectNextRecentFolder),
        bind!(ConfirmRecentFolder, ConfirmRecentFolder),
        bind!(CloseRecentFolders, CloseRecentFolders),
        bind!(ToggleSettings, ToggleKeymap),
        bind!(ToggleTerminal, ToggleTerminal),
        bind!(ToggleResultsPanel, ToggleResultsPanel),
        bind!(ToggleLeftDock, ToggleLeftDock),
        bind!(ToggleRightDock, ToggleRightDock),
    ]);
}

fn app_menus(_cx: &gpui::App) -> Vec<Menu> {
    vec![
        Menu::new("sq/lab").items(vec![MenuItem::action("Settings...", ToggleKeymap)]),
        Menu::new("File").items(vec![
            MenuItem::action("Open Recent Folder...", OpenRecentFolders),
            MenuItem::action("Open Folder...", OpenFolder),
        ]),
        Menu::new("Navigate").items(vec![
            MenuItem::action("Go Back", NavigateBack),
            MenuItem::action("Go Forward", NavigateForward),
        ]),
        Menu::new("Edit\u{200B}").items(vec![
            MenuItem::action("Find", ToggleEditorSearch),
            MenuItem::action("Find in Files...", ToggleProjectSearch),
            MenuItem::separator(),
            MenuItem::action("Replace", ToggleEditorReplace),
            MenuItem::separator(),
            MenuItem::action("Toggle Comment", ToggleCommentLines),
            MenuItem::action("Indent Lines", IndentLines),
            MenuItem::action("Outdent Lines", OutdentLines),
            MenuItem::action("Cut Line", CutEditorLine),
            MenuItem::separator(),
            MenuItem::action("Format Query", FormatQuery),
            MenuItem::separator(),
            MenuItem::action("Execute Query", ExecuteQuery),
            MenuItem::separator(),
            MenuItem::action("Copy Table Selection", CopyResultSelection),
            MenuItem::action("Copy Terminal Selection", CopyTerminalSelection),
            MenuItem::action("Edit Result Cell", EditResultCell),
            MenuItem::separator(),
            MenuItem::action("Save", SaveFile),
        ]),
        Menu::new("Tab").items(vec![
            MenuItem::action("Close Editor Tab", EditorCloseActiveTab),
            MenuItem::action("Close Terminal Tab", TerminalCloseActiveTab),
            MenuItem::action("Close Results Tab", ResultCloseActiveTab),
        ]),
    ]
}

fn set_app_menus(cx: &mut gpui::App) {
    cx.set_menus(app_menus(cx));

    let owned_menus = app_menus(cx).into_iter().map(|menu| menu.owned()).collect();
    GlobalState::global_mut(cx).set_app_menus(owned_menus);
}

fn main() {
    let app = gpui_platform::application().with_assets(assets::AppAssets);

    let args: Vec<String> = std::env::args().collect();
    let (root_path, initial_file) = if let Some(arg) = args.get(1) {
        let path = PathBuf::from(arg);
        if path.is_file() {
            (
                Some(path.parent().unwrap_or(&path).to_path_buf()),
                Some(path.clone()),
            )
        } else {
            (Some(path), None)
        }
    } else {
        (load_recent_folders().into_iter().next(), None)
    };
    let prompt_for_initial_folder = root_path.is_none();

    app.run(move |cx| {
        gpui_component::init(cx);
        ui::panels::file_tree::init(cx);
        ui::panels::file_editor::editor::init(cx);
        ui::panels::file_search::init(cx);
        ui::panels::project_search::init(cx);
        ui::panels::keymap::init(cx);

        app_theme::init(cx);

        cx.on_action(|switch: &app_theme::SwitchTheme, cx| {
            let theme_name = switch.0.to_string();
            if app_theme::apply_theme_by_name(&theme_name, None, cx) {
                app_theme::persist_selected_theme(&theme_name);
                set_app_menus(cx);
            }
        });

        let custom_keymap = load_custom_keymap();
        bind_all_keys(cx, &custom_keymap);
        set_app_menus(cx);
        cx.activate(true);
        cx.set_quit_mode(QuitMode::LastWindowClosed);

        let window_size = size(px(1400.0), px(900.0));
        let window_bounds = Bounds::centered(None, window_size, cx);

        if let Err(error) = cx.open_window(
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
                let workspace = cx
                    .new(|cx| Workspace::new(root_path.clone(), initial_file.clone(), window, cx));
                let workspace_for_open_folder = workspace.downgrade();
                cx.on_action(move |_: &OpenFolder, cx| {
                    _ = workspace_for_open_folder.update_in(cx, |workspace, _window, cx| {
                        workspace.open_folder_picker(cx);
                    });
                });
                let workspace_for_recent_folders = workspace.downgrade();
                cx.on_action(move |_: &OpenRecentFolders, cx| {
                    _ = workspace_for_recent_folders.update_in(cx, |workspace, window, cx| {
                        workspace.open_recent_folders(window, cx);
                    });
                });
                if prompt_for_initial_folder {
                    let workspace_for_initial_picker = workspace.downgrade();
                    cx.defer(move |cx| {
                        _ = workspace_for_initial_picker.update(cx, |workspace, cx| {
                            workspace.open_folder_picker(cx);
                        });
                    });
                }
                cx.new(|cx| Root::new(workspace, window, cx))
            },
        ) {
            eprintln!("Failed to open window: {}", error);
        }
    });
}
