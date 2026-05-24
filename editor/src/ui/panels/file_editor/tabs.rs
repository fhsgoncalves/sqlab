use std::path::PathBuf;

use gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, Styled, Subscription, WeakEntity,
    Window, actions, div, hsla, prelude::FluentBuilder, px, rgb,
};
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, Sizable, WindowExt,
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockPlacement, Panel, PanelEvent, PanelState},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};

use super::data_editor::DataEditorPanel;
use super::editor::{EditorCursorPosition, EditorPanel, EditorPanelEvent, ExecuteQuery};
use crate::credentials;
use crate::query_session::QuerySessionStore;
use crate::ui::activity::ActivityTracker;
use crate::ui::components::tab::{Tab, TabBar};
use crate::ui::panels::connection::ConnectionPanel;
use crate::ui::panels::diagram::{DiagramModel, DiagramPanel};
use sqlab_drivers_core::{
    ConnectionStatus, DataSourceConfig, TableInfo, manager::DataSourceManager,
};

actions!(
    editor_tabs,
    [
        CycleTabForward,
        CycleTabBackward,
        NavigateBack,
        NavigateForward
    ]
);

const NAVIGATION_HISTORY_LIMIT: usize = 100;
const SIGNIFICANT_LINE_DELTA: usize = 20;

pub struct EditorTabs {
    tabs: Vec<EditorTab>,
    active_ix: usize,
    focus_handle: FocusHandle,
    dock_area: Option<WeakEntity<DockArea>>,
    data_source_manager: Entity<DataSourceManager>,
    is_zoomed: bool,
    activity_tracker: Entity<ActivityTracker>,
    query_sessions: QuerySessionStore,
    navigation_history: NavigationHistory,
    navigation_subscriptions: Vec<Subscription>,
    suppress_navigation_recording: bool,
}

enum EditorTab {
    Sql(Entity<EditorPanel>),
    Diagram(Entity<DiagramPanel>),
    Data(Entity<DataEditorPanel>),
}

impl EditorTab {
    fn label(&self, cx: &App) -> String {
        match self {
            EditorTab::Sql(editor) => editor
                .read(cx)
                .path()
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Untitled")
                .to_string(),
            EditorTab::Diagram(diagram) => diagram.read(cx).title().to_string(),
            EditorTab::Data(data_editor) => data_editor.read(cx).title(),
        }
    }

    fn icon(&self) -> Option<Icon> {
        match self {
            EditorTab::Sql(_) => None,
            EditorTab::Diagram(_) => None,
            EditorTab::Data(_) => Some(Icon::new(IconName::File).path("icons/table.svg")),
        }
    }

    fn as_sql(&self) -> Option<&Entity<EditorPanel>> {
        match self {
            EditorTab::Sql(editor) => Some(editor),
            EditorTab::Diagram(_) | EditorTab::Data(_) => None,
        }
    }

