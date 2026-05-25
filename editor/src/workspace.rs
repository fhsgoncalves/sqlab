use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, ScrollHandle, StatefulInteractiveElement, Styled, Window,
    actions, div, hsla, point, prelude::FluentBuilder, px,
};
use gpui_component::ActiveTheme;
use gpui_component::{
    Icon, IconName, Root, Sizable, TitleBar, WindowExt,
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockItem, DockPlacement},
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::{ScrollableElement as _, Scrollbar},
    spinner::Spinner,
    v_flex,
};

use crate::credentials;
use crate::query_session::QuerySessionStore;
use crate::schema_cache;
use crate::ui::activity::ActivityTracker;
use crate::ui::panels::bottom_panel::{BottomPanel, BottomPanelMode, ToggleBottomPanelMode};
use crate::ui::panels::connection::ConnectionPanel;
use crate::ui::panels::diagram::{DiagramModel, ShowDiagramEvent};
use crate::ui::panels::file_editor::data_editor::ShowDataEditorEvent;
use crate::ui::panels::file_editor::query_detector::QueryRange;
use crate::ui::panels::file_editor::{
    EditorPanel, EditorTabs, ExecuteQuery, QueryChoice, QuerySelected, QuerySelector, SaveFile,
};
use crate::ui::panels::file_search::{FileSearch, FileSearchEvent, ToggleFileSearch};
use crate::ui::panels::file_tree::{FileTreePanel, OpenFileEvent, RootChangedEvent};
use crate::ui::panels::project_search::{ProjectSearch, ProjectSearchEvent, ToggleProjectSearch};
use crate::ui::panels::result::ResultPanel;
use crate::ui::panels::terminal::TerminalPanel;
use sqlab_drivers_core::{
    ColumnMetadata, ConnectionStatus, DataSourceConfig, DataSourceError, QueryExecutionOptions,
    QueryResult, manager::DataSourceManager,
};

actions!(
    workspace,
    [
        OpenFolder,
        OpenRecentFolders,
        CloseRecentFolders,
        ConfirmRecentFolder,
        SelectPreviousRecentFolder,
        SelectNextRecentFolder,
        ToggleSearchReplace,
        SelectPreviousConnection,
        SelectNextConnection,
        ConfirmSelectedConnection
    ]
);

const CONNECTION_SELECTOR_CONTEXT: &str = "ConnectionSelector";
const CONNECTION_SELECTOR_LIST_HEIGHT: f32 = 280.0;
const CONNECTION_SELECTOR_ROW_HEIGHT: f32 = 58.0;
const RECENT_FOLDERS_CONTEXT: &str = "RecentFolders";
const RECENT_FOLDERS_LIMIT: usize = 20;

fn app_data_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sqlab")
}

fn recent_folders_path() -> PathBuf {
    app_data_dir().join("recent_folders.json")
}

fn load_recent_folders() -> Vec<PathBuf> {
    let Ok(content) = std::fs::read_to_string(recent_folders_path()) else {
        return Vec::new();
    };
    let Ok(folders) = serde_json::from_str::<Vec<PathBuf>>(&content) else {
        return Vec::new();
    };
    folders
        .into_iter()
        .filter(|path| path.is_dir())
        .take(RECENT_FOLDERS_LIMIT)
        .collect()
}

fn save_recent_folders(folders: &[PathBuf]) {
    let path = recent_folders_path();
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!("failed to create recent folders directory: {}", error);
            return;
        }
    }

    match serde_json::to_string_pretty(folders) {
        Ok(content) => {
            if let Err(error) = std::fs::write(path, content) {
                eprintln!("failed to save recent folders: {}", error);
            }
        }
        Err(error) => eprintln!("failed to serialize recent folders: {}", error),
    }
}

fn add_recent_folder(path: PathBuf) -> Vec<PathBuf> {
    let mut folders = load_recent_folders();
    folders.retain(|folder| folder != &path);
    folders.insert(0, path);
    folders.truncate(RECENT_FOLDERS_LIMIT);
    save_recent_folders(&folders);
    folders
}

#[derive(Clone, Debug)]
pub struct ConnectionSelected {
    pub name: String,
}

struct ConnectionSelector {
    input: Entity<InputState>,
    connections: Vec<DataSourceConfig>,
    filtered_indices: Vec<usize>,
    selected_ix: usize,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    _input_subscription: gpui::Subscription,
}

impl EventEmitter<ConnectionSelected> for ConnectionSelector {}

impl ConnectionSelector {
    fn new(
        connections: Vec<DataSourceConfig>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search connections..."));
        let input_subscription = cx.subscribe_in(&input, window, {
            move |this: &mut ConnectionSelector, _input, event: &InputEvent, _window, cx| {
                match event {
                    InputEvent::Change => this.filter_connections(cx),
                    InputEvent::PressEnter { .. } => this.confirm_selected(cx),
                    _ => {}
                }
            }
        });
        let filtered_indices = (0..connections.len()).collect();

        Self {
            input,
            connections,
            filtered_indices,
            selected_ix: 0,
            focus_handle,
            scroll_handle: ScrollHandle::default(),
            _input_subscription: input_subscription,
        }
    }

    fn filter_connections(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().trim().to_ascii_lowercase();
        if query.is_empty() {
            self.filtered_indices = (0..self.connections.len()).collect();
        } else {
            self.filtered_indices = self
                .connections
                .iter()
                .enumerate()
                .filter_map(|(ix, config)| {
                    config
                        .name
                        .to_ascii_lowercase()
                        .contains(&query)
                        .then_some(ix)
                })
                .collect();
        }
        self.selected_ix = 0;
        self.scroll_handle.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    fn select_previous(&mut self, cx: &mut Context<Self>) {
        if self.filtered_indices.is_empty() {
            return;
        }
        self.selected_ix = if self.selected_ix == 0 {
            self.filtered_indices.len() - 1
        } else {
            self.selected_ix - 1
        };
        self.scroll_selected_connection_into_view();
        cx.notify();
    }

    fn select_next(&mut self, cx: &mut Context<Self>) {
        if self.filtered_indices.is_empty() {
            return;
        }
        self.selected_ix = (self.selected_ix + 1) % self.filtered_indices.len();
        self.scroll_selected_connection_into_view();
        cx.notify();
    }

    fn scroll_selected_connection_into_view(&self) {
        let selected_top = px(self.selected_ix as f32 * CONNECTION_SELECTOR_ROW_HEIGHT);
        let selected_bottom = selected_top + px(CONNECTION_SELECTOR_ROW_HEIGHT);
        let viewport_height = px(CONNECTION_SELECTOR_LIST_HEIGHT);
        let mut offset = self.scroll_handle.offset();
        let visible_top = -offset.y;
        let visible_bottom = visible_top + viewport_height;

        if selected_top < visible_top {
            offset.y = -selected_top;
        } else if selected_bottom > visible_bottom {
            offset.y = -(selected_bottom - viewport_height);
        }

        self.scroll_handle.set_offset(offset);
    }

    fn confirm_selected(&mut self, cx: &mut Context<Self>) {
        let Some(&connection_ix) = self.filtered_indices.get(self.selected_ix) else {
            return;
        };
        let Some(config) = self.connections.get(connection_ix) else {
            return;
        };
        cx.emit(ConnectionSelected {
            name: config.name.clone(),
        });
    }

    fn input_focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }

