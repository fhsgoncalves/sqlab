use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, WeakEntity, Window, actions,
};
use gpui_component::dock::{DockArea, Panel, PanelEvent, PanelState};
use gpui_component::v_flex;

use crate::ui::panels::result::ResultPanel;
use crate::ui::panels::terminal::TerminalPanel;

actions!(bottom_panel, [ToggleBottomPanelMode]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BottomPanelMode {
    Results,
    Terminal,
}

pub struct BottomPanel {
    mode: BottomPanelMode,
    results_panel: Entity<ResultPanel>,
    terminal_panel: Entity<TerminalPanel>,
    focus_handle: FocusHandle,
    dock_area: Option<WeakEntity<DockArea>>,
}

impl BottomPanel {
    pub fn new(
        results_panel: Entity<ResultPanel>,
        terminal_panel: Entity<TerminalPanel>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            mode: BottomPanelMode::Results,
            results_panel,
            terminal_panel,
            focus_handle: cx.focus_handle(),
            dock_area: None,
        }
    }

    pub fn set_mode(&mut self, mode: BottomPanelMode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            cx.notify();
        }
    }

    pub fn mode(&self) -> BottomPanelMode {
        self.mode
    }

    pub fn results_panel(&self) -> &Entity<ResultPanel> {
        &self.results_panel
    }

    pub fn is_zoomed(&self, cx: &App) -> bool {
        match self.mode {
            BottomPanelMode::Results => self.results_panel.read(cx).is_zoomed(),
            BottomPanelMode::Terminal => self.terminal_panel.read(cx).is_zoomed(),
        }
    }

    pub fn set_zoomed(&mut self, zoomed: bool, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode {
            BottomPanelMode::Results => {
                self.results_panel.update(cx, |panel, cx| {
                    panel.set_zoomed(zoomed, window, cx);
                });
            }
            BottomPanelMode::Terminal => {
                self.terminal_panel.update(cx, |panel, cx| {
                    panel.set_zoomed(zoomed, window, cx);
                });
            }
        }
    }

    pub fn sync_zoomed_side_docks(&mut self, cx: &mut Context<Self>) {
        match self.mode {
            BottomPanelMode::Results => {
                self.results_panel.update(cx, |panel, cx| {
                    panel.sync_zoomed_side_docks(cx);
                });
            }
            BottomPanelMode::Terminal => {
                self.terminal_panel.update(cx, |panel, cx| {
                    panel.sync_zoomed_side_docks(cx);
                });
            }
        }
    }

    pub fn set_dock_area(&mut self, dock_area: WeakEntity<DockArea>, cx: &mut App) {
        self.dock_area = Some(dock_area.clone());
        self.results_panel.update(cx, |panel, _| {
            panel.set_dock_area(dock_area.clone());
        });
        self.terminal_panel.update(cx, |panel, _| {
            panel.set_dock_area(dock_area);
        });
    }
}

impl EventEmitter<PanelEvent> for BottomPanel {}

impl Panel for BottomPanel {
    fn panel_name(&self) -> &'static str {
        "BottomPanel"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        match self.mode {
            BottomPanelMode::Results => "Query Results",
            BottomPanelMode::Terminal => "Terminal",
        }
    }

    fn closable(&self, _cx: &App) -> bool {
        false
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState::new(self)
    }
}

impl Render for BottomPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("bottom-panel")
            .size_full()
            .track_focus(&self.focus_handle)
            .child(match self.mode {
                BottomPanelMode::Results => self.results_panel.clone().into_any_element(),
                BottomPanelMode::Terminal => self.terminal_panel.clone().into_any_element(),
            })
    }
}

impl Focusable for BottomPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        match self.mode {
            BottomPanelMode::Results => self.results_panel.focus_handle(_cx),
            BottomPanelMode::Terminal => self.terminal_panel.focus_handle(_cx),
        }
    }
}
