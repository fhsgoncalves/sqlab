use std::{
    collections::HashSet,
    ops::Range,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyBinding, ParentElement, Render, Styled, Subscription, Task, Window, actions,
    div, hsla, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, IconName, IconNamed, Selectable, Sizable,
    button::{Button, ButtonVariants},
    dock::{Panel, PanelEvent, PanelState},
    h_flex,
    input::{
        Input, InputDecoration, InputEvent, InputGutterAdornment, InputInlineAdornment, InputState,
    },
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
        ToggleCommentLines,
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
const SQL_LINE_COMMENT: &str = "-- ";

#[derive(Clone, Debug)]
pub enum EditorPanelEvent {
    CursorMoved,
}

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
    query_executions: Vec<QueryExecutionMarker>,
    last_query_execution_text: String,
    next_query_execution_id: u64,
    elapsed_timer_task: Option<Task<()>>,
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
    selected_search_path: Option<String>,
    selected_connection_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum QueryExecutionStatus {
    Running { started_at: Instant },
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QueryExecutionMarker {
    id: u64,
    range: QueryRange,
    status: QueryExecutionStatus,
}

impl EventEmitter<PanelEvent> for EditorPanel {}
impl EventEmitter<EditorPanelEvent> for EditorPanel {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EditorCursorPosition {
    pub row: usize,
    pub column: usize,
    pub cursor: usize,
    pub visible_rows: Option<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq)]
struct EditorSnapshot {
    text: String,
    cursor: usize,
    selected: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TextEditDelta {
    old_range: Range<usize>,
    new_len: usize,
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

#[cfg(test)]
fn selected_query_range(text: &str, selected_range: Range<usize>) -> Option<QueryRange> {
    let start = selected_range.start.min(text.len());
    let end = selected_range.end.min(text.len());
    if start >= end {
        return None;
    }

    let selected = text.get(start..end)?;
    let leading = selected.len() - selected.trim_start().len();
    let trailing = selected.len() - selected.trim_end().len();
    let trimmed_start = start + leading;
    let trimmed_end = end - trailing;
    if trimmed_start >= trimmed_end {
        return None;
    }

    Some(QueryRange {
        range: selected_range,
        trimmed_range: trimmed_start..trimmed_end,
        text: text.get(trimmed_start..trimmed_end)?.to_string(),
    })
}

fn common_prefix_len(left: &str, right: &str) -> usize {
    let mut len = 0;
    for (left_ch, right_ch) in left.chars().zip(right.chars()) {
        if left_ch != right_ch {
            break;
        }
        len += left_ch.len_utf8();
    }
    len
}

fn common_suffix_len(left: &str, right: &str, prefix_len: usize) -> usize {
    let left_tail = &left[prefix_len..];
    let right_tail = &right[prefix_len..];
    let mut len = 0;

    for (left_ch, right_ch) in left_tail.chars().rev().zip(right_tail.chars().rev()) {
        if left_ch != right_ch {
            break;
        }
        len += left_ch.len_utf8();
    }

    len
}

fn text_edit_delta(old_text: &str, new_text: &str) -> Option<TextEditDelta> {
    if old_text == new_text {
        return None;
    }

    let prefix_len = common_prefix_len(old_text, new_text);
    let suffix_len = common_suffix_len(old_text, new_text, prefix_len);

    Some(TextEditDelta {
        old_range: prefix_len..old_text.len() - suffix_len,
        new_len: new_text.len() - prefix_len - suffix_len,
    })
}

fn transform_offset(offset: usize, delta: &TextEditDelta) -> usize {
    let edit_start = delta.old_range.start;
    let edit_end = delta.old_range.end;
    let deleted_len = edit_end - edit_start;

    if edit_end <= offset {
        offset + delta.new_len - deleted_len
    } else if offset <= edit_start {
        offset
    } else {
        edit_start + delta.new_len
    }
}

fn transform_range(range: Range<usize>, delta: &TextEditDelta) -> Option<Range<usize>> {
    let start = transform_offset(range.start, delta);
    let end = transform_offset(range.end, delta);

    (start < end).then_some(start..end)
}

fn ranges_overlap(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start < right.end && right.start < left.end
}

fn update_query_range_for_edit(
    query_range: &QueryRange,
    delta: &TextEditDelta,
    new_text: &str,
) -> Option<QueryRange> {
    let range = transform_range(query_range.range.clone(), delta)?;
    let trimmed_range = transform_range(query_range.trimmed_range.clone(), delta)?;
    let text = new_text.get(trimmed_range.clone())?.to_string();

    Some(QueryRange {
        range,
        trimmed_range,
        text,
    })
}

fn line_number_for_offset(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
}

fn query_execution_marker_conflicts(
    text: &str,
    marker: &QueryExecutionMarker,
    range: &QueryRange,
) -> bool {
    let marker_line = line_number_for_offset(text, marker.range.trimmed_range.start);
    let range_line = line_number_for_offset(text, range.trimmed_range.start);

    marker_line == range_line || ranges_overlap(&marker.range.trimmed_range, &range.trimmed_range)
}

fn format_elapsed(duration: Duration) -> String {
    let millis = duration.as_millis();
    if millis < 1_000 {
        return format!("{millis} ms");
    }

    let seconds = duration.as_secs_f64();
    if seconds < 10.0 {
        format!("{seconds:.1} s")
    } else {
        format!("{} s", duration.as_secs())
    }
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

fn selected_line_range(text: &str, selected_range: Range<usize>, cursor: usize) -> Range<usize> {
    let start_offset = selected_range.start.min(text.len());
    let end_offset = if selected_range.is_empty() {
        cursor.min(text.len())
    } else {
        selected_range.end.saturating_sub(1).min(text.len())
    };

    let start = text[..start_offset]
        .rfind('\n')
        .map(|ix| ix + 1)
        .unwrap_or(0);
    let end = text[end_offset..]
        .find('\n')
        .map(|ix| end_offset + ix)
        .unwrap_or(text.len());

    start..end
}

fn map_preserving_line_endings(text: &str, mut map_line: impl FnMut(&str) -> String) -> String {
    if text.is_empty() {
        return map_line("");
    }

    let mut output = String::with_capacity(text.len());
    let mut start = 0;

    for (ix, ch) in text.char_indices() {
        if ch == '\n' {
            let line_end = if ix > start && text.as_bytes().get(ix - 1) == Some(&b'\r') {
                ix - 1
            } else {
                ix
            };
            output.push_str(&map_line(&text[start..line_end]));
            output.push_str(&text[line_end..=ix]);
            start = ix + 1;
        }
    }

    if start < text.len() {
        output.push_str(&map_line(&text[start..]));
    }

    output
}

fn comment_lines(text: &str, line_range: Range<usize>) -> String {
    let mut output = text.to_string();
    let replacement = map_preserving_line_endings(&text[line_range.clone()], |line| {
        format!("{SQL_LINE_COMMENT}{line}")
    });
    output.replace_range(line_range, &replacement);
    output
}

fn uncomment_lines(text: &str, line_range: Range<usize>) -> String {
    let mut output = text.to_string();
    let replacement = map_preserving_line_endings(&text[line_range.clone()], |line| {
        let trim_start = line.trim_start_matches(char::is_whitespace);
        let leading_len = line.len() - trim_start.len();

        if let Some(rest) = trim_start.strip_prefix(SQL_LINE_COMMENT) {
            format!("{}{}", &line[..leading_len], rest)
        } else if let Some(rest) = trim_start.strip_prefix("--") {
            format!("{}{}", &line[..leading_len], rest)
        } else {
            line.to_string()
        }
    });
    output.replace_range(line_range, &replacement);
    output
}

fn toggle_comment_lines(text: &str, line_range: Range<usize>) -> String {
    let lines_text = &text[line_range.clone()];
    let all_commented = lines_text.lines().all(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with(SQL_LINE_COMMENT) || trimmed.starts_with("--")
    });

    if all_commented {
        uncomment_lines(text, line_range)
    } else {
        comment_lines(text, line_range)
    }
}

impl EditorPanel {
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn editor_focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.read(cx).focus_handle(cx)
    }

    pub(crate) fn cursor_position(&self, cx: &App) -> EditorCursorPosition {
        let state = self.editor.read(cx);
        let position = state.cursor_position();
        EditorCursorPosition {
            row: position.line as usize,
            column: position.character as usize,
            cursor: state.cursor(),
            visible_rows: state.visible_row_range(),
        }
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
            last_saved_content: content.clone(),
            active_query: None,
            query_decoration_override: None,
            query_decoration_override_snapshot: None,
            query_executions: Vec::new(),
            last_query_execution_text: content,
            next_query_execution_id: 1,
            elapsed_timer_task: None,
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
                    let previous_snapshot = this.last_observed_snapshot.clone();
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
                    if this.last_query_execution_text != snapshot.text {
                        this.update_query_execution_ranges(&snapshot.text, cx);
                    }
                    let cursor_moved_without_edit =
                        previous_snapshot.as_ref().is_some_and(|previous| {
                            previous.text == snapshot.text && previous.cursor != snapshot.cursor
                        });
                    this.last_observed_snapshot = Some(snapshot);
                    this.refresh_active_query(cx);
                    this.schedule_diagnostics(cx);
                    if cursor_moved_without_edit {
                        cx.emit(EditorPanelEvent::CursorMoved);
                    }
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
            selected_search_path: None,
            selected_connection_name: None,
        };
        panel.refresh_active_query(cx);
        panel.refresh_schema_cache(cx);
        panel
    }
    pub fn query_context(&self, cx: &App) -> (Option<QueryRange>, Vec<QueryRange>) {
        let state = self.editor.read(cx);
        let text = state.value().to_string();
        let cursor = state.cursor();
        let selected = state.selected_value().to_string();

        if !selected.trim().is_empty() {
            let selected_range = state.selected_range();
            let selected_text = text.get(selected_range.clone()).unwrap_or("");
            let offset = selected_range.start;
            let queries = query_ranges_in_text(selected_text)
                .into_iter()
                .map(|q| QueryRange {
                    range: (q.range.start + offset)..(q.range.end + offset),
                    trimmed_range: (q.trimmed_range.start + offset)..(q.trimmed_range.end + offset),
                    text: q.text,
                })
                .collect::<Vec<_>>();

            if queries.len() == 1 {
                return (queries.into_iter().next(), Vec::new());
            } else {
                return (None, queries);
            }
        }

        let queries = query_ranges_for_execution(&text, cursor);
        (None, queries)
    }

    pub(crate) fn has_nonempty_selection(&self, cx: &App) -> bool {
        !self.editor.read(cx).selected_value().trim().is_empty()
    }

    pub fn selected_search_path(&self) -> Option<String> {
        self.selected_search_path.clone()
    }

    pub fn selected_connection_name(&self) -> Option<&str> {
        self.selected_connection_name.as_deref()
    }

    pub fn set_selected_connection_name(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        if self.selected_connection_name == name {
            return;
        }
        self.selected_connection_name = name;
        self.refresh_schema_cache(cx);
        cx.notify();
    }

    pub fn search_path_label(&self) -> String {
        self.selected_search_path
            .as_deref()
            .unwrap_or("Default")
            .to_string()
    }

    pub fn available_search_paths(&self, cx: &App) -> Vec<String> {
        let mut schemas = self
            .schema_cache
            .as_ref()
            .map(|(_, schema)| {
                schema
                    .schemas
                    .iter()
                    .map(|schema| schema.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if let Some(config_schema) = self
            .data_source_manager
            .read(cx)
            .configs()
            .iter()
            .find(|config| Some(config.name.as_str()) == self.selected_connection_name())
            .map(|config| config.schema.trim())
            .filter(|schema| !schema.is_empty())
        {
            schemas.push(config_schema.to_string());
        }

        schemas.sort_by_key(|schema| schema.to_ascii_lowercase());
        schemas.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        schemas
    }

    pub fn set_search_path(&mut self, search_path: Option<String>, cx: &mut Context<Self>) {
        self.selected_search_path = search_path.filter(|schema| !schema.trim().is_empty());
        cx.notify();
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
            .configs()
            .iter()
            .find(|config| Some(config.name.as_str()) == self.selected_connection_name())
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

        let changed = match (&self.schema_cache, &schema_cache) {
            (Some((old_key, _)), Some((new_key, _))) => old_key != new_key,
            (None, None) => false,
            _ => true,
        };

        self.schema_cache = schema_cache;

        if self
            .selected_search_path
            .as_deref()
            .is_some_and(|selected| {
                !self
                    .available_search_paths(cx)
                    .iter()
                    .any(|schema| schema.eq_ignore_ascii_case(selected))
            })
        {
            self.selected_search_path = None;
        }

        if changed {
            cx.notify();
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

    pub(crate) fn begin_query_execution(
        &mut self,
        range: Option<QueryRange>,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let range = range?;
        let id = self.next_query_execution_id;
        self.next_query_execution_id += 1;
        let text = self.editor.read(cx).value().to_string();
        self.last_query_execution_text = text;
        self.cleanup_query_execution_markers_for_range(&range);
        self.query_executions.push(QueryExecutionMarker {
            id,
            range,
            status: QueryExecutionStatus::Running {
                started_at: Instant::now(),
            },
        });
        self.apply_query_execution_adornments(cx);
        self.start_elapsed_timer(cx);
        Some(id)
    }

    fn cleanup_query_execution_markers_for_range(&mut self, range: &QueryRange) {
        self.query_executions.retain(|marker| {
            !query_execution_marker_conflicts(&self.last_query_execution_text, marker, range)
        });
    }

    pub(crate) fn finish_query_execution(
        &mut self,
        execution_id: Option<u64>,
        succeeded: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(execution_id) = execution_id else {
            return;
        };

        if let Some(marker) = self
            .query_executions
            .iter_mut()
            .find(|marker| marker.id == execution_id)
        {
            marker.status = if succeeded {
                QueryExecutionStatus::Succeeded
            } else {
                QueryExecutionStatus::Failed
            };
            self.apply_query_execution_adornments(cx);
        }
    }

    fn update_query_execution_ranges(&mut self, new_text: &str, cx: &mut Context<Self>) {
        let Some(delta) = text_edit_delta(&self.last_query_execution_text, new_text) else {
            return;
        };

        self.last_query_execution_text = new_text.to_string();

        if self.query_executions.is_empty() {
            return;
        }

        self.query_executions = self
            .query_executions
            .drain(..)
            .filter_map(|mut marker| {
                marker.range = update_query_range_for_edit(&marker.range, &delta, new_text)?;
                Some(marker)
            })
            .collect();
        self.apply_query_execution_adornments(cx);
    }

    fn start_elapsed_timer(&mut self, cx: &mut Context<Self>) {
        if self.elapsed_timer_task.is_some() {
            return;
        }

        self.elapsed_timer_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;

                let keep_running = this
                    .update(cx, |this, cx| {
                        let has_running = this.query_executions.iter().any(|marker| {
                            matches!(marker.status, QueryExecutionStatus::Running { .. })
                        });
                        if has_running {
                            this.apply_query_execution_adornments(cx);
                        } else {
                            this.elapsed_timer_task = None;
                        }
                        has_running
                    })
                    .unwrap_or(false);

                if !keep_running {
                    break;
                }
            }
        }));
    }

    fn apply_query_execution_adornments(&mut self, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).value().to_string();
        let mut gutter_adornments = Vec::new();
        let mut inline_adornments = Vec::new();
        let mut adorned_lines = HashSet::new();

        for marker in self.query_executions.iter().rev() {
            if marker.range.trimmed_range.start > text.len() {
                continue;
            }

            let line = line_number_for_offset(&text, marker.range.trimmed_range.start);
            if !adorned_lines.insert(line) {
                continue;
            }

            let (icon, color, spin) = match marker.status {
                QueryExecutionStatus::Running { .. } => {
                    (IconName::Loader, hsla(0.0, 0.0, 0.65, 0.9), true)
                }
                QueryExecutionStatus::Succeeded => {
                    (IconName::Check, hsla(0.34, 0.55, 0.48, 1.0), false)
                }
                QueryExecutionStatus::Failed => {
                    (IconName::Close, hsla(0.0, 0.72, 0.58, 1.0), false)
                }
            };

            gutter_adornments.push(InputGutterAdornment {
                line,
                icon_path: icon.path(),
                color,
                spin,
            });

            if let QueryExecutionStatus::Running { started_at } = marker.status {
                inline_adornments.push(InputInlineAdornment {
                    offset: marker.range.trimmed_range.end.min(text.len()),
                    text: format!(" {}", format_elapsed(started_at.elapsed())).into(),
                    color: hsla(0.0, 0.0, 0.62, 0.78),
                });
            }
        }

        self.editor.update(cx, |state, cx| {
            state.set_adornments(gutter_adornments, inline_adornments, cx);
        });
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

    fn edit_selected_lines(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&str, Range<usize>) -> String,
    ) {
        let (text, line_range) = {
            let state = self.editor.read(cx);
            let text = state.value().to_string();
            let selected_range = state.selected_range();
            let line_range = selected_line_range(&text, selected_range, state.cursor());
            (text, line_range)
        };

        let new_text = edit(&text, line_range.clone());
        if new_text == text {
            return;
        }

        let replacement =
            new_text[line_range.start..new_text.len() - (text.len() - line_range.end)].to_string();
        self.editor.update(cx, |editor, cx| {
            editor.set_selected_range(line_range, cx);
            editor.replace(replacement, window, cx);
        });
    }

    fn on_toggle_comment_lines(
        &mut self,
        _: &ToggleCommentLines,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_selected_lines(window, cx, toggle_comment_lines);
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
            .on_action(cx.listener(Self::on_toggle_comment_lines))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_selected_lines_at_column_zero() {
        let text = "select 1;\n  select 2;\nselect 3;";
        let range = selected_line_range(text, 0..22, 0);

        assert_eq!(
            comment_lines(text, range),
            "-- select 1;\n--   select 2;\nselect 3;"
        );
    }

    #[test]
    fn comments_current_empty_line() {
        let text = "select 1;\n\nselect 2;";
        let range = selected_line_range(text, 10..10, 10);

        assert_eq!(comment_lines(text, range), "select 1;\n-- \nselect 2;");
    }

    #[test]
    fn selection_ending_at_next_line_start_does_not_include_next_line() {
        let text = "select 1;\nselect 2;";
        let range = selected_line_range(text, 0..10, 0);

        assert_eq!(comment_lines(text, range), "-- select 1;\nselect 2;");
    }

    #[test]
    fn uncomments_lines_with_optional_leading_whitespace() {
        let text = "-- select 1; -- some doc here\n  -- select 2;";
        let range = selected_line_range(text, 0..text.len(), 0);

        assert_eq!(
            uncomment_lines(text, range),
            "select 1; -- some doc here\n  select 2;"
        );
    }

    #[test]
    fn uncomment_keeps_inline_comments() {
        let text = "select 1; -- some doc here";
        let range = selected_line_range(text, 0..text.len(), 0);

        assert_eq!(uncomment_lines(text, range), text);
    }

    #[test]
    fn uncomment_supports_marker_without_trailing_space() {
        let text = "--select 1;\n\t-- select 2;";
        let range = selected_line_range(text, 0..text.len(), 0);

        assert_eq!(uncomment_lines(text, range), "select 1;\n\tselect 2;");
    }

    #[test]
    fn selected_query_range_trims_absolute_selection() {
        let text = "select 1;\n\n  select 2;  \nselect 3;";
        let range = 11..25;
        let query = selected_query_range(text, range.clone()).unwrap();

        assert_eq!(query.range, range);
        assert_eq!(query.trimmed_range, 13..22);
        assert_eq!(query.text, "select 2;");
    }

    #[test]
    fn query_execution_range_follows_inserted_lines_before_query() {
        let old_text = "select 1;\nselect 1;";
        let new_text = "-- added\n\nselect 1;\nselect 1;";
        let marker = QueryRange {
            range: 10..19,
            trimmed_range: 10..19,
            text: "select 1;".to_string(),
        };
        let delta = text_edit_delta(old_text, new_text).unwrap();
        let updated = update_query_range_for_edit(&marker, &delta, new_text).unwrap();

        assert_eq!(updated.trimmed_range, 20..29);
        assert_eq!(&new_text[updated.trimmed_range.clone()], "select 1;");
    }

    #[test]
    fn query_execution_range_does_not_jump_to_duplicate_query_text() {
        let old_text = "select 1;\nselect 1;";
        let new_text = "select 1;\n\nselect 1;";
        let marker = QueryRange {
            range: 10..19,
            trimmed_range: 10..19,
            text: "select 1;".to_string(),
        };
        let delta = text_edit_delta(old_text, new_text).unwrap();
        let updated = update_query_range_for_edit(&marker, &delta, new_text).unwrap();

        assert_eq!(updated.trimmed_range, 11..20);
        assert_eq!(&new_text[updated.trimmed_range.clone()], "select 1;");
    }

    #[test]
    fn query_execution_range_is_removed_when_query_is_deleted() {
        let old_text = "select 1;\nselect 2;";
        let new_text = "select 1;\n";
        let marker = QueryRange {
            range: 10..19,
            trimmed_range: 10..19,
            text: "select 2;".to_string(),
        };
        let delta = text_edit_delta(old_text, new_text).unwrap();

        assert!(update_query_range_for_edit(&marker, &delta, new_text).is_none());
    }

    #[test]
    fn query_execution_marker_conflicts_on_same_gutter_line() {
        let text = "select 1; select 2;\nselect 3;";
        let marker = QueryExecutionMarker {
            id: 1,
            range: QueryRange {
                range: 0..9,
                trimmed_range: 0..9,
                text: "select 1;".to_string(),
            },
            status: QueryExecutionStatus::Succeeded,
        };
        let next_range = QueryRange {
            range: 10..19,
            trimmed_range: 10..19,
            text: "select 2;".to_string(),
        };

        assert!(query_execution_marker_conflicts(text, &marker, &next_range));
    }

    #[test]
    fn query_execution_marker_does_not_conflict_on_different_gutter_line() {
        let text = "select 1;\nselect 2;";
        let marker = QueryExecutionMarker {
            id: 1,
            range: QueryRange {
                range: 0..9,
                trimmed_range: 0..9,
                text: "select 1;".to_string(),
            },
            status: QueryExecutionStatus::Succeeded,
        };
        let next_range = QueryRange {
            range: 10..19,
            trimmed_range: 10..19,
            text: "select 2;".to_string(),
        };

        assert!(!query_execution_marker_conflicts(
            text,
            &marker,
            &next_range
        ));
    }

    #[test]
    fn formats_running_query_elapsed_time() {
        assert_eq!(format_elapsed(Duration::from_millis(850)), "850 ms");
        assert_eq!(format_elapsed(Duration::from_millis(1_250)), "1.2 s");
        assert_eq!(format_elapsed(Duration::from_secs(12)), "12 s");
    }
}
