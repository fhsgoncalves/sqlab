use std::{ops::Range, path::PathBuf, sync::Arc, time::Duration};

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyBinding, ParentElement, Render, Styled, Subscription, Task, Window, actions,
    div, hsla, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, IconName, Selectable, Sizable,
    button::{Button, ButtonVariants},
    dock::{Panel, PanelEvent, PanelState},
    h_flex,
    input::{Input, InputDecoration, InputEvent, InputState},
    v_flex,
};

use super::query_detector::{QueryRange, query_ranges_for_execution, query_ranges_in_text};
use super::sql_completion::{SqlCompletionProvider, SqlDiagnostic, sql_diagnostics_at};
use crate::schema_cache;
use crate::ui::search::{SearchOptions, TextMatch, find_text_matches};
use sqlab_drivers_core::DatabaseSchema;
use sqlab_drivers_core::manager::DataSourceManager;

actions!(
    editor,
    [
        ExecuteQuery,
        SaveFile,
        FormatQuery,
        ToggleEditorSearch,
        ToggleEditorReplace,
        CloseEditorSearch,
        SelectPreviousEditorMatch,
        SelectNextEditorMatch,
        ReplaceEditorMatch,
        ReplaceAllEditorMatches
    ]
);

const SEARCH_CONTEXT: &str = "EditorSearch";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", CloseEditorSearch, Some(SEARCH_CONTEXT)),
        KeyBinding::new("enter", SelectNextEditorMatch, Some(SEARCH_CONTEXT)),
        KeyBinding::new(
            "shift-enter",
            SelectPreviousEditorMatch,
            Some(SEARCH_CONTEXT),
        ),
    ]);
}

pub struct EditorPanel {
    path: PathBuf,
    editor: Entity<InputState>,
    search_input: Entity<InputState>,
    replace_input: Entity<InputState>,
    data_source_manager: Entity<DataSourceManager>,
    focus_handle: FocusHandle,
    last_saved_content: String,
    active_query: Option<QueryRange>,
    query_decoration_override: Option<QueryRange>,
    query_decoration_override_snapshot: Option<EditorSnapshot>,
    search_open: bool,
    search_replace_mode: bool,
    search_case_sensitive: bool,
    search_regex: bool,
    search_whole_word: bool,
    search_fuzzy: bool,
    search_matches: Vec<TextMatch>,
    selected_match_ix: usize,
    _subscriptions: Vec<Subscription>,
    schema_cache: Option<(String, Arc<DatabaseSchema>)>,
    last_diagnostics: Vec<SqlDiagnostic>,
    diagnostics_epoch: u64,
    pending_diagnostics_task: Option<Task<()>>,
    last_observed_snapshot: Option<EditorSnapshot>,
}

impl EventEmitter<PanelEvent> for EditorPanel {}

#[derive(Clone, Debug, PartialEq)]
struct EditorSnapshot {
    text: String,
    cursor: usize,
    selected: String,
}

fn global_match_range(text: &str, text_match: &TextMatch) -> Option<Range<usize>> {
    let line_start = text
        .lines()
        .take(text_match.line_number.saturating_sub(1))
        .map(|line| line.len() + 1)
        .sum::<usize>();
    let start = line_start + text_match.match_start;
    let end = line_start + text_match.match_end;
    (end <= text.len()).then_some(start..end)
}

fn replace_all_text(text: &str, query: &str, replacement: &str, options: SearchOptions) -> String {
    if options.use_regex {
        return regex::RegexBuilder::new(query)
            .case_insensitive(!options.case_sensitive)
            .build()
            .map(|regex| regex.replace_all(text, replacement).to_string())
            .unwrap_or_else(|_| text.to_string());
    }

    if options.use_fuzzy {
        let mut output = text.to_string();
        let mut ranges = find_text_matches(text, query, options)
            .into_iter()
            .filter_map(|text_match| global_match_range(text, &text_match))
            .collect::<Vec<_>>();
        ranges.sort_by(|left, right| right.start.cmp(&left.start));
        for range in ranges {
            output.replace_range(range, replacement);
        }
        return output;
    }

    if options.case_sensitive {
        text.replace(query, replacement)
    } else {
        replace_case_insensitive(text, query, replacement)
    }
}