    fn on_action_select_previous(
        &mut self,
        _: &SelectPreviousConnection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_previous(cx);
    }

    fn on_action_select_next(
        &mut self,
        _: &SelectNextConnection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_next(cx);
    }

    fn on_action_confirm(
        &mut self,
        _: &ConfirmSelectedConnection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm_selected(cx);
    }

    fn render_connection_row(
        &self,
        ix: usize,
        config: &DataSourceConfig,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let name = config.name.clone();
        let database = if config.database.is_empty() {
            config.host.clone()
        } else {
            config.database.clone()
        };
        let db_type = config.db_type.to_string();

        h_flex()
            .id(format!("connection-selector-row-{}", ix))
            .w_full()
            .h(px(CONNECTION_SELECTOR_ROW_HEIGHT))
            .gap_2()
            .items_center()
            .px_3()
            .py_2()
            .cursor_pointer()
            .when(selected, |this| {
                this.bg(cx.theme().accent)
                    .text_color(cx.theme().accent_foreground)
            })
            .when(!selected, |this| {
                this.text_color(cx.theme().foreground)
                    .hover(|style| style.bg(cx.theme().accent.opacity(0.12)))
            })
            .child(
                Icon::new(IconName::HardDrive)
                    .size_4()
                    .text_color(if selected {
                        cx.theme().accent_foreground
                    } else {
                        cx.theme().muted_foreground
                    }),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .text_sm()
                            .truncate()
                            .font_weight(if selected {
                                gpui::FontWeight::MEDIUM
                            } else {
                                gpui::FontWeight::NORMAL
                            })
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .text_color(if selected {
                                cx.theme().accent_foreground.opacity(0.7)
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(format!("{} / {}", db_type, database)),
                    ),
            )
            .on_click(
                cx.listener(move |this: &mut ConnectionSelector, _, _window, cx| {
                    this.selected_ix = ix;
                    this.confirm_selected(cx);
                }),
            )
            .into_any_element()
    }
}

impl Render for ConnectionSelector {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filtered = self.filtered_indices.clone();

        v_flex()
            .id("connection-selector")
            .key_context(CONNECTION_SELECTOR_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_action_select_previous))
            .on_action(cx.listener(Self::on_action_select_next))
            .on_action(cx.listener(Self::on_action_confirm))
            .w(px(460.))
            .max_h(px(360.))
            .overflow_hidden()
            .gap_2()
            .child(Input::new(&self.input))
            .child(
                div()
                    .w_full()
                    .h(px(CONNECTION_SELECTOR_LIST_HEIGHT))
                    .relative()
                    .overflow_hidden()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .id("connection-selector-scroll")
                            .size_full()
                            .track_scroll(&self.scroll_handle)
                            .overflow_y_scroll()
                            .when(filtered.is_empty(), |this| {
                                this.child(
                                    div()
                                        .px_3()
                                        .py_6()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("No matching connections"),
                                )
                            })
                            .children(filtered.iter().enumerate().filter_map(
                                |(ix, &connection_ix)| {
                                    self.connections.get(connection_ix).map(|config| {
                                        self.render_connection_row(
                                            ix,
                                            config,
                                            ix == self.selected_ix,
                                            cx,
                                        )
                                    })
                                },
                            )),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .child(Scrollbar::vertical(&self.scroll_handle)),
                    ),
            )
    }
}

impl Focusable for ConnectionSelector {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[derive(Clone, Debug)]
struct RecentFolderSelected {
    path: PathBuf,
}

struct RecentFolderSelector {
    folders: Vec<PathBuf>,
    selected_ix: usize,
    visible: bool,
    focus_handle: FocusHandle,
}

impl EventEmitter<RecentFolderSelected> for RecentFolderSelector {}

impl RecentFolderSelector {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            folders: load_recent_folders(),
            selected_ix: 0,
            visible: false,
            focus_handle: cx.focus_handle(),
        }
    }

    fn is_visible(&self) -> bool {
        self.visible
    }

    fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.folders = load_recent_folders();
        self.selected_ix = self.selected_ix.min(self.folders.len().saturating_sub(1));
        self.visible = true;
        cx.notify();
        window.focus(&self.focus_handle, cx);
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        self.visible = false;
        cx.notify();
    }

    fn set_folders(&mut self, folders: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.folders = folders;
        self.selected_ix = self.selected_ix.min(self.folders.len().saturating_sub(1));
        cx.notify();
    }

    fn select_previous(&mut self, cx: &mut Context<Self>) {
        if self.folders.is_empty() {
            return;
        }
        self.selected_ix = if self.selected_ix == 0 {
            self.folders.len() - 1
        } else {
            self.selected_ix - 1
        };
        cx.notify();
    }

    fn select_next(&mut self, cx: &mut Context<Self>) {
        if self.folders.is_empty() {
            return;
        }
        self.selected_ix = (self.selected_ix + 1).min(self.folders.len().saturating_sub(1));
        cx.notify();
    }

    fn confirm_selected(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.folders.get(self.selected_ix).cloned() else {
            return;
        };
        self.visible = false;
        cx.emit(RecentFolderSelected { path });
        cx.notify();
    }

    fn render_folder_row(
        &self,
        path: &Path,
        ix: usize,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        let parent = path
            .parent()
            .map(|parent| parent.to_string_lossy().to_string())
            .unwrap_or_default();

        div()
            .id(format!("recent-folder-row-{}", ix))
            .w_full()
            .px_3()
            .py_2()
            .cursor_pointer()
            .when(selected, |this| {
                this.bg(cx.theme().accent)
                    .text_color(cx.theme().accent_foreground)
            })
            .when(!selected, |this| {
                this.text_color(cx.theme().foreground)
                    .hover(|style| style.bg(cx.theme().accent.opacity(0.12)))
            })
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::Folder).small().text_color(if selected {
                        cx.theme().accent_foreground
                    } else {
                        cx.theme().muted_foreground
                    }))
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .text_sm()
                                    .truncate()
                                    .font_weight(if selected {
                                        gpui::FontWeight::MEDIUM
                                    } else {
                                        gpui::FontWeight::NORMAL
                                    })
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .truncate()
                                    .text_color(if selected {
                                        cx.theme().accent_foreground.opacity(0.7)
                                    } else {
                                        cx.theme().muted_foreground
                                    })
                                    .child(parent),
                            ),
                    ),
            )
            .on_click(
                cx.listener(move |this: &mut RecentFolderSelector, _, _window, cx| {
                    this.selected_ix = ix;
                    this.confirm_selected(cx);
                }),
            )
            .into_any_element()
    }
}

impl Render for RecentFolderSelector {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }

        v_flex()
            .id("recent-folders")
            .key_context(RECENT_FOLDERS_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_action_close))
            .on_action(cx.listener(Self::on_action_confirm))
            .on_action(cx.listener(Self::on_action_select_previous))
            .on_action(cx.listener(Self::on_action_select_next))
            .w(px(560.))
            .max_h(px(420.))
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .shadow_md()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Recent Folders"),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_y_scrollbar()
                    .when(self.folders.is_empty(), |this| {
                        this.child(
                            div()
                                .px_3()
                                .py_6()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No recent folders"),
                        )
                    })
                    .children(self.folders.iter().enumerate().map(|(ix, path)| {
                        self.render_folder_row(path, ix, ix == self.selected_ix, cx)
                    })),
            )
            .into_any_element()
    }
}

