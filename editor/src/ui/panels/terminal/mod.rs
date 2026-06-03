use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

use alacritty_terminal::event::{Event as TerminalEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config as TerminalConfig, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};
use async_channel::{Receiver, Sender};
use gpui::{
    App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement,
    IntoElement, KeyDownEvent, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Render, ScrollHandle, ScrollWheelEvent,
    StatefulInteractiveElement, Styled, TextRun, TextStyle, WeakEntity, WhiteSpace, Window,
    actions, canvas, div, fill, point, prelude::FluentBuilder, px, size,
};

use gpui_component::{
    ActiveTheme, IconName, Sizable,
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockPlacement, Panel, PanelEvent, PanelState},
    h_flex,
    menu::PopupMenuItem,
    scroll::{Scrollbar, ScrollbarHandle},
    v_flex,
};

use crate::ui::components::tab::{Tab, TabBar};

actions!(
    terminal_panel,
    [
        NewTerminalTab,
        CycleTabForward,
        CycleTabBackward,
        Paste,
        CopyTerminalSelection,
        CloseActiveTab
    ]
);

const CELL_WIDTH: f32 = 9.0;
const CELL_HEIGHT: f32 = 18.0;

pub struct TerminalPanel {
    sessions: Vec<TerminalSession>,
    active_ix: usize,
    next_session_id: usize,
    focus_handle: FocusHandle,
    event_tx: Sender<SessionEvent>,
    event_rx: Option<Receiver<SessionEvent>>,
    dock_area: Option<WeakEntity<DockArea>>,
    last_size: TerminalSize,
    working_directory: Option<PathBuf>,
    selection: Option<TerminalSelection>,
    selecting: bool,
    is_zoomed: bool,
    zoomed_side_docks: Option<ZoomedSideDocks>,
    tab_scroll_handle: ScrollHandle,
}

struct TerminalSession {
    id: usize,
    title: String,
    default_title_left: String,
    shell_name: String,
    backend: Option<TerminalBackend>,
}

#[derive(Clone, Copy)]
struct ZoomedSideDocks {
    left: bool,
    right: bool,
}

struct TerminalBackend {
    terminal: Arc<FairMutex<Term<TerminalEventProxy>>>,
    sender: EventLoopSender,
    _join_handle: JoinHandle<(
        EventLoop<tty::Pty, TerminalEventProxy>,
        alacritty_terminal::event_loop::State,
    )>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalPoint {
    row: usize,
    col: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalSelection {
    anchor: TerminalPoint,
    cursor: TerminalPoint,
}

impl TerminalSelection {
    fn normalized(&self) -> Option<(TerminalPoint, TerminalPoint)> {
        if self.anchor == self.cursor {
            return None;
        }

        if (self.anchor.row, self.anchor.col) <= (self.cursor.row, self.cursor.col) {
            Some((self.anchor, self.cursor))
        } else {
            Some((self.cursor, self.anchor))
        }
    }
}

#[derive(Clone)]
struct TerminalEventProxy {
    session_id: usize,
    tx: Sender<SessionEvent>,
}

#[derive(Clone)]
struct SessionEvent {
    session_id: usize,
    event: TerminalEvent,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct TerminalSize {
    columns: usize,
    lines: usize,
    cell_width: gpui::Pixels,
    cell_height: gpui::Pixels,
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.lines
    }

    fn screen_lines(&self) -> usize {
        self.lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

impl TerminalSize {
    fn window_size(self) -> WindowSize {
        WindowSize {
            num_lines: self.lines as u16,
            num_cols: self.columns as u16,
            cell_width: f32::from(self.cell_width) as u16,
            cell_height: f32::from(self.cell_height) as u16,
        }
    }
}

impl EventListener for TerminalEventProxy {
    fn send_event(&self, event: TerminalEvent) {
        let _ = self.tx.try_send(SessionEvent {
            session_id: self.session_id,
            event,
        });
    }
}

#[derive(Clone)]
struct TerminalScrollHandle {
    terminal: Arc<FairMutex<Term<TerminalEventProxy>>>,
    cell_height: gpui::Pixels,
}

impl ScrollbarHandle for TerminalScrollHandle {
    fn offset(&self) -> gpui::Point<gpui::Pixels> {
        let term = self.terminal.lock();
        let display_offset = term.grid().display_offset();
        let history_size = term.history_size();
        let lines_from_top = history_size.saturating_sub(display_offset);
        point(
            px(0.),
            px(-(lines_from_top as f32) * f32::from(self.cell_height)),
        )
    }

    fn set_offset(&self, offset: gpui::Point<gpui::Pixels>) {
        let mut term = self.terminal.lock();
        let current = term.grid().display_offset();
        let history_size = term.history_size();
        let lines_from_top = (-offset.y / self.cell_height).round() as usize;
        let target = history_size.saturating_sub(lines_from_top);
        if target != current {
            let delta = target as i32 - current as i32;
            term.scroll_display(Scroll::Delta(delta));
        }
    }

    fn content_size(&self) -> gpui::Size<gpui::Pixels> {
        let term = self.terminal.lock();
        let total = term.history_size() + term.screen_lines();
        size(px(0.), px(total as f32 * f32::from(self.cell_height)))
    }
}

impl Drop for TerminalBackend {
    fn drop(&mut self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}

impl TerminalPanel {
    pub fn new(
        working_directory: Option<PathBuf>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (event_tx, event_rx) = async_channel::unbounded();
        let last_size = TerminalSize {
            columns: 80,
            lines: 24,
            cell_width: px(CELL_WIDTH),
            cell_height: px(CELL_HEIGHT),
        };
        let mut panel = Self {
            sessions: Vec::new(),
            active_ix: 0,
            next_session_id: 1,
            focus_handle: cx.focus_handle(),
            event_tx,
            event_rx: Some(event_rx),
            dock_area: None,
            last_size,
            working_directory,
            selection: None,
            selecting: false,
            is_zoomed: false,
            zoomed_side_docks: None,
            tab_scroll_handle: ScrollHandle::default(),
        };
        panel.start_event_task(cx);
        if panel.working_directory.is_some() {
            panel.new_tab(cx);
        }
        panel
    }

    pub fn set_dock_area(&mut self, dock_area: WeakEntity<DockArea>) {
        self.dock_area = Some(dock_area);
    }

    pub fn set_working_directory(&mut self, dir: PathBuf) {
        self.working_directory = Some(dir);
    }

    pub fn sessions_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_zoomed(&self) -> bool {
        self.is_zoomed
    }

    pub fn sync_zoomed_side_docks(&mut self, cx: &App) {
        if !self.is_zoomed {
            return;
        }

        if let Some(dock_area) = self.dock_area.as_ref() {
            if let Some(dock_area) = dock_area.upgrade() {
                let dock_area = dock_area.read(cx);
                self.zoomed_side_docks = Some(ZoomedSideDocks {
                    left: dock_area.is_dock_open(DockPlacement::Left, cx),
                    right: dock_area.is_dock_open(DockPlacement::Right, cx),
                });
            }
        }
    }

    pub fn set_zoomed(&mut self, zoomed: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_zoomed == zoomed {
            return;
        }

        self.is_zoomed = zoomed;
        if let Some(dock_area) = self.dock_area.as_ref() {
            if let Some(dock_area) = dock_area.upgrade() {
                let panel = cx.entity();
                let saved_side_docks = self.zoomed_side_docks.take();
                let mut next_side_docks = None;
                dock_area.update(cx, |dock_area, cx| {
                    if zoomed {
                        let side_docks = ZoomedSideDocks {
                            left: dock_area.is_dock_open(DockPlacement::Left, cx),
                            right: dock_area.is_dock_open(DockPlacement::Right, cx),
                        };
                        if side_docks.left {
                            dock_area.toggle_dock(DockPlacement::Left, window, cx);
                        }
                        if side_docks.right {
                            dock_area.toggle_dock(DockPlacement::Right, window, cx);
                        }
                        next_side_docks = Some(side_docks);
                        dock_area.set_zoomed_in(panel.clone(), window, cx);
                    } else {
                        dock_area.set_zoomed_out(window, cx);
                        if let Some(side_docks) = saved_side_docks {
                            if dock_area.is_dock_open(DockPlacement::Left, cx) != side_docks.left {
                                dock_area.toggle_dock(DockPlacement::Left, window, cx);
                            }
                            if dock_area.is_dock_open(DockPlacement::Right, cx) != side_docks.right
                            {
                                dock_area.toggle_dock(DockPlacement::Right, window, cx);
                            }
                        }
                    }
                });
                self.zoomed_side_docks = next_side_docks;
            }
        }
        cx.notify();
    }

    fn toggle_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_zoomed(!self.is_zoomed, window, cx);
    }

    pub fn new_tab(&mut self, cx: &mut Context<Self>) {
        if self.working_directory.is_none() {
            return;
        }
        let id = self.next_session_id;
        self.next_session_id += 1;
        self.sessions.push(TerminalSession::new(
            id,
            self.last_size,
            self.event_tx.clone(),
            self.working_directory.clone(),
        ));
        self.active_ix = self.sessions.len().saturating_sub(1);
        self.scroll_to_active_tab();
        cx.notify();
    }

    pub fn ensure_has_tab(&mut self, cx: &mut Context<Self>) {
        if self.sessions.is_empty() {
            self.new_tab(cx);
        }
    }

    fn start_event_task(&mut self, cx: &mut Context<Self>) {
        let Some(rx) = self.event_rx.take() else {
            return;
        };

        cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let _ = this.update(cx, |panel, cx| {
                    panel.handle_session_event(event, cx);
                });
            }
        })
        .detach();
    }

    fn handle_session_event(&mut self, event: SessionEvent, cx: &mut Context<Self>) {
        let Some((ix, session)) = self
            .sessions
            .iter_mut()
            .enumerate()
            .find(|(_, session)| session.id == event.session_id)
        else {
            return;
        };

        match event.event {
            TerminalEvent::Title(title) => {
                session.title = session.format_title(Some(&title));
                cx.notify();
            }
            TerminalEvent::ResetTitle => {
                session.title = session.format_title(None);
                cx.notify();
            }
            TerminalEvent::PtyWrite(text) => {
                if let Some(backend) = &session.backend {
                    let _ = backend
                        .sender
                        .send(Msg::Input(Cow::Owned(text.into_bytes())));
                }
            }
            TerminalEvent::TextAreaSizeRequest(formatter) => {
                if let Some(backend) = &session.backend {
                    let text = formatter(self.last_size.window_size());
                    let _ = backend
                        .sender
                        .send(Msg::Input(Cow::Owned(text.into_bytes())));
                }
            }
            TerminalEvent::ChildExit(code) => {
                if code != 0 {
                    println!("Terminal process exited with code: {}", code);
                }
                self.close_tab(ix, cx);
            }
            TerminalEvent::Exit => {
                println!("Terminal process exited");
                self.close_tab(ix, cx);
            }
            TerminalEvent::Wakeup
            | TerminalEvent::Bell
            | TerminalEvent::MouseCursorDirty
            | TerminalEvent::CursorBlinkingChange
            | TerminalEvent::ClipboardStore(_, _)
            | TerminalEvent::ClipboardLoad(_, _)
            | TerminalEvent::ColorRequest(_, _) => {
                cx.notify();
            }
        }
    }

    fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.sessions.len() {
            return;
        }
        self.sessions.remove(ix);
        if self.sessions.is_empty() {
            cx.emit(PanelEvent::LayoutChanged);
        } else if self.active_ix >= self.sessions.len() {
            self.active_ix = self.sessions.len().saturating_sub(1);
        }
        self.scroll_to_active_tab();
        cx.notify();
    }