    fn element(&self) -> AnyElement {
        match self {
            EditorTab::Sql(editor) => editor.clone().into_any_element(),
            EditorTab::Diagram(diagram) => diagram.clone().into_any_element(),
            EditorTab::Data(data_editor) => data_editor.clone().into_any_element(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EditorNavigationPoint {
    path: PathBuf,
    row: usize,
    column: usize,
    cursor: usize,
    visible_rows: Option<std::ops::Range<usize>>,
}

impl EditorNavigationPoint {
    fn new(path: PathBuf, cursor: EditorCursorPosition) -> Self {
        Self {
            path,
            row: cursor.row,
            column: cursor.column,
            cursor: cursor.cursor,
            visible_rows: cursor.visible_rows,
        }
    }
}

#[derive(Default)]
struct NavigationHistory {
    back: Vec<EditorNavigationPoint>,
    forward: Vec<EditorNavigationPoint>,
    last_recorded: Option<EditorNavigationPoint>,
}

impl NavigationHistory {
    fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    fn sync_current(&mut self, current: Option<EditorNavigationPoint>) {
        self.last_recorded = current;
    }

    fn record_navigation_away_from(&mut self, current: EditorNavigationPoint) {
        push_limited(&mut self.back, current);
        self.forward.clear();
    }

    fn record_movement_to(&mut self, current: EditorNavigationPoint) {
        let Some(previous) = self.last_recorded.clone() else {
            self.last_recorded = Some(current);
            return;
        };

        if is_significant_movement(&previous, &current) {
            push_limited(&mut self.back, previous);
            self.forward.clear();
            self.last_recorded = Some(current);
        }
    }

    fn go_back(&mut self, current: Option<EditorNavigationPoint>) -> Option<EditorNavigationPoint> {
        let current = current.or_else(|| self.last_recorded.clone());
        while let Some(target) = self.back.pop() {
            if current.as_ref() == Some(&target) {
                continue;
            }
            if let Some(current) = current.clone() {
                push_limited(&mut self.forward, current);
            }
            self.last_recorded = Some(target.clone());
            return Some(target);
        }
        None
    }

    fn go_forward(
        &mut self,
        current: Option<EditorNavigationPoint>,
    ) -> Option<EditorNavigationPoint> {
        let current = current.or_else(|| self.last_recorded.clone());
        while let Some(target) = self.forward.pop() {
            if current.as_ref() == Some(&target) {
                continue;
            }
            if let Some(current) = current.clone() {
                push_limited(&mut self.back, current);
            }
            self.last_recorded = Some(target.clone());
            return Some(target);
        }
        None
    }
}

fn push_limited(stack: &mut Vec<EditorNavigationPoint>, point: EditorNavigationPoint) {
    if stack.last() == Some(&point) {
        return;
    }

    stack.push(point);
    if stack.len() > NAVIGATION_HISTORY_LIMIT {
        stack.remove(0);
    }
}

fn is_significant_movement(
    previous: &EditorNavigationPoint,
    current: &EditorNavigationPoint,
) -> bool {
    if previous.path != current.path {
        return true;
    }

    if previous.cursor == current.cursor {
        return false;
    }

    if let Some(visible_rows) = &previous.visible_rows
        && !visible_rows.contains(&current.row)
    {
        return true;
    }

    previous.row.abs_diff(current.row) >= SIGNIFICANT_LINE_DELTA
}

impl EditorTabs {
    pub fn new(
        data_source_manager: Entity<DataSourceManager>,
        activity_tracker: Entity<ActivityTracker>,
        query_sessions: QuerySessionStore,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            tabs: Vec::new(),
            active_ix: 0,
            focus_handle: cx.focus_handle(),
            dock_area: None,
            data_source_manager,
            is_zoomed: false,
            activity_tracker,
            query_sessions,
            navigation_history: NavigationHistory::default(),
            navigation_subscriptions: Vec::new(),
            suppress_navigation_recording: false,
        }
    }

    pub fn set_dock_area(&mut self, dock_area: WeakEntity<DockArea>) {
        self.dock_area = Some(dock_area);
    }

    pub fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_path(cx).as_ref() != Some(&path) {
            self.record_current_before_navigation(cx);
        }
        self.open_file_internal(path, window, cx);
        self.sync_current_navigation_point(cx);
    }

    fn open_file_internal(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.iter().position(|tab| {
            tab.as_sql()
                .map(|editor| *editor.read(cx).path() == path)
                .unwrap_or(false)
        }) {
            self.active_ix = ix;
            self.sync_active_connection(cx);
            cx.notify();
            return;
        }

        let data_source_manager = self.data_source_manager.clone();
        let editor = cx.new(|cx| EditorPanel::new(path, data_source_manager, window, cx));
        self.subscribe_to_editor_navigation(&editor, cx);
        self.tabs.push(EditorTab::Sql(editor));
        self.active_ix = self.tabs.len() - 1;
        self.sync_active_connection(cx);
        cx.notify();
    }

    pub fn open_diagram(
        &mut self,
        model: DiagramModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self.tabs.iter().position(|tab| match tab {
            EditorTab::Diagram(diagram) => diagram.read(cx).title() == model.title,
            EditorTab::Sql(_) | EditorTab::Data(_) => false,
        }) {
            self.active_ix = ix;
            self.sync_active_connection(cx);
            cx.notify();
            return;
        }

        let diagram =
            cx.new(|cx| DiagramPanel::new(model, self.activity_tracker.clone(), window, cx));
        self.tabs.push(EditorTab::Diagram(diagram));
        self.active_ix = self.tabs.len() - 1;
        self.sync_active_connection(cx);
        cx.notify();
    }

    pub fn open_data_editor(
        &mut self,
        config: DataSourceConfig,
        table: TableInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self.tabs.iter().position(|tab| match tab {
            EditorTab::Data(data_editor) => data_editor.read(cx).matches_table(&config, &table),
            EditorTab::Sql(_) | EditorTab::Diagram(_) => false,
        }) {
            self.active_ix = ix;
            self.sync_active_connection(cx);
            cx.notify();
            return;
        }

        let data_editor = cx.new(|cx| {
            DataEditorPanel::new(config, table, self.activity_tracker.clone(), window, cx)
        });
        self.tabs.push(EditorTab::Data(data_editor));
        self.active_ix = self.tabs.len() - 1;
        self.sync_active_connection(cx);
        cx.notify();
    }

    pub fn open_file_at_position(
        &mut self,
        path: PathBuf,
        line_number: usize,
        column: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.record_current_before_navigation(cx);
        self.open_file_internal(path, window, cx);
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |editor, cx| {
                editor.go_to_position(line_number, column, window, cx);
            });
        }
        self.sync_current_navigation_point(cx);
    }

