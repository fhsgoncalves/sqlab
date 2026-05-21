use std::path::PathBuf;

use gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, Styled, WeakEntity, Window, actions,
    div, hsla, prelude::FluentBuilder, px, rgb,
};
use gpui_component::{
    ActiveTheme, Icon, IconName, Sizable, WindowExt,
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockPlacement, Panel, PanelEvent, PanelState},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};

use super::editor::{EditorPanel, ExecuteQuery};
use crate::credentials;
use crate::drivers::create_configured_data_source;
use crate::ui::activity::ActivityTracker;
use crate::ui::components::tab::{Tab, TabBar};
use crate::ui::panels::connection::ConnectionPanel;
use crate::ui::panels::diagram::{DiagramModel, DiagramPanel};
use sqlab_drivers_core::{
    ConnectionStatus, DataSourceConfig, DataSourceError, manager::DataSourceManager,
};

actions!(editor_tabs, [CycleTabForward, CycleTabBackward]);

pub struct EditorTabs {
    tabs: Vec<EditorTab>,
    active_ix: usize,
    focus_handle: FocusHandle,
    dock_area: Option<WeakEntity<DockArea>>,
    data_source_manager: Entity<DataSourceManager>,
    is_zoomed: bool,
    activity_tracker: Entity<ActivityTracker>,
}

enum EditorTab {
    Sql(Entity<EditorPanel>),
    Diagram(Entity<DiagramPanel>),
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
        }
    }

    fn as_sql(&self) -> Option<&Entity<EditorPanel>> {
        match self {
            EditorTab::Sql(editor) => Some(editor),
            EditorTab::Diagram(_) => None,
        }
    }

    fn element(&self) -> AnyElement {
        match self {
            EditorTab::Sql(editor) => editor.clone().into_any_element(),
            EditorTab::Diagram(diagram) => diagram.clone().into_any_element(),
        }
    }
}