    fn close_other_tabs(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.sessions.len() || self.sessions.len() <= 1 {
            return;
        }

        let sessions = std::mem::take(&mut self.sessions);
        for (session_ix, session) in sessions.into_iter().enumerate() {
            if session_ix == ix {
                self.sessions.push(session);
            }
        }
        self.active_ix = 0;
        self.scroll_to_active_tab();
        cx.notify();
    }

    fn active_session_mut(&mut self) -> Option<&mut TerminalSession> {
        self.sessions.get_mut(self.active_ix)
    }

    fn active_session(&self) -> Option<&TerminalSession> {
        self.sessions.get(self.active_ix)
    }

    fn on_new_terminal_tab(
        &mut self,
        _: &NewTerminalTab,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_tab(cx);
    }

    fn on_cycle_tab_forward(
        &mut self,
        _: &CycleTabForward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sessions.len() > 1 {
            self.active_ix = (self.active_ix + 1) % self.sessions.len();
            self.scroll_to_active_tab();
            cx.notify();
            window.focus(&self.focus_handle, cx);
        }
    }

    fn on_cycle_tab_backward(
        &mut self,
        _: &CycleTabBackward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sessions.len() > 1 {
            self.active_ix = (self.active_ix + self.sessions.len() - 1) % self.sessions.len();
            self.scroll_to_active_tab();
            cx.notify();
            window.focus(&self.focus_handle, cx);
        }
    }

    fn scroll_to_active_tab(&self) {
        if self.sessions.is_empty() {
            return;
        }
        let mut tab_left = px(0.);
        let mut tab_width = px(0.);
        for (ix, session) in self.sessions.iter().enumerate() {
            let label = session.title.clone();
            let estimated_width = px(48.0 + label.chars().count() as f32 * 7.5);
            if ix == self.active_ix {
                tab_width = estimated_width;
                break;
            }
            tab_left += estimated_width;
        }
        let tab_right = tab_left + tab_width;

        let viewport = self.tab_scroll_handle.bounds();
        let viewport_width = viewport.size.width;
        if viewport_width > px(0.) {
            let current_offset = self.tab_scroll_handle.offset();
            let current_scroll_x = (-current_offset.x).max(px(0.));
            let viewport_right = current_scroll_x + viewport_width;

            let mut target_scroll_x = current_scroll_x;
            if tab_left < current_scroll_x {
                target_scroll_x = tab_left;
            } else if tab_right > viewport_right {
                target_scroll_x = (tab_right - viewport_width).max(px(0.));
            }

            if target_scroll_x != current_scroll_x {
                self.tab_scroll_handle
                    .set_offset(point(-target_scroll_x, px(0.)));
            }
        } else {
            self.tab_scroll_handle.set_offset(point(-tab_left, px(0.)));
        }
    }

