use std::path::PathBuf;

use gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, Styled, WeakEntity, Window, actions,
    div, hsla, prelude::FluentBuilder, px, rgb,
};
use gpui_component::{
    ActiveTheme, IconName, Sizable,
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockPlacement, Panel, PanelEvent, PanelState},
    h_flex, v_flex,
};

use super::editor::{EditorPanel, ExecuteQuery};
use crate::ui::components::tab::{Tab, TabBar};
use crate::ui::panels::diagram::{DiagramModel, DiagramPanel};
use sqlab_drivers_core::{ConnectionStatus, manager::DataSourceManager};

actions!(editor_tabs, [CycleTabForward, CycleTabBackward]);

pub struct EditorTabs {
    tabs: Vec<EditorTab>,
    active_ix: usize,
    focus_handle: FocusHandle,
    dock_area: Option<WeakEntity<DockArea>>,
    data_source_manager: Entity<DataSourceManager>,
    is_zoomed: bool,
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

        let diagram = cx.new(|cx| DiagramPanel::new(model, window, cx));
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

        let active_connection = self.data_source_manager.read(cx).active_name().map(|name| {
            let status = self.data_source_manager.read(cx).status(name);
            let status_label = match status {
                ConnectionStatus::Idle => "idle",
                ConnectionStatus::Connected => "connected",
                ConnectionStatus::Failed => "failed",
            };
            format!("{} ({})", name, status_label)
        });

        let active_connection_bg = if cx.theme().is_dark() {
            hsla(0.72, 0.72, 0.68, 0.22)
        } else {
            hsla(0.74, 0.55, 0.74, 0.48)
        };
        let active_connection_fg = if cx.theme().is_dark() {
            hsla(0.72, 0.90, 0.78, 1.0)
        } else {
            hsla(0.74, 0.70, 0.42, 1.0)
        };

        let editor_toolbar = h_flex()
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
            .when_some(active_connection, |toolbar, connection| {
                toolbar.child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded(cx.theme().radius)
                        .bg(active_connection_bg)
                        .text_color(active_connection_fg)
                        .text_xs()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .truncate()
                        .child(connection),
                )
            });

        v_flex()
            .id("editor-tabs")
            .size_full()
            .bg(cx.theme().background)
            .on_action(cx.listener(Self::cycle_tab_forward))
            .on_action(cx.listener(Self::cycle_tab_backward))
            .child(tab_bar)
            .child(editor_toolbar)
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

impl Focusable for EditorTabs {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
