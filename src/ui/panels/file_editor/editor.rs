use std::path::PathBuf;

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, Styled, Subscription, Window, actions, hsla, px,
};
use gpui_component::{
    ActiveTheme,
    dock::{Panel, PanelEvent, PanelState},
    input::{Input, InputDecoration, InputState},
    v_flex,
};

use super::query_detector::{QueryRange, query_ranges_for_execution};
use super::sql_completion::SqlCompletionProvider;
use crate::data_source::manager::DataSourceManager;

actions!(editor, [ExecuteQuery, SaveFile]);

pub struct EditorPanel {
    path: PathBuf,
    editor: Entity<InputState>,
    focus_handle: FocusHandle,
    last_saved_content: String,
    active_query: Option<QueryRange>,
    query_decoration_override: Option<QueryRange>,
    query_decoration_override_snapshot: Option<EditorSnapshot>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PanelEvent> for EditorPanel {}

#[derive(Clone, Debug, PartialEq)]
struct EditorSnapshot {
    text: String,
    cursor: usize,
    selected: String,
}

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
                .default_value(content.clone())
                .placeholder("");
            if is_sql_file {
                state.lsp.completion_provider =
                    Some(SqlCompletionProvider::new(data_source_manager.clone()));
            }
            state
        });

        let editor_focus_handle = editor.read(cx).focus_handle(cx);
        let mut panel = Self {
            path,
            editor: editor.clone(),
            focus_handle: cx.focus_handle(),
            last_saved_content: content,
            active_query: None,
            query_decoration_override: None,
            query_decoration_override_snapshot: None,
            _subscriptions: vec![
                cx.observe(&editor, |this, _, cx| {
                    this.refresh_active_query(cx);
                }),
                cx.on_focus_out(&editor_focus_handle, window, |this, _, _, cx| {
                    this.save(cx);
                }),
            ],
        };
        panel.refresh_active_query(cx);
        panel
    }
    pub fn query_context(&self, cx: &App) -> (String, Vec<QueryRange>) {
        let state = self.editor.read(cx);
        let text = state.value().to_string();
        let cursor = state.cursor();
        let selected = state.selected_value().to_string();

        if !selected.trim().is_empty() {
            return (selected, Vec::new());
        }

        let queries = query_ranges_for_execution(&text, cursor);
        (String::new(), queries)
    }

    fn refresh_active_query(&mut self, cx: &mut Context<Self>) {
        let (active_query, snapshot) = {
            let state = self.editor.read(cx);
            let text = state.value().to_string();
            let selected = state.selected_value().to_string();
            let snapshot = EditorSnapshot {
                text: text.clone(),
                cursor: state.cursor(),
                selected: selected.clone(),
            };

            if self.query_decoration_override_snapshot.as_ref() == Some(&snapshot) {
                (self.query_decoration_override.clone(), snapshot)
            } else if selected.trim().is_empty() {
                (
                    query_ranges_for_execution(&text, state.cursor())
                        .into_iter()
                        .next(),
                    snapshot,
                )
            } else {
                (None, snapshot)
            }
        };

        if self.query_decoration_override_snapshot.as_ref() != Some(&snapshot) {
            self.query_decoration_override = None;
            self.query_decoration_override_snapshot = None;
        }

        self.apply_query_decoration(active_query, cx);
    }

    pub fn override_query_decoration(
        &mut self,
        active_query: Option<QueryRange>,
        cx: &mut Context<Self>,
    ) {
        self.query_decoration_override = active_query.clone();
        self.query_decoration_override_snapshot = Some(self.editor_snapshot(cx));
        self.apply_query_decoration(active_query, cx);
    }

    fn editor_snapshot(&self, cx: &App) -> EditorSnapshot {
        let state = self.editor.read(cx);
        EditorSnapshot {
            text: state.value().to_string(),
            cursor: state.cursor(),
            selected: state.selected_value().to_string(),
        }
    }

    fn apply_query_decoration(&mut self, active_query: Option<QueryRange>, cx: &mut Context<Self>) {
        let decorations = active_query
            .as_ref()
            .map(|query| InputDecoration {
                range: query.trimmed_range.clone(),
                fill: None,
                border: Some(hsla(0.76, 0.73, 0.72, 0.85)),
                border_width: px(1.),
            })
            .into_iter()
            .collect();

        self.editor.update(cx, |state, cx| {
            state.set_decorations(decorations, cx);
        });

        if self.active_query != active_query {
            self.active_query = active_query;
            cx.notify();
        }
    }

    fn on_save_file(&mut self, _: &SaveFile, _window: &mut Window, cx: &mut Context<Self>) {
        self.save(cx);
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let content = self.editor.read(cx).value().to_string();
        if content == self.last_saved_content {
            return;
        }

        if let Err(e) = std::fs::write(&self.path, &content) {
            println!("Failed to save file: {}", e);
        } else {
            self.last_saved_content = content;
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