    fn close_active_tab(
        &mut self,
        _: &CloseActiveTab,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_tab(self.active_ix, cx);
    }

    fn on_action_paste(&mut self, _: &Paste, _window: &mut Window, cx: &mut Context<Self>) {
        let clipboard = cx.read_from_clipboard();
        if let Some(item) = clipboard {
            if let Some(text) = item.text() {
                self.clear_selection(cx);
                let Some(session) = self.active_session_mut() else {
                    return;
                };
                if let Some(backend) = &session.backend {
                    let _ = backend
                        .sender
                        .send(Msg::Input(Cow::Owned(text.into_bytes())));
                    cx.notify();
                }
            }
        }
    }

    fn on_action_copy_terminal_selection(
        &mut self,
        _: &CopyTerminalSelection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.selected_text(cx) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn selected_text(&self, cx: &App) -> Option<String> {
        let selection = self.selection?.normalized()?;
        let lines = self.active_session()?.renderable_text_lines(cx);
        selected_text_from_lines(&lines, selection)
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if self.selection.is_some() || self.selecting {
            self.selection = None;
            self.selecting = false;
            cx.notify();
        }
    }

    fn reorder_tab(&mut self, from_ix: usize, to_ix: usize, cx: &mut Context<Self>) {
        if from_ix >= self.sessions.len() || to_ix >= self.sessions.len() || from_ix == to_ix {
            return;
        }
        let session = self.sessions.remove(from_ix);
        self.sessions.insert(to_ix, session);
        if self.active_ix == from_ix {
            self.active_ix = to_ix;
        } else if from_ix < self.active_ix && to_ix >= self.active_ix {
            self.active_ix -= 1;
        } else if from_ix > self.active_ix && to_ix <= self.active_ix {
            self.active_ix += 1;
        }
        self.scroll_to_active_tab();
        cx.notify();
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.modifiers.platform && event.keystroke.key == "t" {
            self.new_tab(cx);
            cx.stop_propagation();
            return;
        }

        if event.keystroke.modifiers.platform && event.keystroke.key == "d" {
            self.close_tab(self.active_ix, cx);
            cx.stop_propagation();
            return;
        }

        if event.keystroke.key != "c" || !event.keystroke.modifiers.control {
            self.clear_selection(cx);
        }

        let Some(session) = self.active_session_mut() else {
            return;
        };

        if session.write_key(&event.keystroke) {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn begin_selection(
        &mut self,
        point: TerminalPoint,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selection = Some(TerminalSelection {
            anchor: point,
            cursor: point,
        });
        self.selecting = true;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn update_selection(&mut self, point: TerminalPoint, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }

        if let Some(selection) = self.selection.as_mut() {
            if selection.cursor != point {
                selection.cursor = point;
                cx.notify();
            }
        }
    }

    fn finish_selection(&mut self, point: TerminalPoint, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }

        if let Some(selection) = self.selection.as_mut() {
            selection.cursor = point;
            if selection.normalized().is_none() {
                self.selection = None;
            }
        }

        self.selecting = false;
        cx.notify();
    }

    #[allow(dead_code)]
    fn handle_resize(&mut self, size: TerminalSize, cx: &mut Context<Self>) {
        self.resize_sessions(size);
        cx.notify();
    }

    fn resize_sessions(&mut self, size: TerminalSize) {
        if self.last_size == size {
            return;
        }
        self.last_size = size;
        for session in &mut self.sessions {
            if let Some(backend) = &session.backend {
                let mut terminal = backend.terminal.lock();
                terminal.resize(size);
                let _ = backend.sender.send(Msg::Resize(size.window_size()));
            }
        }
    }

    fn build_terminal_paint_state(
        &mut self,
        active_ix: usize,
        bounds: gpui::Bounds<gpui::Pixels>,
        window: &mut Window,
        cx: &App,
    ) -> TerminalPaintState {
        let text_style = terminal_text_style(window, cx);
        let cell_width = terminal_cell_width(&text_style, window, cx);
        let line_height = text_style.line_height_in_pixels(window.rem_size());
        let columns = (bounds.size.width / cell_width).floor().max(1.0) as usize;
        let lines = (bounds.size.height / line_height).floor().max(1.0) as usize;
        self.resize_sessions(TerminalSize {
            columns,
            lines,
            cell_width,
            cell_height: line_height,
        });

        let lines = self
            .sessions
            .get(active_ix)
            .map(|session| {
                session
                    .renderable_cells(cx)
                    .into_iter()
                    .map(group_cells_by_style)
                    .collect()
            })
            .unwrap_or_default();

        TerminalPaintState {
            cell_width,
            line_height,
            text_style,
            lines,
            background: cx.theme().background,
            selection: self.selection.and_then(|selection| selection.normalized()),
        }
    }
}

#[derive(Clone)]
struct StyledCell {
    c: char,
    fg: Hsla,
    bg: Option<Hsla>,
    bold: bool,
    _italic: bool,
    _underline: bool,
}

impl TerminalSession {
    fn new(
        id: usize,
        size: TerminalSize,
        tx: Sender<SessionEvent>,
        working_directory: Option<PathBuf>,
    ) -> Self {
        let default_title_left = terminal_default_title_left(working_directory.as_deref());
        let shell_name = terminal_shell_name();
        let title = terminal_tab_title(&default_title_left, &shell_name);

        match TerminalBackend::new(id, size, tx, working_directory) {
            Ok(backend) => Self {
                id,
                title,
                default_title_left,
                shell_name,
                backend: Some(backend),
            },
            Err(_) => Self {
                id,
                title,
                default_title_left,
                shell_name,
                backend: None,
            },
        }
    }

    fn format_title(&self, title: Option<&str>) -> String {
        let title_left = title
            .and_then(terminal_title_left)
            .unwrap_or(&self.default_title_left);

        terminal_tab_title(title_left, &self.shell_name)
    }

    fn write_key(&mut self, keystroke: &Keystroke) -> bool {
        let Some(backend) = &self.backend else {
            return false;
        };

        let mode = backend.terminal.lock().mode().to_owned();
        let Some(text) = key_to_esc_str(keystroke, &mode) else {
            return false;
        };

        let _ = backend
            .sender
            .send(Msg::Input(Cow::Owned(text.into_owned().into_bytes())));
        true
    }

    fn renderable_cells(&self, cx: &App) -> Vec<Vec<StyledCell>> {
        let Some(backend) = &self.backend else {
            return Vec::new();
        };

        let terminal = backend.terminal.lock();
        let content = terminal.renderable_content();
        let palette = Palette::default();
        let default_cell = StyledCell {
            c: ' ',
            fg: cx.theme().foreground,
            bg: None,
            bold: false,
            _italic: false,
            _underline: false,
        };
        let mut lines =
            vec![vec![default_cell.clone(); terminal.columns()]; terminal.screen_lines()];

        for indexed in content.display_iter {
            let row = indexed.point.line.0 + content.display_offset as i32;
            let Ok(row) = usize::try_from(row) else {
                continue;
            };
            if row >= lines.len() {
                continue;
            }

            let col = indexed.point.column.0;
            let Some(line) = lines.get_mut(row) else {
                continue;
            };
            if col >= line.len() {
                continue;
            }

            let cell = indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }

            let mut ch = cell.c;
            if ch == '\0' {
                ch = ' ';
            }

            let fg = terminal_color_to_gpui(cell.fg, &palette, cx);
            let bg = if cell.bg == AnsiColor::Named(NamedColor::Background) {
                None
            } else {
                Some(terminal_color_to_gpui(cell.bg, &palette, cx))
            };

            if let Some(slot) = line.get_mut(col) {
                *slot = StyledCell {
                    c: ch,
                    fg,
                    bg,
                    bold: cell.flags.contains(Flags::BOLD),
                    _italic: cell.flags.contains(Flags::ITALIC),
                    _underline: cell.flags.contains(Flags::UNDERLINE),
                };
            }
        }

        // Handle cursor
        let cursor_row = content.cursor.point.line.0 + content.display_offset as i32;
        if let Ok(cursor_row) = usize::try_from(cursor_row)
            && let Some(line) = lines.get_mut(cursor_row)
        {
            let cursor_col = content.cursor.point.column.0;
            if cursor_col < line.len() {
                if let Some(cell) = line.get_mut(cursor_col) {
                    if cell.c == ' ' && cell.bg.is_none() {
                        cell.fg = cx.theme().accent_foreground;
                        cell.bg = Some(cx.theme().accent);
                    } else {
                        let cursor_fg = cell.bg.unwrap_or(cx.theme().background);
                        let cursor_bg = cell.fg;
                        cell.fg = cursor_fg;
                        cell.bg = Some(cursor_bg);
                    }
                }
            } else if cursor_col == line.len() {
                // Cursor at end of line
                line.push(StyledCell {
                    c: ' ',
                    fg: cx.theme().accent_foreground,
                    bg: Some(cx.theme().accent),
                    bold: false,
                    _italic: false,
                    _underline: false,
                });
            }
        }

        lines
    }

    fn renderable_text_lines(&self, cx: &App) -> Vec<String> {
        self.renderable_cells(cx)
            .into_iter()
            .map(|line| line.into_iter().map(|cell| cell.c).collect())
            .collect()
    }
}

struct Palette {
    black: Hsla,
    red: Hsla,
    green: Hsla,
    yellow: Hsla,
    blue: Hsla,
    magenta: Hsla,
    cyan: Hsla,
    white: Hsla,
    bright_black: Hsla,
    bright_red: Hsla,
    bright_green: Hsla,
    bright_yellow: Hsla,
    bright_blue: Hsla,
    bright_magenta: Hsla,
    bright_cyan: Hsla,
    bright_white: Hsla,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            black: gpui::rgb(0x21262d).into(),
            red: gpui::rgb(0xe06c75).into(),
            green: gpui::rgb(0x98c379).into(),
            yellow: gpui::rgb(0xe5c07b).into(),
            blue: gpui::rgb(0x61afef).into(),
            magenta: gpui::rgb(0xc678dd).into(),
            cyan: gpui::rgb(0x56b6c2).into(),
            white: gpui::rgb(0xdcdfe4).into(),
            bright_black: gpui::rgb(0x5c6370).into(),
            bright_red: gpui::rgb(0xff6b6b).into(),
            bright_green: gpui::rgb(0xa3be8c).into(),
            bright_yellow: gpui::rgb(0xf0d98c).into(),
            bright_blue: gpui::rgb(0x61afef).into(),
            bright_magenta: gpui::rgb(0xd16d9e).into(),
            bright_cyan: gpui::rgb(0x8bd5ca).into(),
            bright_white: gpui::rgb(0xffffff).into(),
        }
    }
}