impl Focusable for RecentFolderSelector {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl RecentFolderSelector {
    fn on_action_close(
        &mut self,
        _: &CloseRecentFolders,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close(cx);
    }

    fn on_action_confirm(
        &mut self,
        _: &ConfirmRecentFolder,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm_selected(cx);
    }

    fn on_action_select_previous(
        &mut self,
        _: &SelectPreviousRecentFolder,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_previous(cx);
    }

    fn on_action_select_next(
        &mut self,
        _: &SelectNextRecentFolder,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_next(cx);
    }
}

pub struct Workspace {
    file_tree_panel: Entity<FileTreePanel>,
    file_search: Entity<FileSearch>,
    recent_folders: Entity<RecentFolderSelector>,
    project_search: Entity<ProjectSearch>,
    dock_area: Entity<DockArea>,
    editor_tabs: Entity<EditorTabs>,
    bottom_panel: Entity<BottomPanel>,
    data_source_manager: Entity<DataSourceManager>,
    query_sessions: QuerySessionStore,
    activity_tracker: Entity<ActivityTracker>,
    focus_handle: FocusHandle,
    terminal_panel: Entity<TerminalPanel>,
    bottom_panel_size: gpui::Pixels,
    connection_fingerprints: HashMap<String, String>,
}

impl Workspace {
    pub fn new(
        root_path: PathBuf,
        initial_file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let dock_area = cx.new(|cx| DockArea::new("main-dock", None, window, cx));

        let file_tree_panel = cx.new(|cx| FileTreePanel::new(root_path.clone(), window, cx));
        let file_search = cx.new(|cx| FileSearch::new(root_path.clone(), window, cx));
        let recent_folders = cx.new(|cx| RecentFolderSelector::new(cx));
        let project_search = cx.new(|cx| ProjectSearch::new(root_path.clone(), window, cx));
        let data_source_manager = cx.new(|_cx| {
            DataSourceManager::load().unwrap_or_else(|e| {
                eprintln!("failed to load data source config: {}", e);
                DataSourceManager::empty()
            })
        });
        let activity_tracker = cx.new(|_cx| ActivityTracker::new());
        let query_sessions = QuerySessionStore::new();

        cx.observe(&activity_tracker, |_, _, cx| {
            cx.notify();
        })
        .detach();

        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.save_open_editors(cx);
            }
        })
        .detach();

        // Subscribe to file open events from the file tree
        cx.subscribe_in(
            &file_tree_panel,
            window,
            |this, _file_tree, event: &OpenFileEvent, window, cx| {
                this.open_file(event.path.clone(), window, cx);
            },
        )
        .detach();

        // Subscribe to root changed events to clear editor tabs
        let file_search_for_root = file_search.clone();
        let project_search_for_root = project_search.clone();
        cx.subscribe_in(
            &file_tree_panel,
            window,
            move |this, _file_tree, _event: &RootChangedEvent, _window, cx| {
                this.editor_tabs.update(cx, |tabs, cx| {
                    tabs.clear_tabs(cx);
                });
                let root = this.file_tree_panel.read(cx).root().clone();
                this.terminal_panel.update(cx, |terminal, _cx| {
                    terminal.set_working_directory(root.clone());
                });
                file_search_for_root.update(cx, |search, cx| {
                    search.set_root(root.clone(), cx);
                });
                project_search_for_root.update(cx, |search, cx| {
                    search.set_root(root, cx);
                });
            },
        )
        .detach();

        // Track recently opened files from file tree
        let file_search_for_recent = file_search.clone();
        cx.subscribe_in(
            &file_tree_panel,
            window,
            move |_: &mut Workspace, _file_tree, event: &OpenFileEvent, _window, cx| {
                file_search_for_recent.update(cx, |search, cx| {
                    search.add_recent(event.path.clone(), cx);
                });
            },
        )
        .detach();

        let weak_dock_area = dock_area.downgrade();

        let editor_tabs = cx.new(|cx| {
            let mut tabs = EditorTabs::new(
                data_source_manager.clone(),
                activity_tracker.clone(),
                query_sessions.clone(),
                window,
                cx,
            );
            tabs.set_dock_area(weak_dock_area.clone());
            tabs
        });

        let file_tree_for_active_path = file_tree_panel.clone();
        cx.observe(&editor_tabs, move |_, editor_tabs, cx| {
            let active_path = editor_tabs.read(cx).active_path(cx);
            file_tree_for_active_path.update(cx, |tree, cx| {
                tree.set_active_editor_path(active_path, cx);
            });
        })
        .detach();

        // Subscribe to file search results (after editor_tabs is created)
        let editor_tabs_for_focus = editor_tabs.clone();
        cx.subscribe_in(&file_search, window, {
            move |this: &mut Workspace, _file_search, event: &FileSearchEvent, window, cx| {
                match event {
                    FileSearchEvent::OpenFile(path) => {
                        this.open_file(path.clone(), window, cx);
                        // Defer focus to active editor's inner input after render cycle
                        let editor_tabs = editor_tabs_for_focus.clone();
                        cx.defer_in(window, move |_, window, cx| {
                            if let Some(editor) = editor_tabs.read(cx).active_editor() {
                                let input_focus = editor.read(cx).editor_focus_handle(cx);
                                window.focus(&input_focus, cx);
                            }
                        });
                    }
                    FileSearchEvent::Closed => {
                        // Restore focus to active editor when search is closed
                        let editor_tabs = editor_tabs_for_focus.clone();
                        cx.defer_in(window, move |_, window, cx| {
                            if let Some(editor) = editor_tabs.read(cx).active_editor() {
                                let input_focus = editor.read(cx).editor_focus_handle(cx);
                                window.focus(&input_focus, cx);
                            }
                        });
                    }
                }
            }
        })
        .detach();

        cx.subscribe_in(
            &recent_folders,
            window,
            |this, _recent_folders, event: &RecentFolderSelected, _window, cx| {
                this.set_workspace_root(event.path.clone(), cx);
            },
        )
        .detach();

        // Subscribe to project search results
        let editor_tabs_for_project = editor_tabs.clone();
        cx.subscribe_in(&project_search, window, {
            move |this: &mut Workspace, _project_search, event: &ProjectSearchEvent, window, cx| {
                match event {
                    ProjectSearchEvent::OpenFileAtPosition(path, line_number, column) => {
                        this.open_file_at_position(path.clone(), *line_number, *column, window, cx);
                        // Defer focus to active editor's inner input after render cycle
                        let editor_tabs = editor_tabs_for_project.clone();
                        cx.defer_in(window, move |_, window, cx| {
                            if let Some(editor) = editor_tabs.read(cx).active_editor() {
                                let input_focus = editor.read(cx).editor_focus_handle(cx);
                                window.focus(&input_focus, cx);
                            }
                        });
                    }
                    ProjectSearchEvent::Closed => {
                        let editor_tabs = editor_tabs_for_project.clone();
                        cx.defer_in(window, move |_, window, cx| {
                            if let Some(editor) = editor_tabs.read(cx).active_editor() {
                                let input_focus = editor.read(cx).editor_focus_handle(cx);
                                window.focus(&input_focus, cx);
                            }
                        });
                    }
                }
            }
        })
        .detach();

        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        // Set up left dock: file tree (panel mode to avoid title bar)
        let left_panels = DockItem::panel(Arc::new(file_tree_panel.clone()));

        // Set up center dock with our custom editor tabs
        let center_panels = DockItem::panel(Arc::new(editor_tabs.clone()));

        // Set up right dock: database connections
        let database_panel = cx.new(|cx| {
            ConnectionPanel::new(
                data_source_manager.clone(),
                activity_tracker.clone(),
                window,
                cx,
            )
        });
        cx.subscribe_in(
            &database_panel,
            window,
            |this, _panel, event: &ShowDiagramEvent, window, cx| {
                this.open_diagram(event.clone(), window, cx);
            },
        )
        .detach();
        cx.subscribe_in(
            &database_panel,
            window,
            |this, _panel, event: &ShowDataEditorEvent, window, cx| {
                this.open_data_editor(event.clone(), window, cx);
            },
        )
        .detach();
        let right_panels = DockItem::panel(Arc::new(database_panel));

        let results_panel = cx.new(|cx| {
            let mut panel = ResultPanel::new(activity_tracker.clone(), window, cx);
            panel.set_dock_area(weak_dock_area.clone());
            panel
        });

        let terminal_panel = cx.new(|cx| {
            let mut panel = TerminalPanel::new(root_path.clone(), window, cx);
            panel.set_dock_area(weak_dock_area.clone());
            panel
        });

        let bottom_panel = cx.new(|cx| {
            let mut panel = BottomPanel::new(results_panel, terminal_panel.clone(), cx);
            panel.set_dock_area(weak_dock_area.clone(), cx);
            panel
        });

        // Set up bottom dock state. The dock itself starts hidden and is opened
        // once a query runs or the user explicitly toggles it from the bottom bar.
        let bottom_panel_size = px(200.);

        dock_area.update(cx, |dock_area, cx| {
            dock_area.set_center(center_panels, window, cx);
            dock_area.set_left_dock(left_panels, Some(px(240.)), true, window, cx);
            dock_area.set_right_dock(right_panels, Some(px(260.)), true, window, cx);
            dock_area.set_dock_collapsible(
                gpui::Edges {
                    left: true,
                    bottom: false,
                    right: true,
                    ..Default::default()
                },
                window,
                cx,
            );
        });

        let mut this = Self {
            file_tree_panel,
            file_search,
            recent_folders,
            project_search,
            dock_area,
            editor_tabs,
            bottom_panel,
            data_source_manager: data_source_manager.clone(),
            query_sessions,
            activity_tracker,
            focus_handle,
            terminal_panel,
            bottom_panel_size,
            connection_fingerprints: connection_fingerprints(
                data_source_manager.read(cx).configs(),
            ),
        };

        cx.observe(&this.data_source_manager, |this, manager, cx| {
            let next = connection_fingerprints(manager.read(cx).configs());
            let stale_names = this
                .connection_fingerprints
                .iter()
                .filter_map(|(name, fingerprint)| {
                    (next.get(name) != Some(fingerprint)).then_some(name.clone())
                })
                .collect::<Vec<_>>();
            this.connection_fingerprints = next;
            for name in stale_names {
                let query_sessions = this.query_sessions.clone();
                cx.spawn(async move |_this, _cx| {
                    if let Err(error) = query_sessions.close_connection_name(name).await {
                        eprintln!("failed to close stale query sessions: {}", error);
                    }
                })
                .detach();
            }
        })
        .detach();

        cx.subscribe_in(
            &this.terminal_panel,
            window,
            |this, _terminal, _event: &gpui_component::dock::PanelEvent, _window, cx| {
                let sessions_count = this.terminal_panel.read(cx).sessions_count();
                if sessions_count == 0 {
                    this.bottom_panel.update(cx, |panel, cx| {
                        panel.set_mode(BottomPanelMode::Results, cx);
                    });
                }
            },
        )
        .detach();

        if let Some(file) = initial_file {
            this.open_file(file, window, cx);
        }
        let folders = add_recent_folder(root_path);
        this.recent_folders.update(cx, |recent, cx| {
            recent.set_folders(folders, cx);
        });

        this
    }

    fn set_workspace_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        let folders = add_recent_folder(root.clone());
        self.recent_folders.update(cx, |recent, cx| {
            recent.set_folders(folders, cx);
        });
        self.file_tree_panel.update(cx, |tree, cx| {
            tree.set_root(root, cx);
        });
    }

    fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_tabs.update(cx, |editor_tabs, cx| {
            editor_tabs.open_file(path, window, cx);
        });
    }

    fn open_file_at_position(
        &mut self,
        path: PathBuf,
        line_number: usize,
        column: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor_tabs.update(cx, |editor_tabs, cx| {
            editor_tabs.open_file_at_position(path, line_number, column, window, cx);
        });
    }

    fn open_diagram(
        &mut self,
        event: ShowDiagramEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cache_key = schema_cache::cache_key(&event.config);
        let Ok(Some(schema)) = schema_cache::load(&cache_key) else {
            window.open_alert_dialog(cx, |alert, _, _| {
                alert
                    .title("Schema Not Cached")
                    .child("Refresh schema before showing a diagram.")
            });
            return;
        };

        let model = DiagramModel::build(&event.config, &schema, event.scope);
        self.editor_tabs.update(cx, |tabs, cx| {
            tabs.open_diagram(model, window, cx);
        });
    }

    fn open_data_editor(
        &mut self,
        event: ShowDataEditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cache_key = schema_cache::cache_key(&event.config);
        let Ok(Some(schema)) = schema_cache::load(&cache_key) else {
            window.open_alert_dialog(cx, |alert, _, _| {
                alert
                    .title("Schema Not Cached")
                    .child("Refresh schema before editing table data.")
            });
            return;
        };

        let Some(table) = schema
            .tables
            .iter()
            .find(|table| table.schema == event.schema && table.name == event.table)
            .cloned()
        else {
            window.open_alert_dialog(cx, |alert, _, _| {
                alert
                    .title("Table Not Found")
                    .child("Refresh schema before editing table data.")
            });
            return;
        };

        self.editor_tabs.update(cx, |tabs, cx| {
            tabs.open_data_editor(event.config, table, window, cx);
        });
    }

    pub(crate) fn open_folder_picker(&mut self, cx: &mut Context<Self>) {
        let options = gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Folder".into()),
        };
        let rx = cx.prompt_for_paths(options);
        let file_tree = self.file_tree_panel.clone();
        let recent_folders = self.recent_folders.clone();
        cx.spawn(async move |_this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(path) = paths.first() {
                    let folders = add_recent_folder(path.clone());
                    cx.update_entity(&recent_folders, |recent, cx| {
                        recent.set_folders(folders, cx);
                    });
                    cx.update_entity(&file_tree, |tree, cx| {
                        tree.set_root(path.clone(), cx);
                    });
                }
            }
        })
        .detach();
    }

    fn on_open_folder(&mut self, _: &OpenFolder, _window: &mut Window, cx: &mut Context<Self>) {
        self.open_folder_picker(cx);
    }

    pub(crate) fn open_recent_folders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.file_search.update(cx, |search, cx| {
            search.close(window, cx);
        });
        self.project_search.update(cx, |search, cx| {
            search.close(window, cx);
        });
        self.recent_folders.update(cx, |recent, cx| {
            recent.open(window, cx);
        });
    }

    fn on_open_recent_folders(
        &mut self,
        _: &OpenRecentFolders,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_recent_folders(window, cx);
    }

    fn on_save_file(&mut self, _: &SaveFile, _window: &mut Window, cx: &mut Context<Self>) {
        self.save_active_editor(cx);
    }

    fn save_active_editor(&mut self, cx: &mut Context<Self>) {
        self.editor_tabs.update(cx, |tabs, cx| {
            if let Some(editor) = tabs.active_editor() {
                editor.update(cx, |editor, cx| {
                    editor.save(cx);
                });
            }
        });
    }

    fn save_open_editors(&mut self, cx: &mut Context<Self>) {
        self.editor_tabs.update(cx, |tabs, cx| {
            tabs.save_all(cx);
        });
    }

    fn on_execute_query(&mut self, _: &ExecuteQuery, window: &mut Window, cx: &mut Context<Self>) {
        let Some((
            path,
            selected_connection_name,
            active_editor,
            selected,
            active_queries,
            has_selection,
            search_path,
        )) = self.editor_tabs.read(cx).active_editor().map(|editor| {
            let editor_entity = editor.clone();
            let editor = editor.read(cx);
            let (selected, active_queries) = editor.query_context(cx);
            (
                editor.path().clone(),
                editor.selected_connection_name().map(str::to_string),
                editor_entity,
                selected,
                active_queries,
                editor.has_nonempty_selection(cx),
                editor.selected_search_path(),
            )
        })
        else {
            window.open_alert_dialog(cx, |alert, _, _| {
                alert
                    .title("No Active Editor")
                    .child("Open a SQL file before executing a query.")
            });
            return;
        };

        let queries: Vec<QueryChoice> = if let Some(selected) = selected {
            vec![QueryChoice {
                query: selected.text.clone(),
                range: Some(selected),
            }]
        } else {
            active_queries
                .into_iter()
                .map(|query| QueryChoice {
                    query: query.text.clone(),
                    range: Some(query),
                })
                .collect()
        };

        if queries.is_empty() {
            window.open_alert_dialog(cx, |alert, _, _| {
                alert
                    .title("No Query Detected")
                    .child("Place the cursor inside a SQL statement or select query text.")
            });
            return;
        }

        if queries.len() == 1 {
            let Some(query) = queries.first() else {
                return;
            };
            self.execute_single_query(
                path,
                selected_connection_name,
                query.query.clone(),
                query.range.clone(),
                Some(active_editor),
                search_path,
                window,
                cx,
            );
            return;
        }

        if has_selection {
            let config = selected_connection_name.and_then(|name| {
                self.data_source_manager
                    .read(cx)
                    .configs()
                    .iter()
                    .find(|config| config.name == name)
                    .cloned()
            });
            let Some(config) = config else {
                self.show_connection_selector_for_all(
                    path,
                    queries,
                    active_editor,
                    search_path,
                    window,
                    cx,
                );
                return;
            };
            self.execute_all_with_config(
                path,
                queries,
                active_editor,
                config,
                search_path,
                window,
                cx,
            );
            return;
        }

        let selector = cx.new(|cx| QuerySelector::new(queries, cx));
        let selector_search_path = search_path.clone();
        let selector_path = path.clone();
        let selector_connection_name = selected_connection_name.clone();
        cx.subscribe_in(
            &selector,
            window,
            move |this, _selector, event: &QuerySelected, window, cx| {
                let highlighted_editor = this
                    .editor_tabs
                    .update(cx, |tabs, _cx| tabs.active_editor().cloned());
                if let Some(editor) = highlighted_editor {
                    editor.update(cx, |editor, cx| {
                        editor.override_query_decoration(event.choice.range.clone(), cx);
                    });
                }

                if event.confirmed {
                    window.close_dialog(cx);
                    this.execute_single_query(
                        selector_path.clone(),
                        selector_connection_name.clone(),
                        event.choice.query.clone(),
                        event.choice.range.clone(),
                        Some(active_editor.clone()),
                        selector_search_path.clone(),
                        window,
                        cx,
                    );
                }
            },
        )
        .detach();

        window.open_alert_dialog(cx, {
            let selector = selector.clone();
            move |alert, _window, _cx| {
                alert
                    .title("Choose Query")
                    .child(selector.clone())
                    .footer(div())
                    .close_button(true)
            }
        });
        window.focus(&selector.read(cx).focus_handle(cx), cx);
    }

    fn show_connection_selector_for_all(
        &mut self,
        path: PathBuf,
        queries: Vec<QueryChoice>,
        editor: Entity<EditorPanel>,
        search_path: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let connections = self.data_source_manager.read(cx).configs().to_vec();
        if connections.is_empty() {
            window.open_alert_dialog(cx, |alert, _, _| {
                alert
                    .title("No Connections")
                    .child("Create a database connection before running queries.")
            });
            return;
        }

        let selector = cx.new(|cx| ConnectionSelector::new(connections, window, cx));
        let selector_search_path = search_path.clone();
        let selector_path = path.clone();
        cx.subscribe_in(
            &selector,
            window,
            move |this, _selector, event: &ConnectionSelected, window, cx| {
                let name = event.name.clone();
                window.close_dialog(cx);
                if let Some(config) = this.prepare_query_connection(name.clone(), window, cx) {
                    if let Some(editor) = this.editor_tabs.read(cx).active_editor().cloned()
                        && editor.read(cx).path() == &selector_path
                    {
                        editor.update(cx, |editor, cx| {
                            editor.set_selected_connection_name(Some(name.clone()), cx);
                        });
                    }
                    this.execute_all_with_config(
                        selector_path.clone(),
                        queries.clone(),
                        editor.clone(),
                        config,
                        selector_search_path.clone(),
                        window,
                        cx,
                    );
                }
            },
        )
        .detach();

        window.open_alert_dialog(cx, {
            let selector = selector.clone();
            move |alert, _window, _cx| {
                alert
                    .title("Choose Connection")
                    .child(selector.clone())
                    .footer(div())
                    .close_button(true)
            }
        });
        window.focus(&selector.read(cx).input_focus_handle(cx), cx);
    }

    fn execute_all_with_config(
        &mut self,
        path: PathBuf,
        queries: Vec<QueryChoice>,
        editor: Entity<EditorPanel>,
        config: DataSourceConfig,
        search_path: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let results_panel = self.bottom_panel.read(cx).results_panel().clone();
        let bottom_panel = self.bottom_panel.clone();
        let data_source_manager = self.data_source_manager.clone();
        let query_sessions = self.query_sessions.clone();
        let activity_tracker = self.activity_tracker.clone();
        let mut config_for_result = config.clone();
        if let Some(search_path) = search_path.as_deref() {
            config_for_result.schema = search_path.to_string();
        }
        let config_name = config.name.clone();
        let activity_id = self
            .activity_tracker
            .update(cx, |tracker, cx| tracker.begin("Running queries", cx));
        let editor_for_task = editor.clone();
        let search_path_for_task = search_path.clone();
        self.show_bottom_panel(BottomPanelMode::Results, false, window, cx);

        cx.spawn(async move |_this, cx| {
            for query in queries {
                let execution_marker_id = cx.update_entity(&editor_for_task, |editor, cx| {
                    editor.begin_query_execution(query.range.clone(), cx)
                });

                let query_text = query.query.clone();
                let execution_options = QueryExecutionOptions {
                    search_path: search_path_for_task.clone(),
                };
                let path_for_task = path.clone();
                let config_for_task = config.clone();
                let query_sessions_for_task = query_sessions.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        query_sessions_for_task
                            .execute_query(
                                path_for_task,
                                config_for_task,
                                execution_options,
                                query_text,
                            )
                            .await
                    })
                    .await;

                let (result, succeeded, connection_failed) = match result {
                    Ok(result) => (result, true, false),
                    Err(error) => {
                        let is_conn_fail = matches!(error, DataSourceError::ConnectionFailed(_));
                        (error_result(error), false, is_conn_fail)
                    }
                };

                cx.update_entity(&results_panel, |panel, cx| {
                    panel.set_result(
                        query.query.clone(),
                        result,
                        succeeded,
                        Some(config_for_result.clone()),
                        cx,
                    );
                });

                cx.update_entity(&bottom_panel, |panel, cx| {
                    panel.set_mode(BottomPanelMode::Results, cx);
                });

                if let Some(marker_id) = execution_marker_id {
                    cx.update_entity(&editor_for_task, |editor, cx| {
                        editor.finish_query_execution(Some(marker_id), succeeded, cx);
                    });
                }

                let config_name_for_closure = config_name.clone();
                cx.update_entity(&data_source_manager, move |manager, cx| {
                    if connection_failed {
                        manager.set_status(&config_name_for_closure, ConnectionStatus::Failed);
                    } else {
                        manager.set_status(&config_name_for_closure, ConnectionStatus::Connected);
                    }
                    cx.notify();
                });

                if !succeeded {
                    break;
                }
            }

            cx.update_entity(&activity_tracker, |tracker, cx| {
                tracker.finish(activity_id, cx);
            });
        })
        .detach();
    }

    fn execute_single_query(
        &mut self,
        path: PathBuf,
        selected_connection_name: Option<String>,
        query: String,
        range: Option<QueryRange>,
        editor: Option<Entity<EditorPanel>>,
        search_path: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let config = selected_connection_name.and_then(|name| {
            self.data_source_manager
                .read(cx)
                .configs()
                .iter()
                .find(|config| config.name == name)
                .cloned()
        });

        let Some(config) = config else {
            self.show_connection_selector(path, query, range, editor, search_path, window, cx);
            return;
        };

        self.execute_query_with_config(path, query, range, editor, config, search_path, window, cx);
    }

    fn show_connection_selector(
        &mut self,
        path: PathBuf,
        query: String,
        range: Option<QueryRange>,
        editor: Option<Entity<EditorPanel>>,
        search_path: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let connections = self.data_source_manager.read(cx).configs().to_vec();
        if connections.is_empty() {
            window.open_alert_dialog(cx, |alert, _, _| {
                alert
                    .title("No Connections")
                    .child("Create a database connection before running queries.")
            });
            return;
        }

        let selector = cx.new(|cx| ConnectionSelector::new(connections, window, cx));
        let selector_search_path = search_path.clone();
        let selector_path = path.clone();
        cx.subscribe_in(
            &selector,
            window,
            move |this, _selector, event: &ConnectionSelected, window, cx| {
                let name = event.name.clone();
                window.close_dialog(cx);
                if let Some(config) = this.prepare_query_connection(name.clone(), window, cx) {
                    if let Some(editor) = this.editor_tabs.read(cx).active_editor().cloned()
                        && editor.read(cx).path() == &selector_path
                    {
                        editor.update(cx, |editor, cx| {
                            editor.set_selected_connection_name(Some(name.clone()), cx);
                        });
                    }
                    this.execute_query_with_config(
                        selector_path.clone(),
                        query.clone(),
                        range.clone(),
                        editor.clone(),
                        config,
                        selector_search_path.clone(),
                        window,
                        cx,
                    );
                }
            },
        )
        .detach();

        window.open_alert_dialog(cx, {
            let selector = selector.clone();
            move |alert, _window, _cx| {
                alert
                    .title("Choose Connection")
                    .child(selector.clone())
                    .footer(div())
                    .close_button(true)
            }
        });
        window.focus(&selector.read(cx).input_focus_handle(cx), cx);
    }

    fn prepare_query_connection(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<DataSourceConfig> {
        let result = self.data_source_manager.update(cx, |manager, cx| {
            manager.set_active(Some(name.clone()));
            if manager.status(&name) != ConnectionStatus::Connected {
                manager.set_status(&name, ConnectionStatus::Idle);
            }
            manager.clear_last_error(&name);

            if let Err(error) = manager.ensure_password_loaded(&name, |n| {
                credentials::load_password(n).map_err(|e| credentials::recovery_error_message(&e))
            }) {
                manager.set_status(&name, ConnectionStatus::Failed);
                manager.set_last_error(&name, error.clone());
                cx.notify();
                return Err(error);
            }

            let config = manager
                .configs()
                .iter()
                .find(|config| config.name == name)
                .cloned()
                .ok_or_else(|| "The selected connection no longer exists.".to_string());
            cx.notify();
            config
        });

        match result {
            Ok(config) => Some(config),
            Err(error) => {
                window.open_alert_dialog(cx, move |alert, _, _| {
                    alert
                        .title("Connection Setup Failed")
                        .description(error.clone())
                });
                None
            }
        }
    }

    fn execute_query_with_config(
        &mut self,
        path: PathBuf,
        query: String,
        range: Option<QueryRange>,
        editor: Option<Entity<EditorPanel>>,
        config: DataSourceConfig,
        search_path: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let results_panel = self.bottom_panel.read(cx).results_panel().clone();
        let bottom_panel = self.bottom_panel.clone();
        let data_source_manager = self.data_source_manager.clone();
        let query_sessions = self.query_sessions.clone();
        let activity_tracker = self.activity_tracker.clone();
        let mut config_for_result = config.clone();
        if let Some(search_path) = search_path.as_deref() {
            config_for_result.schema = search_path.to_string();
        }
        let config_name = config.name.clone();
        let execution_options = QueryExecutionOptions { search_path };
        let activity_id = self
            .activity_tracker
            .update(cx, |tracker, cx| tracker.begin("Running query", cx));
        let execution_marker_id = editor.as_ref().and_then(|editor| {
            editor.update(cx, |editor, cx| {
                editor.begin_query_execution(range.clone(), cx)
            })
        });
        self.show_bottom_panel(BottomPanelMode::Results, false, window, cx);

        cx.spawn(async move |_this, cx| {
            let query_for_task = query.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    query_sessions
                        .execute_query(path, config, execution_options, query_for_task)
                        .await
                })
                .await;

            let (result, succeeded, connection_failed) = match result {
                Ok(result) => (result, true, false),
                Err(error) => {
                    let is_conn_fail = matches!(error, DataSourceError::ConnectionFailed(_));
                    (error_result(error), false, is_conn_fail)
                }
            };

            let open_result_tab = should_open_result_tab(succeeded, &result);
            if open_result_tab {
                cx.update_entity(&results_panel, |panel, cx| {
                    panel.set_result(query, result, succeeded, Some(config_for_result), cx);
                });
            }

            if let Some(editor) = editor {
                cx.update_entity(&editor, |editor, cx| {
                    editor.finish_query_execution(execution_marker_id, succeeded, cx);
                });
            }

            cx.update_entity(&bottom_panel, |panel, cx| {
                panel.set_mode(BottomPanelMode::Results, cx);
            });

            cx.update_entity(&activity_tracker, |tracker, cx| {
                tracker.finish(activity_id, cx);
            });

            cx.update_entity(&data_source_manager, move |manager, cx| {
                if connection_failed {
                    manager.set_status(&config_name, ConnectionStatus::Failed);
                } else {
                    manager.set_status(&config_name, ConnectionStatus::Connected);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn on_toggle_file_search(
        &mut self,
        _: &ToggleFileSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.recent_folders.update(cx, |recent, cx| {
            recent.close(cx);
        });
        self.file_search.update(cx, |search, cx| {
            search.toggle(window, cx);
        });
    }

    fn on_toggle_project_search(
        &mut self,
        _: &ToggleProjectSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.recent_folders.update(cx, |recent, cx| {
            recent.close(cx);
        });
        self.project_search.update(cx, |search, cx| {
            search.toggle(window, cx);
        });
    }

    fn on_toggle_search_replace(
        &mut self,
        _: &ToggleSearchReplace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project_search.read(cx).is_visible() {
            self.project_search.update(cx, |search, cx| {
                search.toggle_replace(window, cx);
            });
            return;
        }

        self.editor_tabs.update(cx, |tabs, cx| {
            tabs.toggle_replace_in_active_editor(window, cx);
        });
    }

    fn on_toggle_bottom_panel_mode(
        &mut self,
        _action: &ToggleBottomPanelMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut is_open = false;
        self.dock_area.update(cx, |dock_area, cx| {
            is_open = dock_area.is_dock_open(gpui_component::dock::DockPlacement::Bottom, cx);
        });

        if is_open {
            self.bottom_panel.update(cx, |panel, cx| {
                let new_mode = match panel.mode() {
                    BottomPanelMode::Terminal => BottomPanelMode::Results,
                    BottomPanelMode::Results => BottomPanelMode::Terminal,
                };
                panel.set_mode(new_mode, cx);
                window.focus(&panel.focus_handle(cx), cx);
            });
            return;
        }

        self.toggle_bottom_panel(BottomPanelMode::Terminal, window, cx);
    }

    fn toggle_bottom_panel(
        &mut self,
        mode: BottomPanelMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_open = self
            .dock_area
            .read(cx)
            .is_dock_open(DockPlacement::Bottom, cx);
        let current_mode = self.bottom_panel.read(cx).mode();

        if is_open && current_mode == mode {
            self.dock_area.update(cx, |dock_area, cx| {
                if let Some(bottom_dock) = dock_area.bottom_dock() {
                    self.bottom_panel_size = bottom_dock.read(cx).size();
                }
                dock_area.remove_bottom_dock(window, cx);
            });
            return;
        }

        self.show_bottom_panel(mode, true, window, cx);
    }

    fn show_bottom_panel(
        &mut self,
        mode: BottomPanelMode,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_open = self
            .dock_area
            .read(cx)
            .is_dock_open(DockPlacement::Bottom, cx);

        self.bottom_panel.update(cx, |panel, cx| {
            panel.set_mode(mode, cx);
        });

        if !is_open {
            let bottom_panel = self.bottom_panel.clone();
            let bottom_panel_size = self.bottom_panel_size;
            self.dock_area.update(cx, |dock_area, cx| {
                dock_area.set_bottom_dock(
                    DockItem::panel(Arc::new(bottom_panel)),
                    Some(bottom_panel_size),
                    true,
                    window,
                    cx,
                );
            });
        }

        if mode == BottomPanelMode::Terminal {
            self.terminal_panel.update(cx, |panel, cx| {
                panel.ensure_has_tab(window, cx);
            });
        }

        if focus {
            self.bottom_panel.update(cx, |panel, cx| {
                window.focus(&panel.focus_handle(cx), cx);
            });
        }
    }

    fn restore_zoomed_panels(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let editor_zoomed = self.editor_tabs.read(cx).is_zoomed();
        let bottom_zoomed = self.bottom_panel.read(cx).is_zoomed(cx);

        if editor_zoomed {
            self.editor_tabs.update(cx, |tabs, cx| {
                tabs.set_zoomed(false, window, cx);
            });
        }

        if bottom_zoomed {
            self.bottom_panel.update(cx, |panel, cx| {
                panel.set_zoomed(false, window, cx);
            });
        }

        editor_zoomed || bottom_zoomed
    }

    fn toggle_side_dock_from_bottom_bar(
        &mut self,
        placement: DockPlacement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dock_area.update(cx, |dock_area, cx| {
            dock_area.toggle_dock(placement, window, cx);
        });
        self.editor_tabs.update(cx, |tabs, cx| {
            tabs.sync_zoomed_side_docks(cx);
        });
        self.bottom_panel.update(cx, |panel, cx| {
            panel.sync_zoomed_side_docks(cx);
        });
    }

    fn toggle_bottom_panel_from_bottom_bar(
        &mut self,
        mode: BottomPanelMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.restore_zoomed_panels(window, cx) {
            self.show_bottom_panel(mode, true, window, cx);
        } else {
            self.toggle_bottom_panel(mode, window, cx);
        }
    }

    fn render_bottom_bar(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (is_busy, activity_label, activity_count) = {
            let tracker = self.activity_tracker.read(cx);
            (
                tracker.is_busy(),
                tracker.label().to_string(),
                tracker.count(),
            )
        };
        let activity_label = if activity_count > 1 {
            format!("{} (+{})", activity_label, activity_count - 1)
        } else {
            activity_label
        };

        let bottom_panel_mode = self.bottom_panel.read(cx).mode();
        let is_terminal_active = bottom_panel_mode == BottomPanelMode::Terminal;
        let is_results_active = bottom_panel_mode == BottomPanelMode::Results;
        let is_editor_zoomed = self.editor_tabs.read(cx).is_zoomed();
        let is_left_open = self
            .dock_area
            .read(cx)
            .is_dock_open(DockPlacement::Left, cx);
        let is_right_open = self
            .dock_area
            .read(cx)
            .is_dock_open(DockPlacement::Right, cx);
        let is_dock_open = self
            .dock_area
            .read(cx)
            .is_dock_open(DockPlacement::Bottom, cx)
            && !is_editor_zoomed;
        let active_bottom_button_fg = if cx.theme().is_dark() {
            hsla(0.74, 0.78, 0.58, 1.0)
        } else {
            hsla(0.74, 0.78, 0.42, 1.0)
        };

        h_flex()
            .id("workspace-bottom-bar")
            .h(px(24.))
            .flex_none()
            .items_center()
            .justify_between()
            .px_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().tab_bar)
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(
                Button::new("toggle-left-dock")
                    .icon(if is_left_open {
                        IconName::PanelLeft
                    } else {
                        IconName::PanelLeftOpen
                    })
                    .xsmall()
                    .ghost()
                    .tooltip(if is_left_open {
                        "Collapse Left Panel"
                    } else {
                        "Expand Left Panel"
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_side_dock_from_bottom_bar(DockPlacement::Left, window, cx);
                    })),
            )
            .child(div().flex_1())
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(div().child(activity_label))
                            .child(if is_busy {
                                Spinner::new()
                                    .xsmall()
                                    .color(cx.theme().muted_foreground)
                                    .into_any_element()
                            } else {
                                div().size(px(12.)).into_any_element()
                            }),
                    )
                    .child({
                        let btn = Button::new("results-toggle")
                            .icon(Icon::new(IconName::File).path("icons/results-table.svg"))
                            .xsmall()
                            .ghost()
                            .tooltip("Query Results");

                        let btn = if is_results_active && is_dock_open {
                            btn.text_color(active_bottom_button_fg)
                        } else {
                            btn
                        };

                        btn.on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_bottom_panel_from_bottom_bar(
                                BottomPanelMode::Results,
                                window,
                                cx,
                            );
                        }))
                    })
                    .child({
                        let btn = Button::new("terminal-toggle")
                            .icon(Icon::new(IconName::File).path("icons/square-terminal.svg"))
                            .xsmall()
                            .ghost()
                            .tooltip("Terminal");

                        let btn = if is_terminal_active && is_dock_open {
                            btn.text_color(active_bottom_button_fg)
                        } else {
                            btn
                        };

                        btn.on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_bottom_panel_from_bottom_bar(
                                BottomPanelMode::Terminal,
                                window,
                                cx,
                            );
                        }))
                    })
                    .child(
                        Button::new("toggle-right-dock")
                            .icon(if is_right_open {
                                IconName::PanelRight
                            } else {
                                IconName::PanelRightOpen
                            })
                            .xsmall()
                            .ghost()
                            .tooltip(if is_right_open {
                                "Collapse Right Panel"
                            } else {
                                "Expand Right Panel"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_side_dock_from_bottom_bar(
                                    DockPlacement::Right,
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_dark = cx.theme().is_dark();
        let theme_icon = if is_dark {
            IconName::Sun
        } else {
            IconName::Moon
        };

        let is_file_search_visible = self.file_search.read(cx).is_visible();
        let is_recent_folders_visible = self.recent_folders.read(cx).is_visible();
        let is_project_search_visible = self.project_search.read(cx).is_visible();

        v_flex()
            .id("workspace")
            .size_full()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_open_folder))
            .on_action(cx.listener(Self::on_open_recent_folders))
            .on_action(cx.listener(Self::on_save_file))
            .on_action(cx.listener(Self::on_execute_query))
            .on_action(cx.listener(Self::on_toggle_file_search))
            .on_action(cx.listener(Self::on_toggle_project_search))
            .on_action(cx.listener(Self::on_toggle_search_replace))
            .on_action(cx.listener(Self::on_toggle_bottom_panel_mode))
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .child(
                            h_flex()
                                .gap_0()
                                .text_color(cx.theme().foreground)
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child("sq")
                                .child(
                                    div()
                                        .text_color(cx.theme().primary)
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .child("/"),
                                )
                                .child("lab"),
                        )
                        .child(
                            Button::new("theme-toggle")
                                .icon(theme_icon)
                                .small()
                                .ghost()
                                .tooltip(if is_dark {
                                    "Switch to Light"
                                } else {
                                    "Switch to Dark"
                                })
                                .on_click(move |_event, window, cx| {
                                    let new_mode = if is_dark {
                                        gpui_component::ThemeMode::Light
                                    } else {
                                        gpui_component::ThemeMode::Dark
                                    };
                                    gpui_component::Theme::change(new_mode, Some(window), cx);
                                }),
                        ),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .relative()
                    .child(self.dock_area.clone())
                    .when(is_recent_folders_visible, |overlay| {
                        overlay
                            .child(
                                div()
                                    .absolute()
                                    .size_full()
                                    .inset_0()
                                    .occlude()
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| {
                                            this.recent_folders.update(cx, |recent, cx| {
                                                recent.close(cx);
                                            });
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .size_full()
                                    .top(px(80.))
                                    .flex()
                                    .justify_center()
                                    .items_start()
                                    .child(
                                        div()
                                            .occlude()
                                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation();
                                            })
                                            .child(self.recent_folders.clone()),
                                    ),
                            )
                    })
                    .when(is_file_search_visible, |overlay| {
                        overlay
                            .child(
                                div()
                                    .absolute()
                                    .size_full()
                                    .inset_0()
                                    .occlude()
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            this.file_search.update(cx, |search, cx| {
                                                search.close(window, cx);
                                            });
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .size_full()
                                    .top(px(80.))
                                    .flex()
                                    .justify_center()
                                    .items_start()
                                    .child(
                                        div()
                                            .occlude()
                                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation();
                                            })
                                            .child(self.file_search.clone()),
                                    ),
                            )
                    })
                    .when(is_project_search_visible, |overlay| {
                        overlay
                            .child(
                                div()
                                    .absolute()
                                    .size_full()
                                    .inset_0()
                                    .occlude()
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            this.project_search.update(cx, |search, cx| {
                                                search.close(window, cx);
                                            });
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .size_full()
                                    .top(px(80.))
                                    .flex()
                                    .justify_center()
                                    .items_start()
                                    .child(
                                        div()
                                            .occlude()
                                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation();
                                            })
                                            .child(self.project_search.clone()),
                                    ),
                            )
                    }),
            )
            .child(self.render_bottom_bar(window, cx))
            .children(Root::render_dialog_layer(window, cx))
    }
}

