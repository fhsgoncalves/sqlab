use std::{collections::HashMap, path::PathBuf};

use toml::map::Map;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ShortcutAction {
    CloseActiveTab,
    OpenRecentFolders,
    OpenFolder,
    ToggleFileSearch,
    ToggleEditorSearch,
    ToggleProjectSearch,
    ToggleEditorReplace,
    ExecuteQuery,
    SaveFile,
    FormatQuery,
    GoToDefinition,
    ToggleCommentLines,
    IndentLines,
    OutdentLines,
    CutEditorLine,
    ExtendResultSelectionUp,
    ExtendResultSelectionDown,
    ExtendResultSelectionLeft,
    ExtendResultSelectionRight,
    SelectResultCellLeft,
    SelectResultCellRight,
    SelectResultFirstCellColumn,
    SelectResultLastCellColumn,
    SelectResultCellUp,
    SelectResultCellDown,
    EditResultCell,
    ToggleBottomPanel,
    NewTerminalTab,
    CopyTerminalSelection,
    PasteTerminal,
    CopyResultSelection,
    CycleTabForward,
    CycleTabBackward,
    NavigateBack,
    NavigateForward,
    SelectPreviousQuery,
    SelectNextQuery,
    ConfirmSelectedQuery,
    SelectPreviousConnection,
    SelectNextConnection,
    ConfirmSelectedConnection,
    SelectPreviousRecentFolder,
    SelectNextRecentFolder,
    ConfirmRecentFolder,
    CloseRecentFolders,
    ToggleSettings,
    ToggleTerminal,
    ToggleResultsPanel,
    ToggleLeftDock,
    ToggleRightDock,
}

#[derive(Clone, Copy, Debug)]
pub struct ShortcutDefinition {
    pub action: ShortcutAction,
    pub id: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    pub macos_key: &'static str,
    pub linux_windows_key: &'static str,
    pub context: Option<&'static str>,
    pub visible: bool,
}

impl ShortcutDefinition {
    pub fn default_key(&self) -> &'static str {
        if cfg!(target_os = "macos") {
            self.macos_key
        } else {
            self.linux_windows_key
        }
    }
}

macro_rules! shortcut {
    ($action:ident, $id:literal, $label:literal, $category:literal, $macos:literal, $linux_windows:literal, $context:expr, $visible:expr) => {
        ShortcutDefinition {
            action: ShortcutAction::$action,
            id: $id,
            label: $label,
            category: $category,
            macos_key: $macos,
            linux_windows_key: $linux_windows,
            context: $context,
            visible: $visible,
        }
    };
}