fn terminal_color_to_gpui(color: AnsiColor, palette: &Palette, cx: &App) -> Hsla {
    match color {
        AnsiColor::Named(named) => match named {
            NamedColor::Black => palette.black,
            NamedColor::Red => palette.red,
            NamedColor::Green => palette.green,
            NamedColor::Yellow => palette.yellow,
            NamedColor::Blue => palette.blue,
            NamedColor::Magenta => palette.magenta,
            NamedColor::Cyan => palette.cyan,
            NamedColor::White => palette.white,
            NamedColor::BrightBlack => palette.bright_black,
            NamedColor::BrightRed => palette.bright_red,
            NamedColor::BrightGreen => palette.bright_green,
            NamedColor::BrightYellow => palette.bright_yellow,
            NamedColor::BrightBlue => palette.bright_blue,
            NamedColor::BrightMagenta => palette.bright_magenta,
            NamedColor::BrightCyan => palette.bright_cyan,
            NamedColor::BrightWhite => palette.bright_white,
            NamedColor::Foreground => cx.theme().foreground,
            NamedColor::Background => cx.theme().background,
            NamedColor::DimBlack => palette.bright_black,
            NamedColor::DimRed => palette.red,
            NamedColor::DimGreen => palette.green,
            NamedColor::DimYellow => palette.yellow,
            NamedColor::DimBlue => palette.blue,
            NamedColor::DimMagenta => palette.magenta,
            NamedColor::DimCyan => palette.cyan,
            NamedColor::DimWhite => palette.white,
            NamedColor::BrightForeground => cx.theme().foreground,
            NamedColor::DimForeground => cx.theme().foreground,
            _ => cx.theme().foreground,
        },
        AnsiColor::Indexed(idx) => {
            if idx < 16 {
                match idx {
                    0 => palette.black,
                    1 => palette.red,
                    2 => palette.green,
                    3 => palette.yellow,
                    4 => palette.blue,
                    5 => palette.magenta,
                    6 => palette.cyan,
                    7 => palette.white,
                    8 => palette.bright_black,
                    9 => palette.bright_red,
                    10 => palette.bright_green,
                    11 => palette.bright_yellow,
                    12 => palette.bright_blue,
                    13 => palette.bright_magenta,
                    14 => palette.bright_cyan,
                    15 => palette.bright_white,
                    _ => cx.theme().foreground,
                }
            } else {
                gpui::rgb(0xcccccc).into()
            }
        }
        AnsiColor::Spec(rgb) => {
            gpui::rgb((rgb.r as u32) << 16 | (rgb.g as u32) << 8 | (rgb.b as u32)).into()
        }
    }
}

fn terminal_default_title_left(working_directory: Option<&Path>) -> String {
    working_directory
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("Terminal")
        .to_string()
}

