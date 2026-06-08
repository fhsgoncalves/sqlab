use gpui::{App, KeyBinding, SharedString, Unbind};
use gpui_component::dock::ClosePanel;

use crate::{
    app_settings::AppSettings,
    ui::panels::{
        bottom_panel::ToggleBottomPanelMode,
        file_editor::{
            CloseActiveTab as EditorCloseActiveTab, ConfirmSelectedQuery, CutEditorLine,
            CycleTabBackward, CycleTabForward, ExecuteQuery, FormatQuery, IndentLines,
            NavigateBack, NavigateForward, OutdentLines, SaveFile, SelectNextQuery,
            SelectPreviousQuery, ToggleCommentLines, ToggleEditorSearch,
            editor::{CloseEditorSearch, SelectNextEditorMatch, SelectPreviousEditorMatch},
        },
        file_search::{
            CloseFileSearch, ConfirmFileSearch, SelectNextFile, SelectPreviousFile,
            ToggleFileSearch,
        },
        file_tree::{
            CancelInlineEdit, CollapseSelectedItem, CopySelectedName, ExpandSelectedItem,
            OpenSelectedFile, RenameFile, SelectNextItem, SelectPreviousItem,
        },
        project_search::{
            CloseProjectSearch, ConfirmProjectSearch, SelectNextResult, SelectPreviousResult,
            ToggleProjectSearch,
        },
        result::{
            CloseActiveTab as ResultCloseActiveTab, CopyResultSelection,
            CycleTabBackward as ResultCycleTabBackward, CycleTabForward as ResultCycleTabForward,
            EditResultCell, ExtendResultSelectionDown, ExtendResultSelectionLeft,
            ExtendResultSelectionRight, ExtendResultSelectionUp, SelectResultCellDown,
            SelectResultCellLeft, SelectResultCellRight, SelectResultCellUp,
            SelectResultFirstCellColumn, SelectResultLastCellColumn,
        },
        terminal::{
            CloseActiveTab as TerminalCloseActiveTab, CopyTerminalSelection,
            CycleTabBackward as TerminalCycleTabBackward,
            CycleTabForward as TerminalCycleTabForward, NewTerminalTab, Paste,
        },
    },
    workspace::{
        CloseRecentFolders, ConfirmRecentFolder, ConfirmSelectedConnection, OpenFolder,
        OpenRecentFolders, OpenSettings, SelectNextConnection, SelectNextRecentFolder,
        SelectPreviousConnection, SelectPreviousRecentFolder, ToggleSearchReplace,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutAction {
    ClosePanel,
    CloseEditorTab,
    CloseTerminalTab,
    CloseResultTab,
    OpenRecentFolders,
    OpenFolder,
    OpenSettings,
    ToggleFileSearch,
    ToggleEditorSearch,
    ToggleProjectSearch,
    ToggleSearchReplace,
    ExecuteQuery,
    SaveFile,
    FormatQuery,
    ToggleCommentLines,
    IndentLines,
    OutdentLines,
    CutEditorLine,
    ExtendResultSelectionUp,
    ExtendResultSelectionDown,
    ExtendResultSelectionLeft,
    ExtendResultSelectionRight,
    ExtendDataTableSelectionUp,
    ExtendDataTableSelectionDown,
    ExtendDataTableSelectionLeft,
    ExtendDataTableSelectionRight,
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
    CycleEditorTabForward,
    CycleEditorTabBackward,
    NavigateBack,
    NavigateForward,
    CycleTerminalTabForward,
    CycleTerminalTabBackward,
    CycleResultTabForward,
    CycleResultTabBackward,
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
    SelectPreviousFileTreeItem,
    SelectNextFileTreeItem,
    CollapseFileTreeItem,
    ExpandFileTreeItem,
    OpenSelectedFile,
    RenameFile,
    CopySelectedFileName,
    CancelInlineEdit,
    SelectPreviousFile,
    SelectNextFile,
    ConfirmFileSearch,
    CloseFileSearch,
    SelectPreviousProjectSearchResult,
    SelectNextProjectSearchResult,
    ConfirmProjectSearch,
    CloseProjectSearch,
    CloseEditorSearch,
    SelectNextEditorMatch,
    SelectPreviousEditorMatch,
}

pub struct ShortcutDefinition {
    pub action: ShortcutAction,
    pub name: &'static str,
    pub label: &'static str,
    pub context: Option<&'static str>,
    pub macos: &'static str,
    pub linux_windows: &'static str,
}

pub const SHORTCUTS: &[ShortcutDefinition] = &[
    def(
        ShortcutAction::ClosePanel,
        "close_panel",
        "Close Panel",
        None,
        "cmd-w",
        "ctrl-w",
    ),
    def(
        ShortcutAction::CloseEditorTab,
        "close_editor_tab",
        "Close Editor Tab",
        Some("editor_tabs"),
        "cmd-w",
        "ctrl-w",
    ),
    def(
        ShortcutAction::CloseTerminalTab,
        "close_terminal_tab",
        "Close Terminal Tab",
        Some("terminal_panel"),
        "cmd-w",
        "ctrl-w",
    ),
    def(
        ShortcutAction::CloseResultTab,
        "close_result_tab",
        "Close Results Tab",
        Some("results_panel"),
        "cmd-w",
        "ctrl-w",
    ),
    def(
        ShortcutAction::OpenRecentFolders,
        "open_recent_folders",
        "Open Recent Folder",
        None,
        "cmd-o",
        "ctrl-o",
    ),
    def(
        ShortcutAction::OpenFolder,
        "open_folder",
        "Open Folder",
        None,
        "cmd-shift-o",
        "ctrl-shift-o",
    ),
    def(
        ShortcutAction::OpenSettings,
        "open_settings",
        "Open Settings",
        None,
        "cmd-,",
        "ctrl-,",
    ),
    def(
        ShortcutAction::ToggleFileSearch,
        "toggle_file_search",
        "Search Files",
        None,
        "cmd-e",
        "ctrl-e",
    ),
    def(
        ShortcutAction::ToggleEditorSearch,
        "toggle_editor_search",
        "Find",
        Some("Input"),
        "cmd-f",
        "ctrl-f",
    ),
    def(
        ShortcutAction::ToggleProjectSearch,
        "toggle_project_search",
        "Find in Files",
        None,
        "cmd-shift-f",
        "ctrl-shift-f",
    ),
    def(
        ShortcutAction::ToggleSearchReplace,
        "toggle_search_replace",
        "Replace in Files",
        None,
        "cmd-shift-h",
        "ctrl-shift-h",
    ),
    def(
        ShortcutAction::ExecuteQuery,
        "execute_query",
        "Execute Query",
        Some("Input"),
        "cmd-enter",
        "ctrl-enter",
    ),
    def(
        ShortcutAction::SaveFile,
        "save_file",
        "Save File",
        Some("Input"),
        "cmd-s",
        "ctrl-s",
    ),
    def(
        ShortcutAction::FormatQuery,
        "format_query",
        "Format Query",
        Some("Input"),
        "cmd-alt-l",
        "ctrl-alt-l",
    ),
    def(
        ShortcutAction::ToggleCommentLines,
        "toggle_comment_lines",
        "Toggle Comment",
        Some("Input"),
        "cmd-/",
        "ctrl-/",
    ),
    def(
        ShortcutAction::IndentLines,
        "indent_lines",
        "Indent Lines",
        Some("file_editor"),
        "tab",
        "tab",
    ),
    def(
        ShortcutAction::OutdentLines,
        "outdent_lines",
        "Outdent Lines",
        Some("file_editor"),
        "shift-tab",
        "shift-tab",
    ),
    def(
        ShortcutAction::CutEditorLine,
        "cut_editor_line",
        "Cut Line",
        Some("file_editor"),
        "cmd-x",
        "ctrl-x",
    ),
    def(
        ShortcutAction::ExtendResultSelectionUp,
        "extend_result_selection_up",
        "Extend Result Selection Up",
        Some("results_panel"),
        "shift-up",
        "shift-up",
    ),
    def(
        ShortcutAction::ExtendResultSelectionDown,
        "extend_result_selection_down",
        "Extend Result Selection Down",
        Some("results_panel"),
        "shift-down",
        "shift-down",
    ),
    def(
        ShortcutAction::ExtendResultSelectionLeft,
        "extend_result_selection_left",
        "Extend Result Selection Left",
        Some("results_panel"),
        "shift-left",
        "shift-left",
    ),
    def(
        ShortcutAction::ExtendResultSelectionRight,
        "extend_result_selection_right",
        "Extend Result Selection Right",
        Some("results_panel"),
        "shift-right",
        "shift-right",
    ),
    def(
        ShortcutAction::ExtendDataTableSelectionUp,
        "extend_data_table_selection_up",
        "Extend Data Table Selection Up",
        Some("DataTable"),
        "shift-up",
        "shift-up",
    ),
    def(
        ShortcutAction::ExtendDataTableSelectionDown,
        "extend_data_table_selection_down",
        "Extend Data Table Selection Down",
        Some("DataTable"),
        "shift-down",
        "shift-down",
    ),
    def(
        ShortcutAction::ExtendDataTableSelectionLeft,
        "extend_data_table_selection_left",
        "Extend Data Table Selection Left",
        Some("DataTable"),
        "shift-left",
        "shift-left",
    ),
    def(
        ShortcutAction::ExtendDataTableSelectionRight,
        "extend_data_table_selection_right",
        "Extend Data Table Selection Right",
        Some("DataTable"),
        "shift-right",
        "shift-right",
    ),
    def(
        ShortcutAction::SelectResultCellLeft,
        "select_result_cell_left",
        "Select Result Cell Left",
        Some("DataTable"),
        "left",
        "left",
    ),
    def(
        ShortcutAction::SelectResultCellRight,
        "select_result_cell_right",
        "Select Result Cell Right",
        Some("DataTable"),
        "right",
        "right",
    ),
    def(
        ShortcutAction::SelectResultFirstCellColumn,
        "select_result_first_cell_column",
        "Select First Result Column",
        Some("DataTable"),
        "home",
        "home",
    ),
    def(
        ShortcutAction::SelectResultLastCellColumn,
        "select_result_last_cell_column",
        "Select Last Result Column",
        Some("DataTable"),
        "end",
        "end",
    ),
    def(
        ShortcutAction::SelectResultCellUp,
        "select_result_cell_up",
        "Select Result Cell Up",
        Some("DataTable"),
        "up",
        "up",
    ),
    def(
        ShortcutAction::SelectResultCellDown,
        "select_result_cell_down",
        "Select Result Cell Down",
        Some("DataTable"),
        "down",
        "down",
    ),
    def(
        ShortcutAction::EditResultCell,
        "edit_result_cell",
        "Edit Result Cell",
        Some("DataTable"),
        "enter",
        "enter",
    ),
    def(
        ShortcutAction::ToggleBottomPanel,
        "toggle_bottom_panel",
        "Toggle Bottom Panel",
        None,
        "cmd-j",
        "ctrl-j",
    ),
    def(
        ShortcutAction::NewTerminalTab,
        "new_terminal_tab",
        "New Terminal Tab",
        Some("terminal_panel"),
        "cmd-t",
        "ctrl-t",
    ),
    def(
        ShortcutAction::CopyTerminalSelection,
        "copy_terminal_selection",
        "Copy Terminal Selection",
        Some("terminal_panel"),
        "cmd-c",
        "ctrl-c",
    ),
    def(
        ShortcutAction::PasteTerminal,
        "paste_terminal",
        "Paste Terminal",
        Some("terminal_panel"),
        "cmd-v",
        "ctrl-v",
    ),
    def(
        ShortcutAction::CopyResultSelection,
        "copy_result_selection",
        "Copy Table Selection",
        None,
        "cmd-c",
        "ctrl-c",
    ),
    def(
        ShortcutAction::CycleEditorTabForward,
        "cycle_editor_tab_forward",
        "Cycle Editor Tab Forward",
        None,
        "ctrl-tab",
        "ctrl-tab",
    ),
    def(
        ShortcutAction::CycleEditorTabBackward,
        "cycle_editor_tab_backward",
        "Cycle Editor Tab Backward",
        None,
        "ctrl-shift-tab",
        "ctrl-shift-tab",
    ),
    def(
        ShortcutAction::NavigateBack,
        "navigate_back",
        "Go Back",
        None,
        "cmd-[",
        "ctrl-[",
    ),
    def(
        ShortcutAction::NavigateForward,
        "navigate_forward",
        "Go Forward",
        None,
        "cmd-]",
        "ctrl-]",
    ),
    def(
        ShortcutAction::CycleTerminalTabForward,
        "cycle_terminal_tab_forward",
        "Cycle Terminal Tab Forward",
        Some("terminal_panel"),
        "ctrl-tab",
        "ctrl-tab",
    ),
    def(
        ShortcutAction::CycleTerminalTabBackward,
        "cycle_terminal_tab_backward",
        "Cycle Terminal Tab Backward",
        Some("terminal_panel"),
        "ctrl-shift-tab",
        "ctrl-shift-tab",
    ),
    def(
        ShortcutAction::CycleResultTabForward,
        "cycle_result_tab_forward",
        "Cycle Results Tab Forward",
        Some("results_panel"),
        "ctrl-tab",
        "ctrl-tab",
    ),
    def(
        ShortcutAction::CycleResultTabBackward,
        "cycle_result_tab_backward",
        "Cycle Results Tab Backward",
        Some("results_panel"),
        "ctrl-shift-tab",
        "ctrl-shift-tab",
    ),
    def(
        ShortcutAction::SelectPreviousQuery,
        "select_previous_query",
        "Select Previous Query",
        None,
        "up",
        "up",
    ),
    def(
        ShortcutAction::SelectNextQuery,
        "select_next_query",
        "Select Next Query",
        None,
        "down",
        "down",
    ),
    def(
        ShortcutAction::ConfirmSelectedQuery,
        "confirm_selected_query",
        "Confirm Selected Query",
        None,
        "enter",
        "enter",
    ),
    def(
        ShortcutAction::SelectPreviousConnection,
        "select_previous_connection",
        "Select Previous Connection",
        Some("ConnectionSelector"),
        "up",
        "up",
    ),
    def(
        ShortcutAction::SelectNextConnection,
        "select_next_connection",
        "Select Next Connection",
        Some("ConnectionSelector"),
        "down",
        "down",
    ),
    def(
        ShortcutAction::ConfirmSelectedConnection,
        "confirm_selected_connection",
        "Confirm Selected Connection",
        Some("ConnectionSelector"),
        "enter",
        "enter",
    ),
    def(
        ShortcutAction::SelectPreviousRecentFolder,
        "select_previous_recent_folder",
        "Select Previous Recent Folder",
        Some("RecentFolders"),
        "up",
        "up",
    ),
    def(
        ShortcutAction::SelectNextRecentFolder,
        "select_next_recent_folder",
        "Select Next Recent Folder",
        Some("RecentFolders"),
        "down",
        "down",
    ),
    def(
        ShortcutAction::ConfirmRecentFolder,
        "confirm_recent_folder",
        "Confirm Recent Folder",
        Some("RecentFolders"),
        "enter",
        "enter",
    ),
    def(
        ShortcutAction::CloseRecentFolders,
        "close_recent_folders",
        "Close Recent Folders",
        Some("RecentFolders"),
        "escape",
        "escape",
    ),
    def(
        ShortcutAction::SelectPreviousFileTreeItem,
        "select_previous_file_tree_item",
        "Select Previous File",
        Some("FileTree"),
        "up",
        "up",
    ),
    def(
        ShortcutAction::SelectNextFileTreeItem,
        "select_next_file_tree_item",
        "Select Next File",
        Some("FileTree"),
        "down",
        "down",
    ),
    def(
        ShortcutAction::CollapseFileTreeItem,
        "collapse_file_tree_item",
        "Collapse File Tree Item",
        Some("FileTree"),
        "left",
        "left",
    ),
    def(
        ShortcutAction::ExpandFileTreeItem,
        "expand_file_tree_item",
        "Expand File Tree Item",
        Some("FileTree"),
        "right",
        "right",
    ),
    def(
        ShortcutAction::OpenSelectedFile,
        "open_selected_file",
        "Open Selected File",
        Some("FileTree"),
        "enter",
        "enter",
    ),
    def(
        ShortcutAction::RenameFile,
        "rename_file",
        "Rename File",
        Some("FileTree"),
        "f2",
        "f2",
    ),
    def(
        ShortcutAction::CopySelectedFileName,
        "copy_selected_file_name",
        "Copy Selected File Name",
        Some("FileTree"),
        "cmd-c",
        "ctrl-c",
    ),
    def(
        ShortcutAction::CancelInlineEdit,
        "cancel_inline_edit",
        "Cancel Inline Edit",
        Some("FileTree"),
        "escape",
        "escape",
    ),
    def(
        ShortcutAction::SelectPreviousFile,
        "select_previous_file",
        "Select Previous Search File",
        Some("FileSearch"),
        "up",
        "up",
    ),
    def(
        ShortcutAction::SelectNextFile,
        "select_next_file",
        "Select Next Search File",
        Some("FileSearch"),
        "down",
        "down",
    ),
    def(
        ShortcutAction::ConfirmFileSearch,
        "confirm_file_search",
        "Confirm File Search",
        Some("FileSearch"),
        "enter",
        "enter",
    ),
    def(
        ShortcutAction::CloseFileSearch,
        "close_file_search",
        "Close File Search",
        Some("FileSearch"),
        "escape",
        "escape",
    ),
    def(
        ShortcutAction::SelectPreviousProjectSearchResult,
        "select_previous_project_search_result",
        "Select Previous Project Search Result",
        Some("ProjectSearch"),
        "up",
        "up",
    ),
    def(
        ShortcutAction::SelectNextProjectSearchResult,
        "select_next_project_search_result",
        "Select Next Project Search Result",
        Some("ProjectSearch"),
        "down",
        "down",
    ),
    def(
        ShortcutAction::ConfirmProjectSearch,
        "confirm_project_search",
        "Confirm Project Search",
        Some("ProjectSearch"),
        "enter",
        "enter",
    ),
    def(
        ShortcutAction::CloseProjectSearch,
        "close_project_search",
        "Close Project Search",
        Some("ProjectSearch"),
        "escape",
        "escape",
    ),
    def(
        ShortcutAction::CloseEditorSearch,
        "close_editor_search",
        "Close Editor Search",
        Some("EditorSearch"),
        "escape",
        "escape",
    ),
    def(
        ShortcutAction::SelectNextEditorMatch,
        "select_next_editor_match",
        "Select Next Editor Match",
        Some("EditorSearch"),
        "enter",
        "enter",
    ),
    def(
        ShortcutAction::SelectPreviousEditorMatch,
        "select_previous_editor_match",
        "Select Previous Editor Match",
        Some("EditorSearch"),
        "shift-enter",
        "shift-enter",
    ),
];

const fn def(
    action: ShortcutAction,
    name: &'static str,
    label: &'static str,
    context: Option<&'static str>,
    macos: &'static str,
    linux_windows: &'static str,
) -> ShortcutDefinition {
    ShortcutDefinition {
        action,
        name,
        label,
        context,
        macos,
        linux_windows,
    }
}

impl ShortcutDefinition {
    pub fn default_keystroke(&self) -> &'static str {
        if cfg!(target_os = "macos") {
            self.macos
        } else {
            self.linux_windows
        }
    }

    pub fn effective_keystroke(&self, settings: &AppSettings) -> String {
        settings
            .keymap_value(self.name)
            .unwrap_or_else(|| self.default_keystroke().to_string())
    }

    pub fn effective_context(&self, settings: &AppSettings) -> Option<String> {
        settings
            .context_value(self.name)
            .or_else(|| self.context.map(str::to_string))
    }
}

pub fn register(cx: &mut App) {
    let settings = AppSettings::load();
    let mut bindings = Vec::new();
    for definition in SHORTCUTS {
        if settings.keymap.contains_key(definition.name) {
            bindings.push(definition.unbind(definition.default_keystroke(), definition.context));
        }
        bindings.push(definition.binding(
            &definition.effective_keystroke(&settings),
            definition.effective_context(&settings).as_deref(),
        ));
    }
    cx.bind_keys(bindings);
}

impl ShortcutDefinition {
    pub fn rebind(
        &self,
        previous_keystroke: &str,
        previous_context: Option<&str>,
        settings: &AppSettings,
        cx: &mut App,
    ) {
        cx.bind_keys([
            self.unbind(previous_keystroke, previous_context),
            self.binding(
                &self.effective_keystroke(settings),
                self.effective_context(settings).as_deref(),
            ),
        ]);
    }

    fn unbind(&self, keystroke: &str, context: Option<&str>) -> KeyBinding {
        KeyBinding::new(
            keystroke,
            Unbind(SharedString::from(self.gpui_action_name())),
            context,
        )
    }

    fn gpui_action_name(&self) -> String {
        self.binding(self.default_keystroke(), self.context)
            .action()
            .name()
            .to_string()
    }

    fn binding(&self, keystroke: &str, context: Option<&str>) -> KeyBinding {
        match self.action {
            ShortcutAction::ClosePanel => KeyBinding::new(keystroke, ClosePanel, context),
            ShortcutAction::CloseEditorTab => {
                KeyBinding::new(keystroke, EditorCloseActiveTab, context)
            }
            ShortcutAction::CloseTerminalTab => {
                KeyBinding::new(keystroke, TerminalCloseActiveTab, context)
            }
            ShortcutAction::CloseResultTab => {
                KeyBinding::new(keystroke, ResultCloseActiveTab, context)
            }
            ShortcutAction::OpenRecentFolders => {
                KeyBinding::new(keystroke, OpenRecentFolders, context)
            }
            ShortcutAction::OpenFolder => KeyBinding::new(keystroke, OpenFolder, context),
            ShortcutAction::OpenSettings => KeyBinding::new(keystroke, OpenSettings, context),
            ShortcutAction::ToggleFileSearch => {
                KeyBinding::new(keystroke, ToggleFileSearch, context)
            }
            ShortcutAction::ToggleEditorSearch => {
                KeyBinding::new(keystroke, ToggleEditorSearch, context)
            }
            ShortcutAction::ToggleProjectSearch => {
                KeyBinding::new(keystroke, ToggleProjectSearch, context)
            }
            ShortcutAction::ToggleSearchReplace => {
                KeyBinding::new(keystroke, ToggleSearchReplace, context)
            }
            ShortcutAction::ExecuteQuery => KeyBinding::new(keystroke, ExecuteQuery, context),
            ShortcutAction::SaveFile => KeyBinding::new(keystroke, SaveFile, context),
            ShortcutAction::FormatQuery => KeyBinding::new(keystroke, FormatQuery, context),
            ShortcutAction::ToggleCommentLines => {
                KeyBinding::new(keystroke, ToggleCommentLines, context)
            }
            ShortcutAction::IndentLines => KeyBinding::new(keystroke, IndentLines, context),
            ShortcutAction::OutdentLines => KeyBinding::new(keystroke, OutdentLines, context),
            ShortcutAction::CutEditorLine => KeyBinding::new(keystroke, CutEditorLine, context),
            ShortcutAction::ExtendResultSelectionUp
            | ShortcutAction::ExtendDataTableSelectionUp => {
                KeyBinding::new(keystroke, ExtendResultSelectionUp, context)
            }
            ShortcutAction::ExtendResultSelectionDown
            | ShortcutAction::ExtendDataTableSelectionDown => {
                KeyBinding::new(keystroke, ExtendResultSelectionDown, context)
            }
            ShortcutAction::ExtendResultSelectionLeft
            | ShortcutAction::ExtendDataTableSelectionLeft => {
                KeyBinding::new(keystroke, ExtendResultSelectionLeft, context)
            }
            ShortcutAction::ExtendResultSelectionRight
            | ShortcutAction::ExtendDataTableSelectionRight => {
                KeyBinding::new(keystroke, ExtendResultSelectionRight, context)
            }
            ShortcutAction::SelectResultCellLeft => {
                KeyBinding::new(keystroke, SelectResultCellLeft, context)
            }
            ShortcutAction::SelectResultCellRight => {
                KeyBinding::new(keystroke, SelectResultCellRight, context)
            }
            ShortcutAction::SelectResultFirstCellColumn => {
                KeyBinding::new(keystroke, SelectResultFirstCellColumn, context)
            }
            ShortcutAction::SelectResultLastCellColumn => {
                KeyBinding::new(keystroke, SelectResultLastCellColumn, context)
            }
            ShortcutAction::SelectResultCellUp => {
                KeyBinding::new(keystroke, SelectResultCellUp, context)
            }
            ShortcutAction::SelectResultCellDown => {
                KeyBinding::new(keystroke, SelectResultCellDown, context)
            }
            ShortcutAction::EditResultCell => KeyBinding::new(keystroke, EditResultCell, context),
            ShortcutAction::ToggleBottomPanel => {
                KeyBinding::new(keystroke, ToggleBottomPanelMode, context)
            }
            ShortcutAction::NewTerminalTab => KeyBinding::new(keystroke, NewTerminalTab, context),
            ShortcutAction::CopyTerminalSelection => {
                KeyBinding::new(keystroke, CopyTerminalSelection, context)
            }
            ShortcutAction::PasteTerminal => KeyBinding::new(keystroke, Paste, context),
            ShortcutAction::CopyResultSelection => {
                KeyBinding::new(keystroke, CopyResultSelection, context)
            }
            ShortcutAction::CycleEditorTabForward => {
                KeyBinding::new(keystroke, CycleTabForward, context)
            }
            ShortcutAction::CycleEditorTabBackward => {
                KeyBinding::new(keystroke, CycleTabBackward, context)
            }
            ShortcutAction::NavigateBack => KeyBinding::new(keystroke, NavigateBack, context),
            ShortcutAction::NavigateForward => KeyBinding::new(keystroke, NavigateForward, context),
            ShortcutAction::CycleTerminalTabForward => {
                KeyBinding::new(keystroke, TerminalCycleTabForward, context)
            }
            ShortcutAction::CycleTerminalTabBackward => {
                KeyBinding::new(keystroke, TerminalCycleTabBackward, context)
            }
            ShortcutAction::CycleResultTabForward => {
                KeyBinding::new(keystroke, ResultCycleTabForward, context)
            }
            ShortcutAction::CycleResultTabBackward => {
                KeyBinding::new(keystroke, ResultCycleTabBackward, context)
            }
            ShortcutAction::SelectPreviousQuery => {
                KeyBinding::new(keystroke, SelectPreviousQuery, context)
            }
            ShortcutAction::SelectNextQuery => KeyBinding::new(keystroke, SelectNextQuery, context),
            ShortcutAction::ConfirmSelectedQuery => {
                KeyBinding::new(keystroke, ConfirmSelectedQuery, context)
            }
            ShortcutAction::SelectPreviousConnection => {
                KeyBinding::new(keystroke, SelectPreviousConnection, context)
            }
            ShortcutAction::SelectNextConnection => {
                KeyBinding::new(keystroke, SelectNextConnection, context)
            }
            ShortcutAction::ConfirmSelectedConnection => {
                KeyBinding::new(keystroke, ConfirmSelectedConnection, context)
            }
            ShortcutAction::SelectPreviousRecentFolder => {
                KeyBinding::new(keystroke, SelectPreviousRecentFolder, context)
            }
            ShortcutAction::SelectNextRecentFolder => {
                KeyBinding::new(keystroke, SelectNextRecentFolder, context)
            }
            ShortcutAction::ConfirmRecentFolder => {
                KeyBinding::new(keystroke, ConfirmRecentFolder, context)
            }
            ShortcutAction::CloseRecentFolders => {
                KeyBinding::new(keystroke, CloseRecentFolders, context)
            }
            ShortcutAction::SelectPreviousFileTreeItem => {
                KeyBinding::new(keystroke, SelectPreviousItem, context)
            }
            ShortcutAction::SelectNextFileTreeItem => {
                KeyBinding::new(keystroke, SelectNextItem, context)
            }
            ShortcutAction::CollapseFileTreeItem => {
                KeyBinding::new(keystroke, CollapseSelectedItem, context)
            }
            ShortcutAction::ExpandFileTreeItem => {
                KeyBinding::new(keystroke, ExpandSelectedItem, context)
            }
            ShortcutAction::OpenSelectedFile => {
                KeyBinding::new(keystroke, OpenSelectedFile, context)
            }
            ShortcutAction::RenameFile => KeyBinding::new(keystroke, RenameFile, context),
            ShortcutAction::CopySelectedFileName => {
                KeyBinding::new(keystroke, CopySelectedName, context)
            }
            ShortcutAction::CancelInlineEdit => {
                KeyBinding::new(keystroke, CancelInlineEdit, context)
            }
            ShortcutAction::SelectPreviousFile => {
                KeyBinding::new(keystroke, SelectPreviousFile, context)
            }
            ShortcutAction::SelectNextFile => KeyBinding::new(keystroke, SelectNextFile, context),
            ShortcutAction::ConfirmFileSearch => {
                KeyBinding::new(keystroke, ConfirmFileSearch, context)
            }
            ShortcutAction::CloseFileSearch => KeyBinding::new(keystroke, CloseFileSearch, context),
            ShortcutAction::SelectPreviousProjectSearchResult => {
                KeyBinding::new(keystroke, SelectPreviousResult, context)
            }
            ShortcutAction::SelectNextProjectSearchResult => {
                KeyBinding::new(keystroke, SelectNextResult, context)
            }
            ShortcutAction::ConfirmProjectSearch => {
                KeyBinding::new(keystroke, ConfirmProjectSearch, context)
            }
            ShortcutAction::CloseProjectSearch => {
                KeyBinding::new(keystroke, CloseProjectSearch, context)
            }
            ShortcutAction::CloseEditorSearch => {
                KeyBinding::new(keystroke, CloseEditorSearch, context)
            }
            ShortcutAction::SelectNextEditorMatch => {
                KeyBinding::new(keystroke, SelectNextEditorMatch, context)
            }
            ShortcutAction::SelectPreviousEditorMatch => {
                KeyBinding::new(keystroke, SelectPreviousEditorMatch, context)
            }
        }
    }
}