pub const ALL_SHORTCUTS: &[ShortcutDefinition] = &[
    shortcut!(
        CloseActiveTab,
        "close_active_tab",
        "Close Active Tab",
        "File",
        "cmd-w",
        "ctrl-w",
        None,
        true
    ),
    shortcut!(
        OpenRecentFolders,
        "open_recent_folders",
        "Open Recent Folder",
        "File",
        "cmd-o",
        "ctrl-o",
        None,
        true
    ),
    shortcut!(
        OpenFolder,
        "open_folder",
        "Open Folder...",
        "File",
        "cmd-shift-o",
        "ctrl-shift-o",
        None,
        true
    ),
    shortcut!(
        SaveFile,
        "save_file",
        "Save File",
        "File",
        "cmd-s",
        "ctrl-s",
        Some("Input"),
        true
    ),
    shortcut!(
        ExecuteQuery,
        "execute_query",
        "Execute Query",
        "Editor",
        "cmd-enter",
        "ctrl-enter",
        Some("Input"),
        true
    ),
    shortcut!(
        FormatQuery,
        "format_query",
        "Format Query",
        "Editor",
        "cmd-alt-l",
        "ctrl-alt-l",
        Some("Input"),
        true
    ),
    shortcut!(
        ToggleEditorSearch,
        "toggle_editor_search",
        "Find",
        "Editor",
        "cmd-f",
        "ctrl-f",
        Some("Input"),
        true
    ),
    shortcut!(
        ToggleProjectSearch,
        "toggle_project_search",
        "Find in Files",
        "Editor",
        "cmd-shift-f",
        "ctrl-shift-f",
        None,
        true
    ),
    shortcut!(
        ToggleEditorReplace,
        "toggle_editor_replace",
        "Replace",
        "Editor",
        "cmd-shift-h",
        "ctrl-shift-h",
        None,
        true
    ),
    shortcut!(
        GoToDefinition,
        "go_to_definition",
        "Go to Definition",
        "Editor",
        "cmd-b",
        "ctrl-b",
        None,
        true
    ),
    shortcut!(
        ToggleCommentLines,
        "toggle_comment_lines",
        "Toggle Comment",
        "Editor",
        "cmd-/",
        "ctrl-/",
        Some("Input"),
        true
    ),
    shortcut!(
        IndentLines,
        "indent_lines",
        "Indent Lines",
        "Editor",
        "tab",
        "tab",
        Some("file_editor"),
        true
    ),
    shortcut!(
        OutdentLines,
        "outdent_lines",
        "Outdent Lines",
        "Editor",
        "shift-tab",
        "shift-tab",
        Some("file_editor"),
        true
    ),
    shortcut!(
        CutEditorLine,
        "cut_editor_line",
        "Cut Line",
        "Editor",
        "cmd-x",
        "ctrl-x",
        Some("file_editor"),
        true
    ),
    shortcut!(
        SelectPreviousQuery,
        "select_previous_query",
        "Select Previous Query",
        "Editor",
        "up",
        "up",
        None,
        true
    ),
    shortcut!(
        SelectNextQuery,
        "select_next_query",
        "Select Next Query",
        "Editor",
        "down",
        "down",
        None,
        true
    ),
    shortcut!(
        ConfirmSelectedQuery,
        "confirm_selected_query",
        "Confirm Selected Query",
        "Editor",
        "enter",
        "enter",
        None,
        true
    ),
    shortcut!(
        NavigateBack,
        "navigate_back",
        "Go Back",
        "Navigation",
        "cmd-[",
        "ctrl-[",
        None,
        true
    ),
    shortcut!(
        NavigateForward,
        "navigate_forward",
        "Go Forward",
        "Navigation",
        "cmd-]",
        "ctrl-]",
        None,
        true
    ),
    shortcut!(
        ToggleFileSearch,
        "toggle_file_search",
        "Search Files",
        "Navigation",
        "cmd-e",
        "ctrl-e",
        None,
        true
    ),
    shortcut!(
        CycleTabForward,
        "cycle_tab_forward",
        "Cycle Tab Forward",
        "Navigation",
        "ctrl-tab",
        "ctrl-tab",
        None,
        true
    ),
    shortcut!(
        CycleTabBackward,
        "cycle_tab_backward",
        "Cycle Tab Backward",
        "Navigation",
        "ctrl-shift-tab",
        "ctrl-shift-tab",
        None,
        true
    ),
    shortcut!(
        SelectPreviousConnection,
        "select_previous_connection",
        "Select Previous Connection",
        "Navigation",
        "up",
        "up",
        Some("ConnectionSelector"),
        true
    ),
    shortcut!(
        SelectNextConnection,
        "select_next_connection",
        "Select Next Connection",
        "Navigation",
        "down",
        "down",
        Some("ConnectionSelector"),
        true
    ),
    shortcut!(
        ConfirmSelectedConnection,
        "confirm_selected_connection",
        "Confirm Selected Connection",
        "Navigation",
        "enter",
        "enter",
        Some("ConnectionSelector"),
        true
    ),
    shortcut!(
        SelectPreviousRecentFolder,
        "select_previous_recent_folder",
        "Select Previous Recent Folder",
        "Navigation",
        "up",
        "up",
        Some("RecentFolders"),
        true
    ),
    shortcut!(
        SelectNextRecentFolder,
        "select_next_recent_folder",
        "Select Next Recent Folder",
        "Navigation",
        "down",
        "down",
        Some("RecentFolders"),
        true
    ),
    shortcut!(
        ConfirmRecentFolder,
        "confirm_recent_folder",
        "Confirm Recent Folder",
        "Navigation",
        "enter",
        "enter",
        Some("RecentFolders"),
        true
    ),
    shortcut!(
        CloseRecentFolders,
        "close_recent_folders",
        "Close Recent Folders",
        "Navigation",
        "escape",
        "escape",
        Some("RecentFolders"),
        true
    ),
    shortcut!(
        ExtendResultSelectionUp,
        "extend_result_selection_up",
        "Extend Selection Up",
        "Results",
        "shift-up",
        "shift-up",
        Some("DataTable"),
        true
    ),
    shortcut!(
        ExtendResultSelectionDown,
        "extend_result_selection_down",
        "Extend Selection Down",
        "Results",
        "shift-down",
        "shift-down",
        Some("DataTable"),
        true
    ),
    shortcut!(
        ExtendResultSelectionLeft,
        "extend_result_selection_left",
        "Extend Selection Left",
        "Results",
        "shift-left",
        "shift-left",
        Some("DataTable"),
        true
    ),
    shortcut!(
        ExtendResultSelectionRight,
        "extend_result_selection_right",
        "Extend Selection Right",
        "Results",
        "shift-right",
        "shift-right",
        Some("DataTable"),
        true
    ),
    shortcut!(
        SelectResultCellLeft,
        "select_result_cell_left",
        "Select Cell Left",
        "Results",
        "left",
        "left",
        Some("DataTable"),
        true
    ),
    shortcut!(
        SelectResultCellRight,
        "select_result_cell_right",
        "Select Cell Right",
        "Results",
        "right",
        "right",
        Some("DataTable"),
        true
    ),
    shortcut!(
        SelectResultFirstCellColumn,
        "select_result_first_cell_column",
        "Select First Cell Column",
        "Results",
        "home",
        "home",
        Some("DataTable"),
        true
    ),
    shortcut!(
        SelectResultLastCellColumn,
        "select_result_last_cell_column",
        "Select Last Cell Column",
        "Results",
        "end",
        "end",
        Some("DataTable"),
        true
    ),
    shortcut!(
        SelectResultCellUp,
        "select_result_cell_up",
        "Select Cell Up",
        "Results",
        "up",
        "up",
        Some("DataTable"),
        true
    ),
    shortcut!(
        SelectResultCellDown,
        "select_result_cell_down",
        "Select Cell Down",
        "Results",
        "down",
        "down",
        Some("DataTable"),
        true
    ),
    shortcut!(
        CopyResultSelection,
        "copy_result_selection",
        "Copy Results",
        "Results",
        "cmd-c",
        "ctrl-c",
        None,
        true
    ),
    shortcut!(
        EditResultCell,
        "edit_result_cell",
        "Edit Result Cell",
        "Results",
        "enter",
        "enter",
        Some("DataTable"),
        true
    ),
    shortcut!(
        NewTerminalTab,
        "new_terminal_tab",
        "New Terminal Tab",
        "Terminal",
        "cmd-t",
        "ctrl-t",
        Some("terminal_panel"),
        true
    ),
    shortcut!(
        CopyTerminalSelection,
        "copy_terminal_selection",
        "Copy Terminal Selection",
        "Terminal",
        "cmd-c",
        "ctrl-c",
        Some("terminal_panel"),
        true
    ),
    shortcut!(
        PasteTerminal,
        "paste_terminal",
        "Paste in Terminal",
        "Terminal",
        "cmd-v",
        "ctrl-v",
        Some("terminal_panel"),
        true
    ),
    shortcut!(
        ToggleLeftDock,
        "toggle_left_dock",
        "Toggle File Panel",
        "Panels",
        "cmd-1",
        "ctrl-1",
        None,
        true
    ),
    shortcut!(
        ToggleRightDock,
        "toggle_right_dock",
        "Toggle Connection Panel",
        "Panels",
        "cmd-2",
        "ctrl-2",
        None,
        true
    ),
    shortcut!(
        ToggleTerminal,
        "toggle_terminal",
        "Toggle Terminal",
        "Panels",
        "cmd-shift-t",
        "ctrl-shift-t",
        None,
        true
    ),
    shortcut!(
        ToggleResultsPanel,
        "toggle_results_panel",
        "Toggle Results",
        "Panels",
        "cmd-shift-r",
        "ctrl-shift-r",
        None,
        true
    ),
    shortcut!(
        ToggleBottomPanel,
        "toggle_bottom_panel",
        "Toggle Bottom Panel",
        "Panels",
        "cmd-j",
        "ctrl-j",
        None,
        true
    ),
    shortcut!(
        ToggleSettings,
        "toggle_settings",
        "Settings",
        "Panels",
        "cmd-,",
        "ctrl-,",
        None,
        true
    ),
];