fn terminal_shell_name() -> String {
    std::env::var("SHELL")
        .ok()
        .and_then(|shell| {
            Path::new(&shell)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "shell".to_string())
}

fn terminal_title_left(title: &str) -> Option<&str> {
    let title = title
        .split_once(" — ")
        .map(|(left, _)| left)
        .unwrap_or(title)
        .split_whitespace()
        .next()?;

    if title.is_empty() {
        return None;
    }

    title
        .split_once(':')
        .and_then(|(_, path)| terminal_path_leaf(path))
        .or_else(|| terminal_path_leaf(title))
        .or(Some(title))
}

fn terminal_path_leaf(path: &str) -> Option<&str> {
    path.trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|leaf| !leaf.is_empty() && *leaf != "~")
}

fn terminal_tab_title(title_left: &str, shell_name: &str) -> String {
    format!("{} — {}", title_left.trim(), shell_name.trim())
}

impl TerminalBackend {
    fn new(
        id: usize,
        size: TerminalSize,
        tx: Sender<SessionEvent>,
        working_directory: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        tty::setup_env();
        let proxy = TerminalEventProxy { session_id: id, tx };
        let terminal = Arc::new(FairMutex::new(Term::new(
            TerminalConfig::default(),
            &size,
            proxy.clone(),
        )));
        let pty = tty::new(
            &tty::Options {
                working_directory,
                ..Default::default()
            },
            size.window_size(),
            id as u64,
        )?;
        let event_loop = EventLoop::new(terminal.clone(), proxy, pty, true, false)?;
        let sender = event_loop.channel();
        let _join_handle = event_loop.spawn();
        Ok(Self {
            terminal,
            sender,
            _join_handle,
        })
    }
}

fn key_to_esc_str(keystroke: &Keystroke, mode: &TermMode) -> Option<Cow<'static, str>> {
    if keystroke.modifiers.platform {
        return None;
    }

    let ctrl = keystroke.modifiers.control;
    let alt = keystroke.modifiers.alt;
    let shift = keystroke.modifiers.shift;

    let manual = match (keystroke.key.as_str(), ctrl, alt, shift) {
        ("tab", false, false, false) => Some(Cow::Borrowed("\x09")),
        ("tab", false, false, true) => Some(Cow::Borrowed("\x1b[Z")),
        ("escape", false, false, false) => Some(Cow::Borrowed("\x1b")),
        ("enter", false, false, false) => {
            if mode.contains(TermMode::LINE_FEED_NEW_LINE) {
                Some(Cow::Borrowed("\x0d\x0a"))
            } else {
                Some(Cow::Borrowed("\x0d"))
            }
        }
        ("enter", false, false, true) => Some(Cow::Borrowed("\x0a")),
        ("enter", false, true, false) => Some(Cow::Borrowed("\x1b\x0d")),
        ("backspace", false, false, _) => Some(Cow::Borrowed("\x7f")),
        ("backspace", true, false, _) => Some(Cow::Borrowed("\x08")),
        ("backspace", false, true, _) => Some(Cow::Borrowed("\x1b\x7f")),
        ("space", true, false, _) => Some(Cow::Borrowed("\x00")),
        ("home", false, false, _) if mode.contains(TermMode::APP_CURSOR) => {
            Some(Cow::Borrowed("\x1bOH"))
        }
        ("home", false, false, _) => Some(Cow::Borrowed("\x1b[H")),
        ("end", false, false, _) if mode.contains(TermMode::APP_CURSOR) => {
            Some(Cow::Borrowed("\x1bOF"))
        }
        ("end", false, false, _) => Some(Cow::Borrowed("\x1b[F")),
        ("up", false, false, _) if mode.contains(TermMode::APP_CURSOR) => {
            Some(Cow::Borrowed("\x1bOA"))
        }
        ("up", false, false, _) => Some(Cow::Borrowed("\x1b[A")),
        ("down", false, false, _) if mode.contains(TermMode::APP_CURSOR) => {
            Some(Cow::Borrowed("\x1bOB"))
        }
        ("down", false, false, _) => Some(Cow::Borrowed("\x1b[B")),
        ("right", false, false, _) if mode.contains(TermMode::APP_CURSOR) => {
            Some(Cow::Borrowed("\x1bOC"))
        }
        ("right", false, false, _) => Some(Cow::Borrowed("\x1b[C")),
        ("left", false, false, _) if mode.contains(TermMode::APP_CURSOR) => {
            Some(Cow::Borrowed("\x1bOD"))
        }
        ("left", false, false, _) => Some(Cow::Borrowed("\x1b[D")),
        ("insert", false, false, _) => Some(Cow::Borrowed("\x1b[2~")),
        ("delete", false, false, _) => Some(Cow::Borrowed("\x1b[3~")),
        ("pageup", false, false, _) => Some(Cow::Borrowed("\x1b[5~")),
        ("pagedown", false, false, _) => Some(Cow::Borrowed("\x1b[6~")),
        ("f1", false, false, _) => Some(Cow::Borrowed("\x1bOP")),
        ("f2", false, false, _) => Some(Cow::Borrowed("\x1bOQ")),
        ("f3", false, false, _) => Some(Cow::Borrowed("\x1bOR")),
        ("f4", false, false, _) => Some(Cow::Borrowed("\x1bOS")),
        ("f5", false, false, _) => Some(Cow::Borrowed("\x1b[15~")),
        ("f6", false, false, _) => Some(Cow::Borrowed("\x1b[17~")),
        ("f7", false, false, _) => Some(Cow::Borrowed("\x1b[18~")),
        ("f8", false, false, _) => Some(Cow::Borrowed("\x1b[19~")),
        ("f9", false, false, _) => Some(Cow::Borrowed("\x1b[20~")),
        ("f10", false, false, _) => Some(Cow::Borrowed("\x1b[21~")),
        ("f11", false, false, _) => Some(Cow::Borrowed("\x1b[23~")),
        ("f12", false, false, _) => Some(Cow::Borrowed("\x1b[24~")),
        _ => None,
    };

    if let Some(manual) = manual {
        return Some(manual);
    }

    // Common shell word-navigation bindings on macOS terminal emulators.
    if alt && !ctrl {
        match keystroke.key.as_str() {
            "left" => return Some(Cow::Borrowed("\x1bb")),
            "right" => return Some(Cow::Borrowed("\x1bf")),
            _ => {}
        }
    }

    if let Some(key_char) = &keystroke.key_char {
        if !ctrl {
            if alt {
                return Some(Cow::Owned(format!("\x1b{}", key_char)));
            }
            return Some(Cow::Owned(key_char.clone()));
        }
    }

    if ctrl || alt || shift {
        if let Some(sequence) = ctrl_key_sequence(keystroke) {
            return Some(Cow::Borrowed(sequence));
        }
        modified_key_sequence(keystroke).map(Cow::Owned)
    } else {
        None
    }
}

