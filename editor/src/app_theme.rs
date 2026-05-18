use gpui::{Action, App, Menu, MenuItem, SharedString, Window};
use gpui_component::{Theme, ThemeRegistry};

use crate::schema_cache::db;

const THEME_SETTING_KEY: &str = "theme";
const DEFAULT_THEME_NAME: &str = "Sq/lab Dark Theme";
const SQ_LAB_LIGHT_THEME_NAME: &str = "Sq/lab Light Theme";

const BUNDLED_THEMES: &[(&str, &str)] = &[
    (
        "adventure",
        include_str!("ui/themes/gpui-component/adventure.json"),
    ),
    (
        "alduin",
        include_str!("ui/themes/gpui-component/alduin.json"),
    ),
    (
        "asciinema",
        include_str!("ui/themes/gpui-component/asciinema.json"),
    ),
    ("ayu", include_str!("ui/themes/gpui-component/ayu.json")),
    (
        "catppuccin",
        include_str!("ui/themes/gpui-component/catppuccin.json"),
    ),
    ("darcula", include_str!("ui/themes/darcula.json")),
    (
        "everforest",
        include_str!("ui/themes/gpui-component/everforest.json"),
    ),
    (
        "fahrenheit",
        include_str!("ui/themes/gpui-component/fahrenheit.json"),
    ),
    (
        "flexoki",
        include_str!("ui/themes/gpui-component/flexoki.json"),
    ),
    (
        "gruvbox",
        include_str!("ui/themes/gpui-component/gruvbox.json"),
    ),
    (
        "harper",
        include_str!("ui/themes/gpui-component/harper.json"),
    ),
    (
        "hybrid",
        include_str!("ui/themes/gpui-component/hybrid.json"),
    ),
    (
        "jellybeans",
        include_str!("ui/themes/gpui-component/jellybeans.json"),
    ),
    (
        "kibble",
        include_str!("ui/themes/gpui-component/kibble.json"),
    ),
    (
        "macos-classic",
        include_str!("ui/themes/gpui-component/macos-classic.json"),
    ),
    (
        "matrix",
        include_str!("ui/themes/gpui-component/matrix.json"),
    ),
    (
        "mellifluous",
        include_str!("ui/themes/gpui-component/mellifluous.json"),
    ),
    (
        "molokai",
        include_str!("ui/themes/gpui-component/molokai.json"),
    ),
    (
        "solarized",
        include_str!("ui/themes/gpui-component/solarized.json"),
    ),
    (
        "spaceduck",
        include_str!("ui/themes/gpui-component/spaceduck.json"),
    ),
    ("sq-lab", include_str!("ui/themes/sq-lab.json")),
    (
        "tokyonight",
        include_str!("ui/themes/gpui-component/tokyonight.json"),
    ),
    (
        "twilight",
        include_str!("ui/themes/gpui-component/twilight.json"),
    ),
];

#[derive(Action, Clone, PartialEq)]
#[action(namespace = themes, no_json)]
pub struct SwitchTheme(pub SharedString);

pub fn init(cx: &mut App) {
    load_bundled_themes(cx);
    apply_initial_theme(cx);
}

pub fn apply_theme_by_name(theme_name: &str, window: Option<&mut Window>, cx: &mut App) -> bool {
    let (theme_config, paired_theme_config) = {
        let registry = ThemeRegistry::global(cx);
        let themes = registry.themes();
        let theme_config = themes.get(&SharedString::from(theme_name)).cloned();
        let paired_theme_config = paired_sq_lab_theme_name(theme_name)
            .and_then(|paired_theme_name| themes.get(&SharedString::from(paired_theme_name)))
            .cloned();

        (theme_config, paired_theme_config)
    };

    if let Some(theme_config) = theme_config {
        let theme = Theme::global_mut(cx);
        if let Some(paired_theme_config) = paired_theme_config {
            theme.apply_config(&paired_theme_config);
        }
        theme.apply_config(&theme_config);

        if let Some(window) = window {
            window.refresh();
        } else {
            cx.refresh_windows();
        }
        true
    } else {
        false
    }
}

pub fn persist_selected_theme(theme_name: &str) {
    if let Err(error) = db::with_conn(|conn| db::save_setting(conn, THEME_SETTING_KEY, theme_name))
    {
        eprintln!("Failed to persist selected theme: {}", error);
    }
}

pub fn themes_menu_item(cx: &App) -> MenuItem {
    let current_theme_name = Theme::global(cx).theme_name().clone();
    let items = ThemeRegistry::global(cx)
        .sorted_themes()
        .into_iter()
        .map(|theme| {
            let theme_name = theme.name.clone();
            MenuItem::action(theme_name.clone(), SwitchTheme(theme_name.clone()))
                .checked(theme_name == current_theme_name)
        })
        .collect::<Vec<_>>();

    MenuItem::submenu(Menu::new("Themes").items(items))
}

fn load_bundled_themes(cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    for (name, content) in BUNDLED_THEMES {
        if let Err(error) = registry.load_themes_from_str(content) {
            eprintln!(
                "Failed to load bundled GPUI component theme {}: {}",
                name, error
            );
        }
    }
}

fn apply_initial_theme(cx: &mut App) {
    let saved_theme = load_saved_theme_name();

    if saved_theme
        .as_deref()
        .is_some_and(|theme_name| apply_theme_by_name(theme_name, None, cx))
    {
        return;
    }

    if !apply_theme_by_name(DEFAULT_THEME_NAME, None, cx) {
        eprintln!("Failed to apply default theme: {}", DEFAULT_THEME_NAME);
    }
}

fn paired_sq_lab_theme_name(theme_name: &str) -> Option<&'static str> {
    match theme_name {
        DEFAULT_THEME_NAME => Some(SQ_LAB_LIGHT_THEME_NAME),
        SQ_LAB_LIGHT_THEME_NAME => Some(DEFAULT_THEME_NAME),
        _ => None,
    }
}

fn load_saved_theme_name() -> Option<String> {
    match db::with_conn(|conn| db::load_setting(conn, THEME_SETTING_KEY)) {
        Ok(theme_name) => theme_name,
        Err(error) => {
            eprintln!("Failed to load persisted theme: {}", error);
            None
        }
    }
}