    pub fn active_path(&self, cx: &App) -> Option<PathBuf> {
        self.tabs
            .get(self.active_ix)
            .and_then(|tab| tab.as_sql())
            .map(|editor| editor.read(cx).path().clone())
    }

    fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.tabs.len() {
            let tab = self.tabs.remove(ix);
            if let Some(editor) = tab.as_sql() {
                let path = editor.read(cx).path().clone();
                let query_sessions = self.query_sessions.clone();
                cx.spawn(async move |_this, _cx| {
                    if let Err(error) = query_sessions.close_path(path).await {
                        eprintln!("failed to close query session: {}", error);
                    }
                })
                .detach();
            }
            if self.active_ix >= self.tabs.len() {
                self.active_ix = self.tabs.len().saturating_sub(1);
            }
            self.sync_current_navigation_point(cx);
            self.sync_active_connection(cx);
            cx.notify();
        }
    }

    pub fn clear_tabs(&mut self, cx: &mut Context<Self>) {
        self.tabs.clear();
        self.active_ix = 0;
        self.navigation_history = NavigationHistory::default();
        self.sync_active_connection(cx);
        let query_sessions = self.query_sessions.clone();
        cx.spawn(async move |_this, _cx| {
            if let Err(error) = query_sessions.close_all().await {
                eprintln!("failed to close query sessions: {}", error);
            }
        })
        .detach();
        cx.notify();
    }

    pub fn active_editor(&self) -> Option<&Entity<EditorPanel>> {
        self.tabs.get(self.active_ix).and_then(|tab| tab.as_sql())
    }

    pub fn toggle_replace_in_active_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |editor, cx| {
                editor.toggle_search_replace(window, cx);
            });
        }
    }

    fn cycle_tab_forward(
        &mut self,
        _: &CycleTabForward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.len() > 1 {
            self.record_current_before_navigation(cx);
            self.active_ix = (self.active_ix + 1) % self.tabs.len();
            self.sync_current_navigation_point(cx);
            self.sync_active_connection(cx);
            cx.notify();
            self.focus_active_editor(window, cx);
        }
    }

    fn cycle_tab_backward(
        &mut self,
        _: &CycleTabBackward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.len() > 1 {
            self.record_current_before_navigation(cx);
            self.active_ix = (self.active_ix + self.tabs.len() - 1) % self.tabs.len();
            self.sync_current_navigation_point(cx);
            self.sync_active_connection(cx);
            cx.notify();
            self.focus_active_editor(window, cx);
        }
    }

    fn select_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() || ix == self.active_ix {
            return;
        }
        self.record_current_before_navigation(cx);
        self.active_ix = ix;
        self.sync_current_navigation_point(cx);
        self.sync_active_connection(cx);
        cx.notify();
        self.focus_active_editor(window, cx);
    }

    fn reorder_tab(&mut self, from_ix: usize, to_ix: usize, cx: &mut Context<Self>) {
        if from_ix >= self.tabs.len() || to_ix >= self.tabs.len() || from_ix == to_ix {
            return;
        }
        let tab = self.tabs.remove(from_ix);
        self.tabs.insert(to_ix, tab);
        if self.active_ix == from_ix {
            self.active_ix = to_ix;
        } else if from_ix < self.active_ix && to_ix >= self.active_ix {
            self.active_ix -= 1;
        } else if from_ix > self.active_ix && to_ix <= self.active_ix {
            self.active_ix += 1;
        }
        self.sync_active_connection(cx);
        cx.notify();
    }

    fn subscribe_to_editor_navigation(
        &mut self,
        editor: &Entity<EditorPanel>,
        cx: &mut Context<Self>,
    ) {
        let subscription =
            cx.subscribe(
                editor,
                |this, editor, event: &EditorPanelEvent, cx| match event {
                    EditorPanelEvent::CursorMoved => {
                        this.record_editor_movement(&editor, cx);
                    }
                },
            );
        self.navigation_subscriptions.push(subscription);
    }

    fn active_navigation_point(&self, cx: &App) -> Option<EditorNavigationPoint> {
        self.active_editor().map(|editor| {
            EditorNavigationPoint::new(
                editor.read(cx).path().clone(),
                editor.read(cx).cursor_position(cx),
            )
        })
    }

    fn record_current_before_navigation(&mut self, cx: &App) {
        if self.suppress_navigation_recording {
            return;
        }
        if let Some(current) = self.active_navigation_point(cx) {
            self.navigation_history.record_navigation_away_from(current);
        }
    }

    fn sync_current_navigation_point(&mut self, cx: &App) {
        self.navigation_history
            .sync_current(self.active_navigation_point(cx));
    }

    fn record_editor_movement(&mut self, editor: &Entity<EditorPanel>, cx: &mut Context<Self>) {
        if self.suppress_navigation_recording {
            return;
        }

        let Some(active_editor) = self.active_editor() else {
            return;
        };
        if active_editor != editor {
            return;
        }

        let point = EditorNavigationPoint::new(
            editor.read(cx).path().clone(),
            editor.read(cx).cursor_position(cx),
        );
        self.navigation_history.record_movement_to(point);
        cx.notify();
    }

    fn navigate_back(&mut self, _: &NavigateBack, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.active_navigation_point(cx);
        if let Some(target) = self.navigation_history.go_back(current) {
            self.navigate_to_point(target, window, cx);
        }
    }

    fn navigate_forward(
        &mut self,
        _: &NavigateForward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.active_navigation_point(cx);
        if let Some(target) = self.navigation_history.go_forward(current) {
            self.navigate_to_point(target, window, cx);
        }
    }

    fn navigate_to_point(
        &mut self,
        target: EditorNavigationPoint,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.suppress_navigation_recording = true;
        self.open_file_internal(target.path.clone(), window, cx);
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |editor, cx| {
                editor.go_to_position(target.row + 1, target.column, window, cx);
            });
        }
        self.suppress_navigation_recording = false;
        self.navigation_history.sync_current(Some(target));
        self.sync_active_connection(cx);
        cx.notify();
        self.focus_active_editor(window, cx);
    }

    fn focus_active_editor(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor() {
            let focus_handle = editor.read(cx).editor_focus_handle(cx);
            window.focus(&focus_handle, cx);
        }
    }

    pub fn save_all(&mut self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            if let Some(editor) = tab.as_sql() {
                editor.update(cx, |editor, cx| {
                    editor.save(cx);
                });
            }
        }
    }

    fn select_connection(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor().cloned() else {
            return;
        };

        let result = self.data_source_manager.update(cx, |manager, cx| {
            manager.set_status(&name, ConnectionStatus::Idle);
            manager.clear_last_error(&name);

            if let Err(error) = manager.ensure_password_loaded(&name, |n| {
                credentials::load_password(n).map_err(|e| credentials::recovery_error_message(&e))
            }) {
                manager.set_status(&name, ConnectionStatus::Failed);
                manager.set_last_error(&name, error.clone());
                cx.notify();
                return Err(error);
            }

            manager
                .configs()
                .iter()
                .find(|config| config.name == name)
                .cloned()
                .ok_or_else(|| "The selected connection no longer exists.".to_string())?;
            cx.notify();
            Ok(())
        });

        match result {
            Ok(()) => {
                editor.update(cx, |editor, cx| {
                    editor.set_selected_connection_name(Some(name), cx);
                });
                self.sync_active_connection(cx);
            }
            Err(error) => {
                window.open_alert_dialog(cx, move |alert, _, _| {
                    alert
                        .title("Connection Setup Failed")
                        .description(error.clone())
                });
            }
        }
    }

    fn close_active_connection(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor().cloned() else {
            return;
        };
        let path = editor.read(cx).path().clone();
        let connection_name = editor
            .read(cx)
            .selected_connection_name()
            .map(str::to_string);
        let Some(connection_name) = connection_name else {
            return;
        };

        let query_sessions = self.query_sessions.clone();
        let manager = self.data_source_manager.clone();
        query_sessions.mark_closing(path.clone(), connection_name.clone());
        cx.spawn(async move |_this, cx| {
            if let Err(error) = query_sessions
                .close_path_connection(path, connection_name.clone())
                .await
            {
                eprintln!("failed to close query session: {}", error);
            }
            if !query_sessions.is_connection_open(&connection_name) {
                cx.update_entity(&manager, move |manager, cx| {
                    manager.set_status(&connection_name, ConnectionStatus::Idle);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn sync_active_connection(&self, cx: &mut Context<Self>) {
        let active_name = self.active_editor().and_then(|editor| {
            editor
                .read(cx)
                .selected_connection_name()
                .map(str::to_string)
        });
        self.data_source_manager.update(cx, |manager, cx| {
            manager.set_active(active_name);
            cx.notify();
        });
    }
}

impl EventEmitter<PanelEvent> for EditorTabs {}

impl Panel for EditorTabs {
    fn panel_name(&self) -> &'static str {
        "EditorTabs"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        ""
    }

    fn closable(&self, _cx: &App) -> bool {
        false
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState::new(self)
    }
}

impl Render for EditorTabs {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();

        let zoom_btn = self.dock_area.as_ref().map(|_| {
            let is_zoomed = self.is_zoomed;
            Button::new("toggle-zoom")
                .icon(if is_zoomed {
                    IconName::Minimize
                } else {
                    IconName::Maximize
                })
                .xsmall()
                .ghost()
                .tooltip(if is_zoomed { "Restore" } else { "Maximize" })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.is_zoomed = !this.is_zoomed;
                    if let Some(dock_area) = this.dock_area.as_ref() {
                        if let Some(dock_area) = dock_area.upgrade() {
                            dock_area.update(cx, |dock_area, cx| {
                                if this.is_zoomed {
                                    if dock_area.is_dock_open(DockPlacement::Left, cx) {
                                        dock_area.toggle_dock(DockPlacement::Left, window, cx);
                                    }
                                    if dock_area.is_dock_open(DockPlacement::Right, cx) {
                                        dock_area.toggle_dock(DockPlacement::Right, window, cx);
                                    }
                                    if dock_area.is_dock_open(DockPlacement::Bottom, cx) {
                                        dock_area.toggle_dock(DockPlacement::Bottom, window, cx);
                                    }
                                } else {
                                    if !dock_area.is_dock_open(DockPlacement::Left, cx) {
                                        dock_area.toggle_dock(DockPlacement::Left, window, cx);
                                    }
                                    if !dock_area.is_dock_open(DockPlacement::Right, cx) {
                                        dock_area.toggle_dock(DockPlacement::Right, window, cx);
                                    }
                                    if !dock_area.is_dock_open(DockPlacement::Bottom, cx) {
                                        dock_area.toggle_dock(DockPlacement::Bottom, window, cx);
                                    }
                                }
                            });
                        }
                    }
                    cx.notify();
                }))
        });

        let can_go_back = self.navigation_history.can_go_back();
        let can_go_forward = self.navigation_history.can_go_forward();
        let navigation_controls = h_flex()
            .id("editor-navigation-controls")
            .h_full()
            .items_center()
            .gap_0p5()
            .px_1()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("editor-navigate-back")
                    .icon(IconName::ArrowLeft)
                    .xsmall()
                    .ghost()
                    .disabled(!can_go_back)
                    .tooltip_with_action("Go Back", &NavigateBack, None)
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(NavigateBack), cx);
                    }),
            )
            .child(
                Button::new("editor-navigate-forward")
                    .icon(IconName::ArrowRight)
                    .xsmall()
                    .ghost()
                    .disabled(!can_go_forward)
                    .tooltip_with_action("Go Forward", &NavigateForward, None)
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(NavigateForward), cx);
                    }),
            );

        let tab_bar = TabBar::new("editor-tab-bar")
            .selected_index(self.active_ix)
            .prefix(navigation_controls)
            .suffix(h_flex().gap_1().children(zoom_btn))
            .on_click(cx.listener(|this, ix: &usize, window, cx| {
                this.select_tab(*ix, window, cx);
            }))
            .on_reorder(cx.listener(|this, (from_ix, to_ix), _, cx| {
                this.reorder_tab(*from_ix, *to_ix, cx);
            }));

        let tab_bar = self
            .tabs
            .iter()
            .enumerate()
            .fold(tab_bar, |tab_bar, (ix, tab)| {
                let entity = entity.clone();
                let is_active = ix == self.active_ix;

                tab_bar.child(
                    Tab::new()
                        .label(tab.label(cx))
                        .when_some(tab.icon(), |tab, icon| tab.icon(icon))
                        .selected(is_active)
                        .closable(true)
                        .on_close(move |_window, cx| {
                            entity.update(cx, |this, cx| {
                                this.close_tab(ix, cx);
                            });
                        }),
                )
            });

        let active_sql_editor = self
            .tabs
            .get(self.active_ix)
            .and_then(|tab| tab.as_sql())
            .cloned();
        let active_name = active_sql_editor.as_ref().and_then(|editor| {
            editor
                .read(cx)
                .selected_connection_name()
                .map(str::to_string)
        });
        let active_path = active_sql_editor
            .as_ref()
            .map(|editor| editor.read(cx).path().clone());
        let active_connection = active_name.as_ref().map(|name| {
            let status = if active_path
                .as_deref()
                .is_some_and(|path| self.query_sessions.is_open(path, name))
            {
                ConnectionStatus::Connected
            } else if self.data_source_manager.read(cx).status(name) == ConnectionStatus::Failed {
                ConnectionStatus::Failed
            } else {
                ConnectionStatus::Idle
            };
            format!("{} ({})", name, connection_status_label(status))
        });
        let connections = self
            .data_source_manager
            .read(cx)
            .configs()
            .iter()
            .cloned()
            .map(|config| {
                let status = self.data_source_manager.read(cx).status(&config.name);
                (config, status)
            })
            .collect::<Vec<_>>();

        let active_connection_fg = if cx.theme().is_dark() {
            hsla(0.72, 0.90, 0.78, 1.0)
        } else {
            hsla(0.74, 0.70, 0.42, 1.0)
        };

        let is_sql_active = self
            .tabs
            .get(self.active_ix)
            .map_or(false, |tab| tab.as_sql().is_some());
        let editor_toolbar = is_sql_active.then(|| {
            let search_path_selector = active_sql_editor.as_ref().and_then(|editor| {
                let schemas = editor.read(cx).available_search_paths(cx);
                if schemas.is_empty() {
                    return None;
                }

                let selected_label = editor.read(cx).search_path_label();
                let editor_for_default = editor.clone();
                let editor_for_menu = editor.clone();
                Some(
                    Button::new("editor-search-path-selector")
                        .label(selected_label)
                        .xsmall()
                        .ghost()
                        .tooltip("Select search path")
                        .dropdown_menu(move |menu, window, _cx| {
                            let mut menu = menu.item(PopupMenuItem::new("Default").on_click(
                                window.listener_for(&editor_for_default, |this, _, _window, cx| {
                                    this.set_search_path(None, cx);
                                }),
                            ));

                            for schema in &schemas {
                                let editor_for_schema = editor_for_menu.clone();
                                let schema = schema.clone();
                                menu = menu.item(PopupMenuItem::new(schema.clone()).on_click(
                                    window.listener_for(
                                        &editor_for_schema,
                                        move |this, _, _window, cx| {
                                            this.set_search_path(Some(schema.clone()), cx);
                                        },
                                    ),
                                ));
                            }

                            menu
                        }),
                )
            });

            h_flex()
                .id("editor-toolbar")
                .h(px(32.))
                .flex_none()
                .items_center()
                .px_2()
                .border_b_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().tab_bar)
                .child(
                    Button::new("execute-query")
                        .icon(IconName::Play)
                        .xsmall()
                        .ghost()
                        .text_color(rgb(0x58a65c))
                        .tooltip_with_action("Execute Query", &ExecuteQuery, Some("Input"))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ExecuteQuery), cx);
                        }),
                )
                .child(div().flex_1())
                .when_some(search_path_selector, |toolbar, selector| {
                    toolbar.child(selector)
                })
                .when(!connections.is_empty(), |toolbar| {
                    let selected_name = active_name.clone();
                    let active_path = active_path.clone();
                    let connection_label =
                        active_connection.unwrap_or_else(|| "No connection".to_string());
                    let connections = connections.clone();
                    let query_sessions = self.query_sessions.clone();
                    let view = entity.clone();
                    toolbar.child(
                        Button::new("active-connection-picker")
                            .label(connection_label)
                            .icon(IconName::HardDrive)
                            .dropdown_caret(true)
                            .xsmall()
                            .text_color(active_connection_fg)
                            .tooltip("Switch Active Connection")
                            .dropdown_menu(move |menu, window, _cx| {
                                let mut menu = menu;
                                for (config, status) in &connections {
                                    let name = config.name.clone();
                                    let view_for_item = view.clone();
                                    let view_for_close = view.clone();
                                    let is_selected =
                                        selected_name.as_deref() == Some(name.as_str());
                                    let icon_path =
                                        ConnectionPanel::database_icon_path(config.db_type);
                                    let close_button_id =
                                        format!("close-active-connection-{}", config.name);
                                    let active_path_for_row = active_path.clone();
                                    let query_sessions_for_row = query_sessions.clone();
                                    let status = *status;
                                    let label_name = name.clone();
                                    let select_name = name.clone();
                                    let is_open_name = name.clone();
                                    menu = menu.item(
                                        PopupMenuItem::element(move |_window, _cx| {
                                            let is_open = is_selected
                                                && active_path_for_row.as_deref().is_some_and(
                                                    |path| query_sessions_for_row.is_open(path, &is_open_name),
                                                );
                                            let row_status = if is_open {
                                                ConnectionStatus::Connected
                                            } else if is_selected
                                                && status != ConnectionStatus::Failed
                                            {
                                                ConnectionStatus::Idle
                                            } else {
                                                status
                                            };
                                            let label = format!(
                                                "{} ({})",
                                                label_name,
                                                connection_status_label(row_status)
                                            );
                                            h_flex()
                                                .w_full()
                                                .items_center()
                                                .gap_2()
                                                .child(div().flex_1().child(label.clone()))
                                                .when(is_open, |row| {
                                                    row.child(
                                                        Button::new(close_button_id.clone())
                                                            .icon(IconName::Close)
                                                            .xsmall()
                                                            .ghost()
                                                            .tooltip("Close Connection")
                                                            .on_click({
                                                                let view_for_close =
                                                                    view_for_close.clone();
                                                                move |_, window, cx| {
                                                                    window.prevent_default();
                                                                    cx.stop_propagation();
                                                                    view_for_close.update(
                                                                        cx,
                                                                        |this, cx| {
                                                                            this.close_active_connection(
                                                                                cx,
                                                                            );
                                                                        },
                                                                    );
                                                                    cx.refresh_windows();
                                                                }
                                                            }),
                                                    )
                                                })
                                        })
                                            .icon(Icon::new(IconName::File).path(icon_path))
                                            .checked(is_selected)
                                            .on_click(window.listener_for(
                                                &view_for_item,
                                                move |this, _, window, cx| {
                                                    this.select_connection(
                                                        select_name.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                },
                                            )),
                                    );
                                }
                                menu
                            }),
                    )
                })
        });

        v_flex()
            .id("editor-tabs")
            .size_full()
            .bg(cx.theme().background)
            .on_action(cx.listener(Self::cycle_tab_forward))
            .on_action(cx.listener(Self::cycle_tab_backward))
            .on_action(cx.listener(Self::navigate_back))
            .on_action(cx.listener(Self::navigate_forward))
            .child(tab_bar)
            .when_some(editor_toolbar, |this, toolbar| this.child(toolbar))
            .child(
                div()
                    .id("editor-content")
                    .flex_1()
                    .overflow_hidden()
                    .map(|this| {
                        if let Some(tab) = self.tabs.get(self.active_ix) {
                            this.child(tab.element())
                        } else {
                            this
                        }
                    }),
            )
    }
}