fn ctrl_key_sequence(keystroke: &Keystroke) -> Option<&'static str> {
    if !keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt {
        return None;
    }

    match keystroke.key.as_str() {
        "a" | "A" => Some("\x01"),
        "b" | "B" => Some("\x02"),
        "c" | "C" => Some("\x03"),
        "d" | "D" => Some("\x04"),
        "e" | "E" => Some("\x05"),
        "f" | "F" => Some("\x06"),
        "g" | "G" => Some("\x07"),
        "h" | "H" => Some("\x08"),
        "i" | "I" => Some("\x09"),
        "j" | "J" => Some("\x0a"),
        "k" | "K" => Some("\x0b"),
        "l" | "L" => Some("\x0c"),
        "m" | "M" => Some("\x0d"),
        "n" | "N" => Some("\x0e"),
        "o" | "O" => Some("\x0f"),
        "p" | "P" => Some("\x10"),
        "q" | "Q" => Some("\x11"),
        "r" | "R" => Some("\x12"),
        "s" | "S" => Some("\x13"),
        "t" | "T" => Some("\x14"),
        "u" | "U" => Some("\x15"),
        "v" | "V" => Some("\x16"),
        "w" | "W" => Some("\x17"),
        "x" | "X" => Some("\x18"),
        "y" | "Y" => Some("\x19"),
        "z" | "Z" => Some("\x1a"),
        "@" => Some("\x00"),
        "[" => Some("\x1b"),
        "\\" => Some("\x1c"),
        "]" => Some("\x1d"),
        "^" => Some("\x1e"),
        "_" => Some("\x1f"),
        "?" => Some("\x7f"),
        _ => None,
    }
}

fn modified_key_sequence(keystroke: &Keystroke) -> Option<String> {
    let code = modifier_code(keystroke);
    match keystroke.key.as_str() {
        "up" => Some(format!("\x1b[1;{}A", code)),
        "down" => Some(format!("\x1b[1;{}B", code)),
        "right" => Some(format!("\x1b[1;{}C", code)),
        "left" => Some(format!("\x1b[1;{}D", code)),
        "insert" => Some(format!("\x1b[2;{}~", code)),
        "delete" => Some(format!("\x1b[3;{}~", code)),
        "pageup" => Some(format!("\x1b[5;{}~", code)),
        "pagedown" => Some(format!("\x1b[6;{}~", code)),
        "end" => Some(format!("\x1b[1;{}F", code)),
        "home" => Some(format!("\x1b[1;{}H", code)),
        _ => None,
    }
}

fn modifier_code(keystroke: &Keystroke) -> u32 {
    let mut code = 0;
    if keystroke.modifiers.shift {
        code |= 1;
    }
    if keystroke.modifiers.alt {
        code |= 1 << 1;
    }
    if keystroke.modifiers.control {
        code |= 1 << 2;
    }
    code + 1
}

impl EventEmitter<PanelEvent> for TerminalPanel {}