pub fn shortcut(action: ShortcutAction) -> Option<&'static ShortcutDefinition> {
    ALL_SHORTCUTS
        .iter()
        .find(|definition| definition.action == action)
}

#[derive(Clone, Debug, Default)]
pub struct CustomKeymap {
    bindings: HashMap<String, String>,
    contexts: HashMap<String, Option<String>>,
}

impl CustomKeymap {
    pub fn key_for(&self, action: ShortcutAction) -> String {
        let Some(definition) = shortcut(action) else {
            return String::new();
        };
        self.bindings
            .get(definition.id)
            .cloned()
            .unwrap_or_else(|| definition.default_key().to_string())
    }

    pub fn context_for(&self, action: ShortcutAction) -> Option<String> {
        let definition = shortcut(action)?;
        self.contexts
            .get(definition.id)
            .cloned()
            .unwrap_or_else(|| definition.context.map(ToString::to_string))
    }

    pub fn key_for_definition(&self, definition: &ShortcutDefinition) -> String {
        self.bindings
            .get(definition.id)
            .cloned()
            .unwrap_or_else(|| definition.default_key().to_string())
    }

    pub fn context_for_definition(&self, definition: &ShortcutDefinition) -> Option<String> {
        self.contexts
            .get(definition.id)
            .cloned()
            .unwrap_or_else(|| definition.context.map(ToString::to_string))
    }