impl EditorTabs {
    pub fn new(
        data_source_manager: Entity<DataSourceManager>,
        activity_tracker: Entity<ActivityTracker>,
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
        }
    }

    pub fn set_dock_area(&mut self, dock_area: WeakEntity<DockArea>) {
        self.dock_area = Some(dock_area);
    }

    pub fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.iter().position(|tab| {
            tab.as_sql()
                .map(|editor| *editor.read(cx).path() == path)
                .unwrap_or(false)
        }) {
            self.active_ix = ix;
            cx.notify();
            return;
        }

        let data_source_manager = self.data_source_manager.clone();
        let editor = cx.new(|cx| EditorPanel::new(path, data_source_manager, window, cx));
        self.tabs.push(EditorTab::Sql(editor));
        self.active_ix = self.tabs.len() - 1;
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
            EditorTab::Sql(_) => false,
        }) {
            self.active_ix = ix;
            cx.notify();
            return;
        }

        let diagram =
            cx.new(|cx| DiagramPanel::new(model, self.activity_tracker.clone(), window, cx));
        self.tabs.push(EditorTab::Diagram(diagram));
        self.active_ix = self.tabs.len() - 1;
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
        self.open_file(path, window, cx);
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |editor, cx| {
                editor.go_to_position(line_number, column, window, cx);
            });
        }
    }

    pub fn active_path(&self, cx: &App) -> Option<PathBuf> {
        self.tabs
            .get(self.active_ix)
            .and_then(|tab| tab.as_sql())
            .map(|editor| editor.read(cx).path().clone())
    }

    fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.tabs.len() {
            self.tabs.remove(ix);
            if self.active_ix >= self.tabs.len() {
                self.active_ix = self.tabs.len().saturating_sub(1);
            }
            cx.notify();
        }
    }

    pub fn clear_tabs(&mut self, cx: &mut Context<Self>) {
        self.tabs.clear();
        self.active_ix = 0;
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
            self.active_ix = (self.active_ix + 1) % self.tabs.len();
            cx.notify();
            if let Some(editor) = self.active_editor() {
                let focus_handle = editor.read(cx).editor_focus_handle(cx);
                window.focus(&focus_handle, cx);
            }
        }
    }

    fn cycle_tab_backward(
        &mut self,
        _: &CycleTabBackward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.len() > 1 {
            self.active_ix = (self.active_ix + self.tabs.len() - 1) % self.tabs.len();
            cx.notify();
            if let Some(editor) = self.active_editor() {
                let focus_handle = editor.read(cx).editor_focus_handle(cx);
                window.focus(&focus_handle, cx);
            }
        }
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
        cx.notify();
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
        let result = self.data_source_manager.update(cx, |manager, cx| {
            let status = manager.status(&name);
            manager.set_active(Some(name.clone()));

            if status == ConnectionStatus::Connected {
                cx.notify();
                return Ok(None);
            }

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

            let config = manager
                .configs()
                .iter()
                .find(|config| config.name == name)
                .cloned()
                .ok_or_else(|| "The selected connection no longer exists.".to_string())?;
            cx.notify();
            Ok(Some(config))
        });

        match result {
            Ok(Some(config)) => self.test_selected_connection(config, cx),
            Ok(None) => {}
            Err(error) => {
                window.open_alert_dialog(cx, move |alert, _, _| {
                    alert
                        .title("Connection Setup Failed")
                        .description(error.clone())
                });
            }
        }
    }

    fn test_selected_connection(&mut self, config: DataSourceConfig, cx: &mut Context<Self>) {
        let manager = self.data_source_manager.clone();
        let activity_tracker = self.activity_tracker.clone();
        let config_name = config.name.clone();
        let activity_label = format!("Connecting: {}", config_name);
        let activity_id = self
            .activity_tracker
            .update(cx, |tracker, cx| tracker.begin(activity_label, cx));

        cx.spawn(async move |_this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut source = create_configured_data_source(&config)?;
                    source.connect().await?;
                    source.disconnect().await?;
                    Ok::<(), DataSourceError>(())
                })
                .await;

            cx.update_entity(&manager, move |manager, cx| {
                match result {
                    Ok(_) => {
                        manager.set_status(&config_name, ConnectionStatus::Connected);
                        manager.clear_last_error(&config_name);
                    }
                    Err(error) => {
                        manager.set_status(&config_name, ConnectionStatus::Failed);
                        manager.set_last_error(&config_name, error.to_string());
                    }
                }
                cx.notify();
            });

            cx.update_entity(&activity_tracker, |tracker, cx| {
                tracker.finish(activity_id, cx);
            });
        })
        .detach();
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

        let tab_bar = TabBar::new("editor-tab-bar")
            .selected_index(self.active_ix)
            .suffix(h_flex().gap_1().children(zoom_btn))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.active_ix = *ix;
                cx.notify();
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
                        .selected(is_active)
                        .closable(true)
                        .on_close(move |_window, cx| {
                            entity.update(cx, |this, cx| {
                                this.close_tab(ix, cx);
                            });
                        }),
                )
            });

        let active_name = self
            .data_source_manager
            .read(cx)
            .active_name()
            .map(|name| name.to_string());
        let active_connection = active_name.as_ref().map(|name| {
            let status = self.data_source_manager.read(cx).status(name);
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
                .when(!connections.is_empty(), |toolbar| {
                    let selected_name = active_name.clone();
                    let connection_label =
                        active_connection.unwrap_or_else(|| "No connection".to_string());
                    let connections = connections.clone();
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
                                    let label =
                                        format!("{} ({})", name, connection_status_label(*status));
                                    let view_for_item = view.clone();
                                    let is_selected =
                                        selected_name.as_deref() == Some(name.as_str());
                                    menu = menu.item(
                                        PopupMenuItem::new(label)
                                            .icon(Icon::new(IconName::File).path(
                                                ConnectionPanel::database_icon_path(config.db_type),
                                            ))
                                            .checked(is_selected)
                                            .on_click(window.listener_for(
                                                &view_for_item,
                                                move |this, _, window, cx| {
                                                    this.select_connection(
                                                        name.clone(),
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