impl Panel for TerminalPanel {
    fn panel_name(&self) -> &'static str {
        "TerminalPanel"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        "Terminal"
    }

    fn closable(&self, _cx: &App) -> bool {
        false
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState::new(self)
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let active_ix = self.active_ix;

        let text_style = terminal_text_style(window, cx);
        let line_height = text_style.line_height_in_pixels(window.rem_size());

        let scroll_handle: Option<TerminalScrollHandle> = self
            .sessions
            .get(active_ix)
            .and_then(|s| s.backend.as_ref())
            .map(|backend| TerminalScrollHandle {
                terminal: backend.terminal.clone(),
                cell_height: line_height,
            });

        let scroll_size = scroll_handle
            .as_ref()
            .map(|h| h.content_size())
            .unwrap_or(size(px(0.), px(0.)));

        let sh_for_paint = scroll_handle.clone();

        let tab_bar = TabBar::new("terminal-tab-bar")
            .selected_index(active_ix)
            .scroll_handle(self.tab_scroll_handle.clone())
            .on_click(cx.listener(|this, ix: &usize, _window, cx| {
                this.active_ix = *ix;
                this.scroll_to_active_tab();
                cx.notify();
            }))
            .on_reorder(cx.listener(|this, (from_ix, to_ix), _, cx| {
                this.reorder_tab(*from_ix, *to_ix, cx);
            }))
            .suffix(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("terminal-zoom")
                            .icon(if self.is_zoomed {
                                IconName::Minimize
                            } else {
                                IconName::Maximize
                            })
                            .xsmall()
                            .ghost()
                            .tooltip(if self.is_zoomed {
                                "Restore"
                            } else {
                                "Maximize"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_zoom(window, cx);
                            })),
                    )
                    .child(
                        Button::new("new-terminal-tab")
                            .icon(IconName::Plus)
                            .xsmall()
                            .ghost()
                            .tooltip("New Terminal")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.new_tab(cx);
                            })),
                    ),
            );

        let tab_bar = self
            .sessions
            .iter()
            .enumerate()
            .fold(tab_bar, |tab_bar, (ix, session)| {
                let entity_for_close = entity.clone();
                let entity_for_menu = entity.clone();
                let can_close_others = self.sessions.len() > 1;
                tab_bar.child(
                    Tab::new()
                        .label(session.title.clone())
                        .selected(ix == active_ix)
                        .closable(true)
                        .on_close(move |_window, cx| {
                            entity_for_close.update(cx, |this, cx| {
                                this.close_tab(ix, cx);
                            });
                        })
                        .context_menu(move |menu, _window, _cx| {
                            let entity = entity_for_menu.clone();
                            menu.item(
                                PopupMenuItem::new("Close others")
                                    .disabled(!can_close_others)
                                    .on_click(move |_, _window, cx| {
                                        entity.update(cx, |this, cx| {
                                            this.close_other_tabs(ix, cx);
                                        });
                                    }),
                            )
                        }),
                )
            });

        v_flex()
            .id("terminal-panel")
            .key_context("terminal_panel")
            .size_full()
            .bg(cx.theme().background)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_new_terminal_tab))
            .on_action(cx.listener(Self::on_cycle_tab_forward))
            .on_action(cx.listener(Self::on_cycle_tab_backward))
            .on_action(cx.listener(Self::on_action_paste))
            .on_action(cx.listener(Self::on_action_copy_terminal_selection))
            .on_action(cx.listener(Self::close_active_tab))
            .capture_key_down(cx.listener(Self::handle_key_down))
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.focus_handle, cx);
            }))
            .child(tab_bar)
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_w_full()
                    .relative()
                    .track_focus(&self.focus_handle)
                    .bg(cx.theme().background)
                    .p_2()
                    .child(
                        div()
                            .size_full()
                            .relative()
                            .child(
                                canvas(
                                    {
                                        let entity = entity.clone();
                                        move |bounds, window, cx| {
                                            entity.update(cx, |this, cx| {
                                                this.build_terminal_paint_state(
                                                    active_ix, bounds, window, cx,
                                                )
                                            })
                                        }
                                    },
                                    {
                                        let sh = sh_for_paint;
                                        move |bounds, state, window, cx| {
                                            let scale_factor = window.scale_factor();
                                            let snap_px = |value: gpui::Pixels| {
                                                gpui::Pixels::from(
                                                    (f32::from(value) * scale_factor).floor()
                                                        / scale_factor,
                                                )
                                            };
                                            window.paint_quad(fill(bounds, state.background));
                                            let origin = point(
                                                snap_px(bounds.origin.x),
                                                snap_px(bounds.origin.y),
                                            );

                                            for (line_ix, line) in state.lines.iter().enumerate() {
                                                for span in line {
                                                    if let Some(bg) = span.bg {
                                                        let x = snap_px(
                                                            origin.x
                                                                + span.start_col as f32
                                                                    * state.cell_width,
                                                        );
                                                        let y = snap_px(
                                                            origin.y
                                                                + line_ix as f32
                                                                    * state.line_height,
                                                        );
                                                        window.paint_quad(fill(
                                                            gpui::Bounds::new(
                                                                point(x, y),
                                                                size(
                                                                    (state.cell_width
                                                                        * span.cell_count as f32)
                                                                        .ceil(),
                                                                    state.line_height,
                                                                ),
                                                            ),
                                                            bg,
                                                        ));
                                                    }

                                                    let mut run_font = state.text_style.font();
                                                    if span.bold {
                                                        run_font = run_font.bold();
                                                    }
                                                    if span.italic {
                                                        run_font = run_font.italic();
                                                    }

                                                    let run = TextRun {
                                                        len: span.text.len(),
                                                        font: run_font,
                                                        color: span.fg,
                                                        background_color: None,
                                                        underline: None,
                                                        strikethrough: None,
                                                    };

                                                    let shaped = window.text_system().shape_line(
                                                        span.text.clone().into(),
                                                        state
                                                            .text_style
                                                            .font_size
                                                            .to_pixels(window.rem_size()),
                                                        &[run],
                                                        Some(state.cell_width),
                                                    );

                                                    let x = snap_px(
                                                        origin.x
                                                            + span.start_col as f32
                                                                * state.cell_width,
                                                    );
                                                    let y = snap_px(
                                                        origin.y
                                                            + line_ix as f32 * state.line_height,
                                                    );
                                                    let _ = shaped.paint(
                                                        point(x, y),
                                                        state.line_height,
                                                        gpui::TextAlign::Left,
                                                        None,
                                                        window,
                                                        cx,
                                                    );
                                                }
                                            }

                                            if let Some((start, end)) = state.selection {
                                                for (row, start_col, end_col) in
                                                    selected_cell_ranges(
                                                        state.lines.len(),
                                                        state.columns(),
                                                        start,
                                                        end,
                                                    )
                                                {
                                                    let x = snap_px(
                                                        origin.x
                                                            + start_col as f32 * state.cell_width,
                                                    );
                                                    let y = snap_px(
                                                        origin.y + row as f32 * state.line_height,
                                                    );
                                                    window.paint_quad(fill(
                                                        gpui::Bounds::new(
                                                            point(x, y),
                                                            size(
                                                                (state.cell_width
                                                                    * (end_col - start_col) as f32)
                                                                    .ceil(),
                                                                state.line_height,
                                                            ),
                                                        ),
                                                        cx.theme().selection,
                                                    ));
                                                }
                                            }

                                            if let Some(ref handle) = sh {
                                                let handle = handle.clone();
                                                let line_height = state.line_height;
                                                let view_id = window.current_view();
                                                window.on_mouse_event(
                                                    move |event: &ScrollWheelEvent,
                                                          phase,
                                                          _,
                                                          cx| {
                                                        if !(bounds.contains(&event.position)
                                                            && phase.bubble())
                                                        {
                                                            return;
                                                        }
                                                        let mut offset = handle.offset();
                                                        let delta = event
                                                            .delta
                                                            .pixel_delta(line_height);
                                                        offset.y += 3.0_f32 * delta.y;
                                                        if offset != handle.offset() {
                                                            handle.set_offset(offset);
                                                            cx.notify(view_id);
                                                            cx.stop_propagation();
                                                        }
                                                    },
                                                );
                                            }

                                            let terminal_geometry = TerminalGeometry {
                                                bounds,
                                                cell_width: state.cell_width,
                                                line_height: state.line_height,
                                                columns: state.columns(),
                                                rows: state.lines.len(),
                                            };

                                            let selection_entity = entity.clone();
                                            window.on_mouse_event(
                                                move |event: &MouseDownEvent, phase, window, cx| {
                                                    if !(phase.bubble()
                                                        && event.button == MouseButton::Left
                                                        && terminal_geometry
                                                            .bounds
                                                            .contains(&event.position))
                                                    {
                                                        return;
                                                    }

                                                    let point = terminal_geometry
                                                        .point_for_position(event.position);
                                                    selection_entity.update(cx, |this, cx| {
                                                        this.begin_selection(point, window, cx);
                                                    });
                                                    cx.stop_propagation();
                                                },
                                            );

                                            let selection_entity = entity.clone();
                                            window.on_mouse_event(
                                                move |event: &MouseMoveEvent, _, _, cx| {
                                                    if event.pressed_button
                                                        != Some(MouseButton::Left)
                                                    {
                                                        return;
                                                    }

                                                    let point = terminal_geometry
                                                        .point_for_position(event.position);
                                                    selection_entity.update(cx, |this, cx| {
                                                        if this.selecting {
                                                            this.update_selection(point, cx);
                                                            cx.stop_propagation();
                                                        }
                                                    });
                                                },
                                            );

                                            let selection_entity = entity.clone();
                                            window.on_mouse_event(
                                                move |event: &MouseUpEvent, phase, _, cx| {
                                                    if !(phase.bubble()
                                                        && event.button == MouseButton::Left)
                                                    {
                                                        return;
                                                    }

                                                    let point = terminal_geometry
                                                        .point_for_position(event.position);
                                                    selection_entity.update(cx, |this, cx| {
                                                        if this.selecting {
                                                            this.finish_selection(point, cx);
                                                            cx.stop_propagation();
                                                        }
                                                    });
                                                },
                                            );
                                        }
                                    },
                                )
                                .size_full()
                                .cursor_text(),
                            )
                            .when_some(scroll_handle, |parent, handle| {
                                parent.child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .right_0()
                                        .bottom_0()
                                        .child(
                                            Scrollbar::vertical(&handle).scroll_size(scroll_size),
                                        ),
                                )
                            }),
                    ),
            )
    }
}

struct StyledSpan {
    start_col: usize,
    cell_count: usize,
    text: String,
    fg: Hsla,
    bg: Option<Hsla>,
    bold: bool,
    italic: bool,
    underline: bool,
}

struct TerminalPaintState {
    cell_width: gpui::Pixels,
    line_height: gpui::Pixels,
    text_style: TextStyle,
    lines: Vec<Vec<StyledSpan>>,
    background: Hsla,
    selection: Option<(TerminalPoint, TerminalPoint)>,
}

impl TerminalPaintState {
    fn columns(&self) -> usize {
        self.lines
            .iter()
            .flat_map(|line| {
                line.iter()
                    .map(|span| span.start_col.saturating_add(span.cell_count))
            })
            .max()
            .unwrap_or(0)
    }
}