fn replace_case_insensitive(text: &str, query: &str, replacement: &str) -> String {
    let lower_text = text.to_lowercase();
    let lower_query = query.to_lowercase();
    let mut result = String::new();
    let mut last_end = 0;
    let mut start = 0;

    while let Some(pos) = lower_text[start..].find(&lower_query) {
        let match_start = start + pos;
        let match_end = match_start + query.len();
        result.push_str(&text[last_end..match_start]);
        result.push_str(replacement);
        last_end = match_end;
        start = match_start + 1;
    }

    result.push_str(&text[last_end..]);
    result
}

impl EditorPanel {
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn editor_focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.read(cx).focus_handle(cx)
    }

    pub fn go_to_line(&mut self, line_number: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.go_to_position(line_number, 0, window, cx);
    }

    pub fn go_to_position(
        &mut self,
        line_number: usize,
        column: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row = line_number.saturating_sub(1) as u32;
        let character = column as u32;
        self.editor.update(cx, |editor, cx| {
            editor.set_cursor_position(lsp_types::Position::new(row, character), window, cx);
        });
    }

    pub fn toggle_search_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_open {
            self.open_search(window, cx);
        }
        self.search_replace_mode = true;
        window.focus(&self.replace_input.read(cx).focus_handle(cx), cx);
        cx.notify();
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
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Find"));
        let replace_input = cx.new(|cx| InputState::new(window, cx).placeholder("Replace"));

        let editor_focus_handle = editor.read(cx).focus_handle(cx);
        let data_source_manager_for_obs = data_source_manager.clone();
        let mut panel = Self {
            path,
            editor: editor.clone(),
            search_input: search_input.clone(),
            replace_input: replace_input.clone(),
            data_source_manager,
            focus_handle: cx.focus_handle(),
            last_saved_content: content,
            active_query: None,
            query_decoration_override: None,
            query_decoration_override_snapshot: None,
            search_open: false,
            search_replace_mode: false,
            search_case_sensitive: false,
            search_regex: false,
            search_whole_word: false,
            search_fuzzy: false,
            search_matches: Vec::new(),
            selected_match_ix: 0,
            _subscriptions: vec![
                cx.subscribe(&editor, |this, _, event: &InputEvent, cx| match event {
                    InputEvent::Change => {
                        this.refresh_active_query(cx);
                        this.update_search_matches(cx);
                        this.schedule_diagnostics(cx);
                    }
                    InputEvent::PressEnter { .. } => {
                        this.refresh_active_query(cx);
                        this.schedule_diagnostics(cx);
                    }
                    InputEvent::Focus | InputEvent::Blur => {}
                }),
                cx.observe(&editor, |this, _, cx| {
                    let snapshot = {
                        let state = this.editor.read(cx);
                        EditorSnapshot {
                            text: state.value().to_string(),
                            cursor: state.cursor(),
                            selected: state.selected_value().to_string(),
                        }
                    };
                    if this.last_observed_snapshot.as_ref() == Some(&snapshot) {
                        return;
                    }
                    this.last_observed_snapshot = Some(snapshot);
                    this.refresh_active_query(cx);
                    this.schedule_diagnostics(cx);
                }),
                cx.observe(&data_source_manager_for_obs, |this, _, cx| {
                    this.refresh_schema_cache(cx);
                    this.schedule_diagnostics(cx);
                }),
                cx.on_focus_out(&editor_focus_handle, window, |this, _, _, cx| {
                    this.save(cx);
                }),
                cx.subscribe_in(&search_input, window, {
                    move |this: &mut EditorPanel, _input, event: &InputEvent, window, cx| {
                        match event {
                            InputEvent::Change => {
                                this.update_search_matches(cx);
                            }
                            InputEvent::PressEnter { .. } => {
                                this.select_next_match(window, cx);
                            }
                            _ => {}
                        }
                    }
                }),
                cx.subscribe_in(&replace_input, window, {
                    move |this: &mut EditorPanel, _input, event: &InputEvent, window, cx| {
                        if matches!(event, InputEvent::PressEnter { .. }) {
                            this.replace_current_match(window, cx);
                        }
                    }
                }),
            ],
            schema_cache: None,
            last_diagnostics: Vec::new(),
            diagnostics_epoch: 0,
            pending_diagnostics_task: None,
            last_observed_snapshot: None,
        };
        panel.refresh_active_query(cx);
        panel.refresh_schema_cache(cx);
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

    fn refresh_schema_cache(&mut self, cx: &mut Context<Self>) {
        let schema_cache = self
            .data_source_manager
            .read(cx)
            .active_config()
            .and_then(|config| {
                let key = schema_cache::cache_key(config);
                if let Some((cached_key, cached_schema)) = &self.schema_cache {
                    if cached_key == &key {
                        return Some((key, cached_schema.clone()));
                    }
                }
                schema_cache::load(&key)
                    .ok()
                    .flatten()
                    .map(|schema| (key, Arc::new(schema)))
            });

        if let Some((key, schema)) = schema_cache {
            self.schema_cache = Some((key, schema));
        }
    }

    fn schedule_diagnostics(&mut self, cx: &mut Context<Self>) {
        self.diagnostics_epoch += 1;
        let epoch = self.diagnostics_epoch;
        let editor = self.editor.clone();
        let schema = self.schema_cache.clone().map(|(_, s)| s);

        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;

            let (text, cursor) =
                editor.read_with(cx, |state, _| (state.value().to_string(), state.cursor()));

            let diagnostics = schema
                .as_ref()
                .map(|schema| sql_diagnostics_at(&text, schema, Some(cursor)))
                .unwrap_or_default();

            this.update(cx, |this, cx| {
                if this.diagnostics_epoch == epoch {
                    this.last_diagnostics = diagnostics;
                    this.apply_current_decorations(cx);
                }
            })
            .ok();
        });

        self.pending_diagnostics_task = Some(task);
    }

    fn apply_current_decorations(&mut self, cx: &mut Context<Self>) {
        let active_query = self.active_query.clone();
        let mut decorations = active_query
            .as_ref()
            .map(|query| InputDecoration {
                range: query.trimmed_range.clone(),
                fill: None,
                border: Some(hsla(0.76, 0.73, 0.72, 0.85)),
                border_width: px(1.),
                underline: None,
                underline_wavy: false,
            })
            .into_iter()
            .collect::<Vec<_>>();

        decorations.extend(
            self.last_diagnostics
                .iter()
                .map(|diagnostic| InputDecoration {
                    range: diagnostic.range.clone(),
                    fill: None,
                    border: None,
                    border_width: px(1.),
                    underline: Some(hsla(0.0, 0.76, 0.62, 0.95)),
                    underline_wavy: true,
                }),
        );

        self.editor.update(cx, |state, cx| {
            state.set_decorations(decorations, cx);
        });
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
        let mut decorations = active_query
            .as_ref()
            .map(|query| InputDecoration {
                range: query.trimmed_range.clone(),
                fill: None,
                border: Some(hsla(0.76, 0.73, 0.72, 0.85)),
                border_width: px(1.),
                underline: None,
                underline_wavy: false,
            })
            .into_iter()
            .collect::<Vec<_>>();

        decorations.extend(
            self.last_diagnostics
                .iter()
                .map(|diagnostic| InputDecoration {
                    range: diagnostic.range.clone(),
                    fill: None,
                    border: None,
                    border_width: px(1.),
                    underline: Some(hsla(0.0, 0.76, 0.62, 0.95)),
                    underline_wavy: true,
                }),
        );

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

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_open = true;
        self.search_replace_mode = false;

        let selected = self.editor.read(cx).selected_value().to_string();
        if !selected.trim().is_empty() && !selected.contains(['\n', '\r']) {
            self.search_input.update(cx, |input, cx| {
                input.set_value(selected, window, cx);
            });
        } else {
            self.update_search_matches(cx);
        }

        window.focus(&self.search_input.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    fn toggle_search(
        &mut self,
        _: &ToggleEditorSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search_open {
            window.focus(&self.search_input.read(cx).focus_handle(cx), cx);
        } else {
            self.open_search(window, cx);
        }
    }

    fn toggle_replace(
        &mut self,
        _: &ToggleEditorReplace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.search_open {
            self.open_search(window, cx);
        }
        self.search_replace_mode = !self.search_replace_mode;
        if self.search_replace_mode {
            window.focus(&self.replace_input.read(cx).focus_handle(cx), cx);
        } else {
            window.focus(&self.search_input.read(cx).focus_handle(cx), cx);
        }
        cx.notify();
    }

    fn close_search(&mut self, _: &CloseEditorSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.search_open = false;
        self.search_replace_mode = false;
        self.search_matches.clear();
        self.selected_match_ix = 0;
        window.focus(&self.editor.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    fn update_search_matches(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value().to_string();
        let text = self.editor.read(cx).value().to_string();
        self.search_matches = find_text_matches(&text, &query, self.search_options());
        if self.selected_match_ix >= self.search_matches.len() {
            self.selected_match_ix = self.search_matches.len().saturating_sub(1);
        }
        cx.notify();
    }

    fn search_options(&self) -> SearchOptions {
        SearchOptions {
            case_sensitive: self.search_case_sensitive,
            use_regex: self.search_regex,
            whole_word: self.search_whole_word,
            use_fuzzy: self.search_fuzzy && !self.search_regex,
        }
    }

    fn selected_match(&self) -> Option<&TextMatch> {
        self.search_matches.get(self.selected_match_ix)
    }

    fn select_previous_match(
        &mut self,
        _: &SelectPreviousEditorMatch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search_matches.is_empty() {
            return;
        }
        self.selected_match_ix = if self.selected_match_ix == 0 {
            self.search_matches.len() - 1
        } else {
            self.selected_match_ix - 1
        };
        self.scroll_to_selected_match(window, cx);
    }

    fn select_next_match(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_matches.is_empty() {
            return;
        }
        self.selected_match_ix = (self.selected_match_ix + 1) % self.search_matches.len();
        self.scroll_to_selected_match(window, cx);
    }

    fn on_select_next_match(
        &mut self,
        _: &SelectNextEditorMatch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_next_match(window, cx);
    }

    fn scroll_to_selected_match(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(line_number) = self.selected_match().map(|m| m.line_number) else {
            return;
        };
        self.go_to_line(line_number, window, cx);
        window.focus(&self.search_input.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    fn replace_current_match_action(
        &mut self,
        _: &ReplaceEditorMatch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_current_match(window, cx);
    }

    fn replace_current_match(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).value().to_string();
        let Some(range) = self
            .selected_match()
            .and_then(|m| global_match_range(&text, m))
        else {
            return;
        };
        let replacement = self.replace_input.read(cx).value().to_string();
        let mut text = text;
        text.replace_range(range, &replacement);
        self.editor.update(cx, |editor, cx| {
            editor.set_value(text, window, cx);
        });
        self.update_search_matches(cx);
        self.scroll_to_selected_match(window, cx);
    }

    fn replace_all_matches(
        &mut self,
        _: &ReplaceAllEditorMatches,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = self.search_input.read(cx).value().to_string();
        if query.is_empty() {
            return;
        }

        let replacement = self.replace_input.read(cx).value().to_string();
        let text = self.editor.read(cx).value().to_string();
        let new_text = replace_all_text(&text, &query, &replacement, self.search_options());
        self.editor.update(cx, |editor, cx| {
            editor.set_value(new_text, window, cx);
        });
        self.update_search_matches(cx);
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

    fn on_format_query(&mut self, _: &FormatQuery, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.editor.read(cx);
        let text = state.value().to_string();
        let selected = state.selected_value().to_string();
        let cursor = state.cursor();

        let (formatted, range, new_cursor) = if !selected.trim().is_empty() {
            let formatted = sqlformat::format(
                &selected,
                &sqlformat::QueryParams::default(),
                &sqlformat::FormatOptions::default(),
            );
            let selection_range = state.selected_range();
            let new_cursor = selection_range.start + formatted.len();
            (formatted, selection_range, new_cursor)
        } else if let Some(query) = query_ranges_in_text(&text)
            .into_iter()
            .find(|q| cursor >= q.trimmed_range.start && cursor <= q.trimmed_range.end)
        {
            let formatted = sqlformat::format(
                &query.text,
                &sqlformat::QueryParams::default(),
                &sqlformat::FormatOptions::default(),
            );
            let new_cursor = query.trimmed_range.start + formatted.len();
            (formatted, query.trimmed_range, new_cursor)
        } else if let Some(query) = query_ranges_for_execution(&text, cursor).into_iter().next() {
            let formatted = sqlformat::format(
                &query.text,
                &sqlformat::QueryParams::default(),
                &sqlformat::FormatOptions::default(),
            );
            let new_cursor = query.trimmed_range.start + formatted.len();
            (formatted, query.trimmed_range, new_cursor)
        } else {
            return;
        };

        let mut new_text = text;
        new_text.replace_range(range.clone(), &formatted);

        // Calculate cursor position after formatting
        let end_line = new_text[..new_cursor].matches('\n').count() as u32;
        let end_col = new_text[..new_cursor]
            .rfind('\n')
            .map(|ix| new_cursor - ix - 1)
            .unwrap_or(new_cursor) as u32;

        // Set the selection to the range we want to replace, then use replace()
        // which records the change in undo history
        self.editor.update(cx, |editor, cx| {
            editor.set_selected_range(range.clone(), cx);
            editor.replace(formatted.clone(), window, cx);
            editor.set_cursor_position(lsp_types::Position::new(end_line, end_col), window, cx);
        });
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
            .on_action(cx.listener(Self::on_format_query))
            .on_action(cx.listener(Self::toggle_search))
            .on_action(cx.listener(Self::toggle_replace))
            .on_action(cx.listener(Self::close_search))
            .on_action(cx.listener(Self::select_previous_match))
            .on_action(cx.listener(Self::on_select_next_match))
            .on_action(cx.listener(Self::replace_current_match_action))
            .on_action(cx.listener(Self::replace_all_matches))
            .when(self.search_open, |this| {
                this.child(self.render_search_bar(cx))
            })
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

impl EditorPanel {
    fn render_search_bar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let match_label = if self.search_matches.is_empty() {
            "0/0".to_string()
        } else {
            format!(
                "{}/{}",
                self.selected_match_ix + 1,
                self.search_matches.len()
            )
        };

        v_flex()
            .id("editor-search")
            .key_context(SEARCH_CONTEXT)
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .px_3()
            .py_2()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        div().flex_1().child(
                            Input::new(&self.search_input)
                                .small()
                                .focus_bordered(false)
                                .suffix(self.render_search_toggles(cx)),
                        ),
                    )
                    .child(
                        Button::new("editor-replace-toggle")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Replace)
                            .selected(self.search_replace_mode)
                            .tooltip("Toggle Replace")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_replace(&ToggleEditorReplace, window, cx);
                            })),
                    )
                    .child(
                        Button::new("editor-prev-match")
                            .xsmall()
                            .ghost()
                            .icon(IconName::ChevronLeft)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.select_previous_match(&SelectPreviousEditorMatch, window, cx);
                            })),
                    )
                    .child(
                        Button::new("editor-next-match")
                            .xsmall()
                            .ghost()
                            .icon(IconName::ChevronRight)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.select_next_match(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .min_w(px(44.))
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(match_label),
                    )
                    .child(
                        Button::new("editor-search-close")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_search(&CloseEditorSearch, window, cx);
                            })),
                    ),
            )
            .when(self.search_replace_mode, |this| {
                this.child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .child(
                            div().flex_1().child(
                                Input::new(&self.replace_input)
                                    .small()
                                    .focus_bordered(false),
                            ),
                        )
                        .child(
                            Button::new("editor-replace-one")
                                .small()
                                .label("Replace")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.replace_current_match(window, cx);
                                })),
                        )
                        .child(
                            Button::new("editor-replace-all")
                                .small()
                                .label("All")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.replace_all_matches(&ReplaceAllEditorMatches, window, cx);
                                })),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_search_toggles(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_1()
            .child(
                Button::new("editor-search-case")
                    .selected(self.search_case_sensitive)
                    .xsmall()
                    .compact()
                    .ghost()
                    .icon(IconName::CaseSensitive)
                    .tooltip("Match Case")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.search_case_sensitive = !this.search_case_sensitive;
                        this.update_search_matches(cx);
                    })),
            )
            .child(
                Button::new("editor-search-word")
                    .selected(self.search_whole_word)
                    .xsmall()
                    .compact()
                    .ghost()
                    .label("W")
                    .tooltip("Whole Word")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.search_whole_word = !this.search_whole_word;
                        this.update_search_matches(cx);
                    })),
            )
            .child(
                Button::new("editor-search-regex")
                    .selected(self.search_regex)
                    .xsmall()
                    .compact()
                    .ghost()
                    .label(".*")
                    .tooltip("Use Regex")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.search_regex = !this.search_regex;
                        if this.search_regex {
                            this.search_fuzzy = false;
                        }
                        this.update_search_matches(cx);
                    })),
            )
            .child(
                Button::new("editor-search-fuzzy")
                    .selected(self.search_fuzzy)
                    .xsmall()
                    .compact()
                    .ghost()
                    .label("fz")
                    .tooltip("Fuzzy Search")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.search_fuzzy = !this.search_fuzzy;
                        if this.search_fuzzy {
                            this.search_regex = false;
                        }
                        this.update_search_matches(cx);
                    })),
            )
    }
}

impl Focusable for EditorPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