    pub fn has_custom_key(&self, definition: &ShortcutDefinition) -> bool {
        self.bindings.contains_key(definition.id)
    }

    fn has_custom_context(&self, definition: &ShortcutDefinition) -> bool {
        self.contexts.contains_key(definition.id)
    }

    pub fn set_key(&mut self, definition: &ShortcutDefinition, key: String) {
        let key = normalize_key(&key);
        if key.is_empty() || key == definition.default_key() {
            self.bindings.remove(definition.id);
        } else {
            self.bindings.insert(definition.id.to_string(), key);
        }
    }

    pub fn reset_key(&mut self, definition: &ShortcutDefinition) {
        self.bindings.remove(definition.id);
    }

    pub fn reset_all(&mut self) {
        self.bindings.clear();
        self.contexts.clear();
    }
}

fn app_data_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sqlab")
}

pub fn settings_path() -> PathBuf {
    app_data_dir().join("settings.toml")
}

pub fn load_setting_string(key: &str) -> Option<String> {
    let value = load_settings_value()?;
    value
        .as_table()?
        .get(key)?
        .as_str()
        .map(ToString::to_string)
}

pub fn save_setting_string(key: &str, value: &str) {
    let mut root = load_settings_table_without_keymap();
    root.insert(key.to_string(), toml::Value::String(value.to_string()));
    let keymap = load_custom_keymap();
    write_settings(root, &keymap);
}