fn connection_status_label(status: ConnectionStatus) -> &'static str {
    match status {
        ConnectionStatus::Idle => "idle",
        ConnectionStatus::Connected => "connected",
        ConnectionStatus::Failed => "failed",
    }
}

impl Focusable for EditorTabs {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(path: &str, row: usize) -> EditorNavigationPoint {
        EditorNavigationPoint {
            path: PathBuf::from(path),
            row,
            column: 0,
            cursor: row * 10,
            visible_rows: Some(row..row + 10),
        }
    }

    #[test]
    fn small_cursor_moves_do_not_create_history_entries() {
        let mut history = NavigationHistory::default();
        history.sync_current(Some(point("a.sql", 10)));

        history.record_movement_to(point("a.sql", 14));

        assert!(!history.can_go_back());
    }

    #[test]
    fn large_cursor_moves_record_previous_location() {
        let mut history = NavigationHistory::default();
        let start = point("a.sql", 10);
        let target = point("a.sql", 40);
        history.sync_current(Some(start.clone()));

        history.record_movement_to(target.clone());

        assert_eq!(history.go_back(Some(target)), Some(start));
        assert!(history.can_go_forward());
    }

    #[test]
    fn recording_new_navigation_clears_forward_history() {
        let mut history = NavigationHistory::default();
        let first = point("a.sql", 0);
        let second = point("b.sql", 0);
        let third = point("c.sql", 0);

        history.record_navigation_away_from(first.clone());
        history.sync_current(Some(second.clone()));
        assert_eq!(history.go_back(Some(second)), Some(first.clone()));
        assert!(history.can_go_forward());

        history.record_navigation_away_from(first);
        history.sync_current(Some(third));

        assert!(!history.can_go_forward());
    }

    #[test]
    fn back_history_is_limited_to_latest_entries() {
        let mut history = NavigationHistory::default();

        for row in 0..(NAVIGATION_HISTORY_LIMIT + 5) {
            history.record_navigation_away_from(point("a.sql", row));
        }

        assert_eq!(history.back.len(), NAVIGATION_HISTORY_LIMIT);
        assert_eq!(history.back.first().map(|point| point.row), Some(5));
    }
}
