use std::ops::Range;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, MouseDownEvent, MouseMoveEvent, ParentElement, Render, Styled, Window, div,
};
use gpui_component::{
    ActiveTheme,
    dock::{Panel, PanelEvent, PanelState},
    input::{Input, InputState},
    v_flex,
};
use sqlab_drivers_core::{DatabaseSchema, ddl::create_ddl_generator};

use super::editor::GoToDefinition;
use super::sql_completion::{TableDefinitionTarget, table_definition_target_at};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DdlPanelEvent {
    OpenTableDefinition {
        connection_name: String,
        schema_name: String,
        table_name: String,
        ddl: String,
    },
}

pub struct DdlPanel {
    connection_name: String,
    schema_name: String,
    table_name: String,
    title: String,
    schema: Arc<DatabaseSchema>,
    editor: Entity<InputState>,
    focus_handle: FocusHandle,
    hover_table_definition_range: Option<Range<usize>>,
}

impl DdlPanel {
    pub fn new(
        connection_name: String,
        schema_name: String,
        table_name: String,
        ddl: String,
        schema: Arc<DatabaseSchema>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let ddl_text = ddl.clone();
        let editor = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("sql")
                .line_number(true)
                .indent_guides(true)
                .default_value(ddl_text)
                .placeholder("")
        });
        let title = format!("{}.{} DDL", schema_name, table_name);
        Self {
            connection_name,
            schema_name,
            table_name,
            title,
            schema,
            editor,
            focus_handle: cx.focus_handle(),
            hover_table_definition_range: None,
        }
    }

    pub fn title_text(&self) -> &str {
        &self.title
    }

    pub fn connection_name(&self) -> &str {
        &self.connection_name
    }

    pub fn schema_name(&self) -> &str {
        &self.schema_name
    }

    pub fn table_name(&self) -> &str {
        &self.table_name
    }

    pub fn schema(&self) -> &Arc<DatabaseSchema> {
        &self.schema
    }

    pub fn matches_table(
        &self,
        connection_name: &str,
        schema_name: &str,
        table_name: &str,
    ) -> bool {
        self.connection_name == connection_name
            && self.schema_name.eq_ignore_ascii_case(schema_name)
            && self.table_name.eq_ignore_ascii_case(table_name)
    }

    fn table_definition_target_at_offset(
        &self,
        offset: usize,
        cx: &App,
    ) -> Option<TableDefinitionTarget> {
        let text = self.editor.read(cx).value().to_string();
        table_definition_target_at(&text, offset, &self.schema, None, None)
    }

    fn open_table_definition_at_cursor(&mut self, cx: &mut Context<Self>) {
        let offset = self.editor.read(cx).cursor();
        let target = match self.table_definition_target_at_offset(offset, cx) {
            Some(t) => t,
            None => return,
        };
        let table = self.schema.tables.iter().find(|t| {
            t.schema.eq_ignore_ascii_case(&target.schema_name)
                && t.name.eq_ignore_ascii_case(&target.table_name)
        });
        let Some(table) = table else {
            return;
        };
        let generator = create_ddl_generator(self.schema.db_type);
        let ddl = generator.generate_table_ddl(&self.schema, table);
        cx.emit(DdlPanelEvent::OpenTableDefinition {
            connection_name: self.connection_name.clone(),
            schema_name: target.schema_name.clone(),
            table_name: target.table_name.clone(),
            ddl,
        });
    }

    fn open_table_definition_at_mouse_position(
        &mut self,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let offset = self
            .editor
            .read(cx)
            .offset_for_mouse_position(event.position);
        let target = match self.table_definition_target_at_offset(offset, cx) {
            Some(t) => t,
            None => return,
        };
        let table = self.schema.tables.iter().find(|t| {
            t.schema.eq_ignore_ascii_case(&target.schema_name)
                && t.name.eq_ignore_ascii_case(&target.table_name)
        });
        let Some(table) = table else {
            return;
        };
        let generator = create_ddl_generator(self.schema.db_type);
        let ddl = generator.generate_table_ddl(&self.schema, table);
        cx.emit(DdlPanelEvent::OpenTableDefinition {
            connection_name: self.connection_name.clone(),
            schema_name: target.schema_name.clone(),
            table_name: target.table_name.clone(),
            ddl,
        });
    }

    fn on_go_to_definition(
        &mut self,
        _: &GoToDefinition,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_table_definition_at_cursor(cx);
    }

    fn update_hover_table_definition(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let next_range =
            if event.modifiers.secondary() && !event.modifiers.shift && !event.modifiers.alt {
                let offset = self
                    .editor
                    .read(cx)
                    .offset_for_mouse_position(event.position);
                self.table_definition_target_at_offset(offset, cx)
                    .map(|target| target.token_range)
            } else {
                None
            };

        if self.hover_table_definition_range == next_range {
            return;
        }

        self.hover_table_definition_range = next_range;
        cx.notify();
    }
}

impl EventEmitter<DdlPanelEvent> for DdlPanel {}
impl EventEmitter<PanelEvent> for DdlPanel {}

impl Panel for DdlPanel {
    fn panel_name(&self) -> &'static str {
        "DdlPanel"
    }

    fn title(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.title.clone()
    }

    fn closable(&self, _cx: &App) -> bool {
        true
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState::new(self)
    }
}

impl Render for DdlPanel {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hover_range = self.hover_table_definition_range.clone();
        v_flex()
            .id("ddl-panel")
            .key_context("ddl_panel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().background)
            .on_action(cx.listener(Self::on_go_to_definition))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    if event.modifiers.secondary() && !event.modifiers.shift && !event.modifiers.alt
                    {
                        this.open_table_definition_at_mouse_position(event, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.update_hover_table_definition(event, cx);
            }))
            .when(hover_range.is_some(), |this| this.cursor_pointer())
            .child(
                div().key_context("ddl_editor").size_full().child(
                    Input::new(&self.editor)
                        .bordered(false)
                        .p_0()
                        .h_full()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(cx.theme().mono_font_size),
                ),
            )
    }
}

impl Focusable for DdlPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