fn error_result(error: DataSourceError) -> QueryResult {
    let error_msg = match error {
        DataSourceError::QueryFailed(msg) => msg,
        _ => error.to_string(),
    };
    QueryResult {
        columns: vec!["error".into()],
        column_metadata: vec![ColumnMetadata {
            name: "error".into(),
            data_type: "text".into(),
            is_pk: false,
            is_fk: false,
        }],
        rows: vec![vec![error_msg]],
        nulls: vec![vec![false]],
        row_count: 1,
        execution_time_ms: 0,
    }
}

fn should_open_result_tab(succeeded: bool, result: &QueryResult) -> bool {
    !succeeded || !result.columns.is_empty() || !result.rows.is_empty()
}

fn connection_fingerprints(configs: &[DataSourceConfig]) -> HashMap<String, String> {
    configs
        .iter()
        .map(|config| {
            (
                config.name.clone(),
                format!(
                    "{:?}\n{}\n{}\n{}\n{}\n{}\n{}",
                    config.db_type,
                    config.host,
                    config.port,
                    config.user,
                    config.database,
                    config.schema,
                    config.query_string
                ),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query_result(columns: Vec<&str>, rows: Vec<Vec<&str>>) -> QueryResult {
        QueryResult {
            columns: columns.into_iter().map(str::to_string).collect(),
            column_metadata: Vec::new(),
            rows: rows
                .into_iter()
                .map(|row| row.into_iter().map(str::to_string).collect())
                .collect(),
            nulls: Vec::new(),
            row_count: 0,
            execution_time_ms: 0,
        }
    }

    #[test]
    fn opens_result_tab_for_empty_select_result() {
        let result = query_result(vec!["id"], Vec::new());

        assert!(should_open_result_tab(true, &result));
    }

    #[test]
    fn skips_result_tab_for_success_without_result_set() {
        let result = query_result(Vec::new(), Vec::new());

        assert!(!should_open_result_tab(true, &result));
    }

    #[test]
    fn opens_result_tab_for_errors() {
        let result = query_result(Vec::new(), Vec::new());

        assert!(should_open_result_tab(false, &result));
    }
}