#[derive(Clone, Copy)]
struct TerminalGeometry {
    bounds: gpui::Bounds<gpui::Pixels>,
    cell_width: gpui::Pixels,
    line_height: gpui::Pixels,
    columns: usize,
    rows: usize,
}

impl TerminalGeometry {
    fn point_for_position(&self, position: gpui::Point<gpui::Pixels>) -> TerminalPoint {
        let rows = self.rows.max(1);
        let columns = self.columns.max(1);
        let relative_x = f32::from(position.x - self.bounds.origin.x);
        let relative_y = f32::from(position.y - self.bounds.origin.y);

        let row = (relative_y / f32::from(self.line_height)).floor() as isize;
        let col = (relative_x / f32::from(self.cell_width)).round() as isize;

        TerminalPoint {
            row: row.clamp(0, rows.saturating_sub(1) as isize) as usize,
            col: col.clamp(0, columns as isize) as usize,
        }
    }
}

fn selected_cell_ranges(
    rows: usize,
    columns: usize,
    start: TerminalPoint,
    end: TerminalPoint,
) -> Vec<(usize, usize, usize)> {
    if rows == 0 || columns == 0 {
        return Vec::new();
    }

    let start_row = start.row.min(rows.saturating_sub(1));
    let end_row = end.row.min(rows.saturating_sub(1));
    if start_row > end_row {
        return Vec::new();
    }

    let mut ranges = Vec::new();
    for row in start_row..=end_row {
        let start_col = if row == start_row {
            start.col.min(columns)
        } else {
            0
        };
        let end_col = if row == end_row {
            end.col.min(columns)
        } else {
            columns
        };

        if start_col < end_col {
            ranges.push((row, start_col, end_col));
        }
    }
    ranges
}

fn selected_text_from_lines(
    lines: &[String],
    (start, end): (TerminalPoint, TerminalPoint),
) -> Option<String> {
    if lines.is_empty() || start.row >= lines.len() {
        return None;
    }

    let end_row = end.row.min(lines.len().saturating_sub(1));
    if start.row > end_row {
        return None;
    }

    let mut selected_lines = Vec::new();
    for row in start.row..=end_row {
        let line = lines.get(row)?;
        let start_col = if row == start.row {
            start.col.min(line.chars().count())
        } else {
            0
        };
        let end_col = if row == end_row {
            end.col.min(line.chars().count())
        } else {
            line.chars().count()
        };

        if start_col > end_col {
            selected_lines.push(String::new());
            continue;
        }

        let text = line
            .chars()
            .skip(start_col)
            .take(end_col - start_col)
            .collect::<String>()
            .trim_end_matches(' ')
            .to_string();
        selected_lines.push(text);
    }

    let text = selected_lines.join("\n");
    (!text.is_empty()).then_some(text)
}

fn group_cells_by_style(cells: Vec<StyledCell>) -> Vec<StyledSpan> {
    let mut spans = Vec::new();
    if cells.is_empty() {
        return spans;
    }

    let mut current_span: Option<StyledSpan> = None;
    let mut col = 0usize;

    for cell in cells {
        if let Some(span) = &mut current_span {
            if span.fg == cell.fg
                && span.bg == cell.bg
                && span.bold == cell.bold
                && span.italic == cell._italic
                && span.underline == cell._underline
            {
                span.text.push(cell.c);
                span.cell_count += 1;
                col += 1;
                continue;
            } else {
                if let Some(span) = current_span.take() {
                    spans.push(span);
                }
            }
        }

        current_span = Some(StyledSpan {
            start_col: col,
            cell_count: 1,
            text: cell.c.to_string(),
            fg: cell.fg,
            bg: cell.bg,
            bold: cell.bold,
            italic: cell._italic,
            underline: cell._underline,
        });
        col += 1;
    }

    if let Some(span) = current_span {
        spans.push(span);
    }

    spans
}

fn terminal_text_style(_window: &mut Window, cx: &App) -> TextStyle {
    let font_size = cx.theme().mono_font_size;
    TextStyle {
        font_family: cx.theme().mono_font_family.clone(),
        font_size: font_size.into(),
        line_height: font_size.into(),
        white_space: WhiteSpace::Normal,
        background_color: Some(cx.theme().background),
        color: cx.theme().foreground,
        ..Default::default()
    }
}

fn terminal_cell_width(text_style: &TextStyle, window: &mut Window, cx: &App) -> gpui::Pixels {
    let font_pixels = text_style.font_size.to_pixels(window.rem_size());
    let font_id = cx.text_system().resolve_font(&text_style.font());
    cx.text_system()
        .advance(font_id, font_pixels, 'w')
        .map(|advance| advance.width)
        .unwrap_or(px(CELL_WIDTH))
}

#[cfg(test)]
mod tests {
    use super::{
        TerminalPoint, TerminalSelection, selected_cell_ranges, selected_text_from_lines,
        terminal_default_title_left, terminal_tab_title, terminal_title_left,
    };
    use std::path::Path;

    #[test]
    fn normalizes_reversed_selection() {
        let selection = TerminalSelection {
            anchor: TerminalPoint { row: 2, col: 4 },
            cursor: TerminalPoint { row: 1, col: 3 },
        };

        assert_eq!(
            selection.normalized(),
            Some((
                TerminalPoint { row: 1, col: 3 },
                TerminalPoint { row: 2, col: 4 },
            ))
        );
    }

    #[test]
    fn selected_text_spans_lines_and_trims_terminal_padding() {
        let lines = vec![
            "prompt select *    ".to_string(),
            "from users         ".to_string(),
            "where id = 1       ".to_string(),
        ];

        assert_eq!(
            selected_text_from_lines(
                &lines,
                (
                    TerminalPoint { row: 0, col: 7 },
                    TerminalPoint { row: 2, col: 12 },
                ),
            ),
            Some("select *\nfrom users\nwhere id = 1".to_string())
        );
    }

    #[test]
    fn selected_cell_ranges_skip_empty_edges() {
        assert_eq!(
            selected_cell_ranges(
                3,
                10,
                TerminalPoint { row: 0, col: 10 },
                TerminalPoint { row: 2, col: 3 },
            ),
            vec![(1, 0, 10), (2, 0, 3)]
        );
    }

    #[test]
    fn terminal_default_title_uses_working_directory_leaf() {
        assert_eq!(
            terminal_default_title_left(Some(Path::new("/Users/dev/repos/sqlab"))),
            "sqlab"
        );
    }

    #[test]
    fn terminal_tab_title_adds_shell_suffix() {
        assert_eq!(terminal_tab_title("sqlab", "zsh"), "sqlab — zsh");
    }

    #[test]
    fn terminal_title_left_avoids_duplicate_shell_suffix() {
        assert_eq!(terminal_title_left("opencode — zsh"), Some("opencode"));
    }

    #[test]
    fn terminal_title_left_collapses_prompt_path_to_leaf() {
        assert_eq!(
            terminal_title_left("fernando.goncalves@Fernandos-MBP:~/repos/devligeiro/sqlab"),
            Some("sqlab")
        );
    }
}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
