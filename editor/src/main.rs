use std::{path::PathBuf, sync::Arc};

use gpui::{AppContext, Bounds, Menu, MenuItem, QuitMode, WindowBounds, WindowOptions, px, size};
use gpui_component::{GlobalState, Root};

mod app_settings;
mod app_theme;
mod assets;
mod credentials;
mod drivers;
mod keymap;
mod query_session;
mod schema_cache;
mod ui;
mod workspace;

use ui::panels::file_editor::{
    CloseActiveTab as EditorCloseActiveTab, CutEditorLine, ExecuteQuery, FormatQuery, IndentLines,
    NavigateBack, NavigateForward, OutdentLines, SaveFile, ToggleCommentLines, ToggleEditorReplace,
    ToggleEditorSearch,
};
use ui::panels::project_search::ToggleProjectSearch;
use ui::panels::result::{
    CloseActiveTab as ResultCloseActiveTab, CopyResultSelection, EditResultCell,
};
use ui::panels::terminal::{CloseActiveTab as TerminalCloseActiveTab, CopyTerminalSelection};
use workspace::{OpenFolder, OpenRecentFolders, OpenSettings, Workspace, load_recent_folders};

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

fn app_menus(_cx: &gpui::App) -> Vec<Menu> {
    vec![
        Menu::new("sq/lab").items(vec![MenuItem::action("Settings...", OpenSettings)]),
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

        app_theme::init(cx);

        cx.on_action(|switch: &app_theme::SwitchTheme, cx| {
            let theme_name = switch.0.to_string();
            if app_theme::apply_theme_by_name(&theme_name, None, cx) {
                app_theme::persist_selected_theme(&theme_name);
                set_app_menus(cx);
            }
        });

        keymap::register(cx);
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
                let workspace_for_settings = workspace.downgrade();
                cx.on_action(move |_: &OpenSettings, cx| {
                    _ = workspace_for_settings.update_in(cx, |workspace, window, cx| {
                        workspace.open_settings(window, cx);
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