pub fn load_custom_keymap() -> CustomKeymap {
    let Some(value) = load_settings_value() else {
        return CustomKeymap::default();
    };
    let Some(root) = value.as_table() else {
        return CustomKeymap::default();
    };

    let mut bindings = HashMap::new();
    let mut contexts = HashMap::new();
    let keymap = root
        .get("keymap")
        .and_then(|value| value.as_table())
        .unwrap_or(root);

    for definition in ALL_SHORTCUTS {
        let Some(value) = keymap.get(definition.id) else {
            continue;
        };

        if let Some(shortcut) = value.as_str() {
            bindings.insert(definition.id.to_string(), normalize_key(shortcut));
            continue;
        }

        let Some(entry) = value.as_table() else {
            continue;
        };
        if let Some(shortcut) = entry.get("shortcut").and_then(|value| value.as_str()) {
            bindings.insert(definition.id.to_string(), normalize_key(shortcut));
        }
        if let Some(context) = entry
            .get("context")
            .or_else(|| entry.get("content"))
            .and_then(|value| value.as_str())
        {
            contexts.insert(definition.id.to_string(), normalize_context(context));
        }
    }

    CustomKeymap { bindings, contexts }
}

pub fn save_custom_keymap(keymap: &CustomKeymap) {
    let path = settings_path();
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("failed to create settings directory: {}", error);
        return;
    }

    let content = serialize_keymap_settings(keymap);
    let root = load_settings_table_without_keymap();
    write_settings_content(path, root, content);
}

fn serialize_keymap_settings(keymap: &CustomKeymap) -> String {
    let mut lines = Vec::new();
    for definition in ALL_SHORTCUTS {
        let shortcut = keymap.bindings.get(definition.id);
        let effective_context = keymap.context_for_definition(definition);
        let should_write_context = keymap.has_custom_context(definition)
            || (shortcut.is_some() && effective_context.is_some());

        match (shortcut, should_write_context.then_some(effective_context)) {
            (Some(shortcut), Some(context)) => lines.push(format!(
                "{} = {{ shortcut = \"{}\", context = \"{}\" }}",
                definition.id,
                display_key(shortcut),
                context.unwrap_or_else(|| "global".to_string())
            )),
            (Some(shortcut), None) => {
                lines.push(format!("{} = \"{}\"", definition.id, display_key(shortcut)))
            }
            (None, Some(context)) => lines.push(format!(
                "{} = {{ context = \"{}\" }}",
                definition.id,
                context.unwrap_or_else(|| "global".to_string())
            )),
            (None, None) => {}
        }
    }

    if lines.is_empty() {
        String::new()
    } else {
        lines.insert(0, "[keymap]".to_string());
        lines.push(String::new());
        lines.join("\n")
    }
}

fn load_settings_value() -> Option<toml::Value> {
    let content = std::fs::read_to_string(settings_path()).ok()?;
    toml::from_str::<toml::Value>(&content).ok()
}

fn load_settings_table_without_keymap() -> Map<String, toml::Value> {
    load_settings_value()
        .and_then(|value| value.as_table().cloned())
        .map(|mut table| {
            table.remove("keymap");
            for definition in ALL_SHORTCUTS {
                table.remove(definition.id);
            }
            table
        })
        .unwrap_or_default()
}

fn write_settings(root: Map<String, toml::Value>, keymap: &CustomKeymap) {
    write_settings_content(settings_path(), root, serialize_keymap_settings(keymap));
}

fn write_settings_content(path: PathBuf, root: Map<String, toml::Value>, keymap_content: String) {
    let mut content = if root.is_empty() {
        String::new()
    } else {
        match toml::to_string_pretty(&toml::Value::Table(root)) {
            Ok(content) => content,
            Err(error) => {
                eprintln!("failed to serialize settings: {}", error);
                return;
            }
        }
    };

    if !content.is_empty() && !keymap_content.is_empty() && !content.ends_with("\n\n") {
        content.push('\n');
    }
    content.push_str(&keymap_content);

    if let Err(error) = std::fs::write(path, content) {
        eprintln!("failed to save settings: {}", error);
    }
}

pub fn normalize_key(key: &str) -> String {
    key.trim().to_lowercase().replace('+', "-")
}

pub fn display_key(key: &str) -> String {
    key.replace('-', "+")
}

fn normalize_context(context: &str) -> Option<String> {
    let trimmed = context.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("global")
        || trimmed.eq_ignore_ascii_case("none")
    {
        None
    } else {
        Some(trimmed.to_string())
    }
}
