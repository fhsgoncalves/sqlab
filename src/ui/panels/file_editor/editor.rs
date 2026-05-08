use std::path::PathBuf;

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, Styled, Window, actions,
};
use gpui_component::{
    ActiveTheme,
    dock::{Panel, PanelEvent, PanelState},
    input::{Input, InputState},
    v_flex,
};

use crate::data_source::manager::DataSourceManager;
use super::sql_completion::SqlCompletionProvider;

actions!(editor, [ExecuteQuery, SaveFile]);

pub struct EditorPanel {
    path: PathBuf,
    editor: Entity<InputState>,
    focus_handle: FocusHandle,
}

impl EventEmitter<PanelEvent> for EditorPanel {}

impl EditorPanel {
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn new(
        path: PathBuf,
        data_source_manager: Entity<DataSourceManager>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let language = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("txt")
            .to_string();
        let is_sql_file = matches!(language.as_str(), "sql" | "psql");

        let editor = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .code_editor(language)
                .line_number(true)
                .indent_guides(true)
                .default_value(content)
                .placeholder("");
            if is_sql_file {
                state.lsp.completion_provider =
                    Some(SqlCompletionProvider::new(data_source_manager.clone()));
            }
            state
        });

        Self {
            path,
            editor,
            focus_handle: cx.focus_handle(),
        }
    }
    pub fn query_context(&self, cx: &App) -> (String, usize, String) {
        let state = self.editor.read(cx);
        let text = state.value().to_string();
        let cursor = state.cursor();
        let selected = state.selected_value().to_string();
        (text, cursor, selected)
    }

    fn on_save_file(&mut self, _: &SaveFile, _window: &mut Window, cx: &mut Context<Self>) {
        self.save(cx);
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let content = self.editor.read(cx).value().to_string();
        if let Err(e) = std::fs::write(&self.path, content) {
            println!("Failed to save file: {}", e);
        }
    }
}

impl Panel for EditorPanel {
    fn panel_name(&self) -> &'static str {
        "EditorPanel"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Untitled")
            .to_string()
    }

    fn closable(&self, _cx: &App) -> bool {
        true
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState::new(self)
    }
}

impl Render for EditorPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("editor-panel")
            .size_full()
            .on_action(cx.listener(Self::on_save_file))
            .child(
                Input::new(&self.editor)
                    .bordered(false)
                    .p_0()
                    .h_full()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(cx.theme().mono_font_size),
            )
    }
}

impl Focusable for EditorPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
