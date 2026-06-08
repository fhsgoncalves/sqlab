use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, ScrollHandle, StatefulInteractiveElement, Styled, Task,
    Window, actions, div, point, prelude::FluentBuilder, px,
};
use gpui_component::IconName;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::Scrollbar;
use gpui_component::{ActiveTheme, Selectable, Sizable, h_flex, v_flex};

use crate::ui::search::{SearchOptions, find_text_matches};

actions!(
    project_search,
    [
        ToggleProjectSearch,
        CloseProjectSearch,
        ConfirmProjectSearch,
        SelectPreviousResult,
        SelectNextResult,
        ToggleReplace,
        ToggleCaseSensitive,
        ToggleRegex,
        ToggleWholeWord,
        ToggleFuzzy,
        ReplaceNext,
        ReplaceAll,
    ]
);

const CONTEXT: &str = "ProjectSearch";
const RESULT_LIST_HEIGHT: f32 = 430.0;
const RESULT_ROW_HEIGHT: f32 = 58.0;
const SEARCH_DEBOUNCE: Duration = Duration::from_secs(1);
const MAX_SEARCH_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SEARCH_RESULTS: usize = 2_000;

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub file: PathBuf,
    pub line_number: usize,
    pub line_content: String,
    pub match_start: usize,
    pub match_end: usize,
    pub score: i64,
    pub context_before: Vec<(usize, String)>,
    pub context_after: Vec<(usize, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreviewKey {
    file: PathBuf,
    line_number: usize,
    match_start: usize,
    match_end: usize,
    text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreviewData {
    key: PreviewKey,
    relative_path: String,
    language: String,
    text: String,
    line_numbers: Vec<usize>,
    selected_line_ix: usize,
}

pub struct ProjectSearch {
    search_input: Entity<InputState>,
    replace_input: Entity<InputState>,
    preview_input: Entity<InputState>,
    root: Option<PathBuf>,
    results: Vec<SearchResult>,
    filtered_results: Vec<usize>,
    selected_ix: usize,
    visible: bool,
    replace_mode: bool,
    case_sensitive: bool,
    use_regex: bool,
    whole_word: bool,
    use_fuzzy: bool,
    include_ignored: bool,
    searching: bool,
    search_generation: u64,
    results_truncated: bool,
    preview_generation: u64,
    preview_key: Option<PreviewKey>,
    preview_language: Option<String>,
    focus_handle: FocusHandle,
    results_scroll_handle: ScrollHandle,
    pending_search_task: Option<Task<()>>,
    _search_subscription: gpui::Subscription,
    _replace_subscription: gpui::Subscription,
}

pub enum ProjectSearchEvent {
    OpenFileAtPosition(PathBuf, usize, usize),
    Closed,
}

impl EventEmitter<ProjectSearchEvent> for ProjectSearch {}

impl ProjectSearch {
    pub fn new(root: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search in project..."));
        let replace_input = cx.new(|cx| InputState::new(window, cx).placeholder("Replace with..."));
        let preview_input = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("sql")
                .line_number(false)
                .indent_guides(false)
                .folding(false)
                .searchable(false)
        });

        let search_subscription = cx.subscribe_in(&search_input, window, {
            move |this: &mut ProjectSearch, _input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.perform_search(cx);
                }
                InputEvent::PressEnter { .. } => {
                    this.navigate_to_selected(window, cx);
                }
                _ => {}
            }
        });

        let replace_subscription = cx.subscribe_in(&replace_input, window, {
            move |this: &mut ProjectSearch, _input, event: &InputEvent, _window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    this.replace_next(cx);
                }
                _ => {}
            }
        });

        Self {
            search_input,
            replace_input,
            preview_input,
            root,
            results: Vec::new(),
            filtered_results: Vec::new(),
            selected_ix: 0,
            visible: false,
            replace_mode: false,
            case_sensitive: false,
            use_regex: false,
            whole_word: false,
            use_fuzzy: false,
            include_ignored: false,
            searching: false,
            search_generation: 0,
            results_truncated: false,
            preview_generation: 0,
            preview_key: None,
            preview_language: None,
            focus_handle,
            results_scroll_handle: ScrollHandle::default(),
            pending_search_task: None,
            _search_subscription: search_subscription,
            _replace_subscription: replace_subscription,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible {
            self.close(window, cx);
        } else {
            self.open(window, cx);
        }
    }

    fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = true;
        cx.notify();
        window.focus(&self.search_input.read(cx).focus_handle(cx), cx);
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.search_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        self.results.clear();
        self.filtered_results.clear();
        self.results_truncated = false;
        self.searching = false;
        self.search_generation = self.search_generation.wrapping_add(1);
        self.pending_search_task = None;
        cx.emit(ProjectSearchEvent::Closed);
        cx.notify();
    }

    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.root = Some(root);
        self.results.clear();
        self.filtered_results.clear();
        self.results_truncated = false;
        if self.visible && !self.search_input.read(cx).value().is_empty() {
            self.perform_search(cx);
            return;
        }
        cx.notify();
    }

    pub fn toggle_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_mode = !self.replace_mode;
        if self.replace_mode {
            window.focus(&self.replace_input.read(cx).focus_handle(cx), cx);
        } else {
            window.focus(&self.search_input.read(cx).focus_handle(cx), cx);
        }
        cx.notify();
    }

    fn perform_search(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value();
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;

        if query.is_empty() {
            self.results.clear();
            self.filtered_results.clear();
            self.results_truncated = false;
            self.selected_ix = 0;
            self.searching = false;
            self.pending_search_task = None;
            cx.notify();
            return;
        }

        self.searching = true;
        cx.notify();

        let Some(root) = self.root.clone() else {
            self.results.clear();
            self.filtered_results.clear();
            self.results_truncated = false;
            self.selected_ix = 0;
            self.searching = false;
            self.pending_search_task = None;
            cx.notify();
            return;
        };
        let query_clone = query.clone();
        let case_sensitive = self.case_sensitive;
        let use_regex = self.use_regex;
        let whole_word = self.whole_word;
        let use_fuzzy = self.use_fuzzy;
        let include_ignored = self.include_ignored;

        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;

            let should_search = this
                .update(cx, |this, _| {
                    this.visible && this.search_generation == generation
                })
                .unwrap_or(false);
            if !should_search {
                return;
            }

            let search_result = cx
                .background_executor()
                .spawn(async move {
                    search_in_directory(
                        &root,
                        &query_clone,
                        case_sensitive,
                        use_regex,
                        whole_word,
                        use_fuzzy,
                        include_ignored,
                    )
                })
                .await;

            this.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                this.results = search_result.results;
                this.results_truncated = search_result.truncated;
                this.filtered_results = (0..this.results.len()).collect();
                this.selected_ix = 0;
                this.results_scroll_handle.set_offset(point(px(0.), px(0.)));
                this.searching = false;
                cx.notify();
            })
            .ok();
        });

        self.pending_search_task = Some(task);
    }

    fn select_previous(&mut self, cx: &mut Context<Self>) {
        if self.selected_ix == 0 {
            return;
        }
        self.selected_ix = self.selected_ix.saturating_sub(1);
        self.scroll_selected_result_into_view();
        cx.notify();
    }

    fn select_next(&mut self, cx: &mut Context<Self>) {
        if self.filtered_results.is_empty() {
            return;
        }
        self.selected_ix =
            (self.selected_ix + 1).min(self.filtered_results.len().saturating_sub(1));
        self.scroll_selected_result_into_view();
        cx.notify();
    }

    fn scroll_selected_result_into_view(&self) {
        let selected_top = px(self.selected_ix as f32 * RESULT_ROW_HEIGHT);
        let selected_bottom = selected_top + px(RESULT_ROW_HEIGHT);
        let viewport_height = px(RESULT_LIST_HEIGHT);
        let mut offset = self.results_scroll_handle.offset();
        let visible_top = -offset.y;
        let visible_bottom = visible_top + viewport_height;

        if selected_top < visible_top {
            offset.y = -selected_top;
        } else if selected_bottom > visible_bottom {
            offset.y = -(selected_bottom - viewport_height);
        }

        self.results_scroll_handle.set_offset(offset);
    }

    fn navigate_to_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(&result_idx) = self.filtered_results.get(self.selected_ix) {
            if let Some(result) = self.results.get(result_idx) {
                let match_column =
                    byte_offset_to_character_offset(&result.line_content, result.match_start);
                cx.emit(ProjectSearchEvent::OpenFileAtPosition(
                    result.file.clone(),
                    result.line_number,
                    match_column,
                ));
                self.close(window, cx);
            }
        }
    }

    fn replace_next(&mut self, cx: &mut Context<Self>) {
        if let Some(&result_idx) = self.filtered_results.get(self.selected_ix) {
            if let Some(result) = self.results.get(result_idx) {
                let replace_text = self.replace_input.read(cx).value();
                let content = std::fs::read_to_string(&result.file).unwrap_or_default();
                let lines: Vec<&str> = content.lines().collect();
                if let Some(line) = lines.get(result.line_number - 1) {
                    let line_start =
                        content[..line.as_ptr() as usize - content.as_ptr() as usize].len();
                    let match_start = line_start + result.match_start;
                    let match_end = line_start + result.match_end;

                    let mut new_content = content.clone();
                    new_content.replace_range(match_start..match_end, &replace_text);

                    if std::fs::write(&result.file, &new_content).is_ok() {
                        self.perform_search(cx);
                    }
                }
            }
        }
    }

    fn replace_all(&mut self, cx: &mut Context<Self>) {
        let replace_text = self.replace_input.read(cx).value();
        let query = self.search_input.read(cx).value();

        if query.is_empty() {
            return;
        }

        let mut modified_files = std::collections::HashSet::new();

        for result in &self.results {
            if modified_files.contains(&result.file) {
                continue;
            }
            modified_files.insert(result.file.clone());

            let content = std::fs::read_to_string(&result.file).unwrap_or_default();
            let new_content = if self.use_regex {
                if let Ok(regex) = regex::Regex::new(&query) {
                    if self.case_sensitive {
                        regex
                            .replace_all(&content, replace_text.as_str())
                            .to_string()
                    } else {
                        let regex_ci = regex::RegexBuilder::new(&query)
                            .case_insensitive(true)
                            .build()
                            .ok();
                        if let Some(r) = regex_ci {
                            r.replace_all(&content, replace_text.as_str()).to_string()
                        } else {
                            content
                        }
                    }
                } else {
                    content
                }
            } else {
                if self.case_sensitive {
                    content.replace(query.as_str(), replace_text.as_str())
                } else {
                    replace_case_insensitive(&content, query.as_str(), replace_text.as_str())
                }
            };

            let _ = std::fs::write(&result.file, &new_content);
        }

        self.perform_search(cx);
    }
}

struct DirectorySearchResult {
    results: Vec<SearchResult>,
    truncated: bool,
}

fn search_in_directory(
    root: &Path,
    query: &str,
    case_sensitive: bool,
    use_regex: bool,
    whole_word: bool,
    use_fuzzy: bool,
    include_ignored: bool,
) -> DirectorySearchResult {
    let mut builder = ignore::WalkBuilder::new(root);
    builder.git_ignore(!include_ignored);
    builder.git_global(!include_ignored);
    builder.git_exclude(!include_ignored);
    builder.ignore(!include_ignored);
    builder.hidden(false);
    builder.require_git(false);
    builder.follow_links(false);
    builder.filter_entry(|entry| entry.file_name().to_str() != Some(".git"));

    let mut results = Vec::new();
    let mut truncated = false;
    let options = SearchOptions {
        case_sensitive,
        use_regex,
        whole_word,
        use_fuzzy: use_fuzzy && !use_regex,
    };

    'entries: for entry in builder.build().flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_file()) {
            continue;
        }

        let path = entry.path();
        if std::fs::metadata(path).map_or(true, |metadata| metadata.len() > MAX_SEARCH_FILE_BYTES) {
            continue;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let lines: Vec<&str> = content.lines().collect();
        for text_match in find_text_matches(&content, query, options) {
            let line_idx = text_match.line_number.saturating_sub(1);
            let context_start = line_idx.saturating_sub(4);
            let context_before = lines
                .get(context_start..line_idx)
                .unwrap_or(&[])
                .iter()
                .enumerate()
                .map(|(ix, line)| (context_start + ix + 1, (*line).to_string()))
                .collect();
            let context_after = lines
                .iter()
                .enumerate()
                .skip(line_idx + 1)
                .take(4)
                .map(|(ix, line)| (ix + 1, (*line).to_string()))
                .collect();

            results.push(SearchResult {
                file: path.to_path_buf(),
                line_number: text_match.line_number,
                line_content: text_match.line_content,
                match_start: text_match.match_start,
                match_end: text_match.match_end,
                score: text_match.score,
                context_before,
                context_after,
            });

            if results.len() >= MAX_SEARCH_RESULTS {
                truncated = true;
                break 'entries;
            }
        }
    }

    results.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.file.cmp(&right.file))
            .then_with(|| left.line_number.cmp(&right.line_number))
    });
    DirectorySearchResult { results, truncated }
}

fn replace_case_insensitive(content: &str, query: &str, replacement: &str) -> String {
    let lower_content = content.to_lowercase();
    let lower_query = query.to_lowercase();

    let mut result = String::new();
    let mut last_end = 0;
    let mut start = 0;

    while let Some(pos) = lower_content[start..].find(&lower_query) {
        let match_start = start + pos;
        let match_end = match_start + query.len();

        result.push_str(&content[last_end..match_start]);
        result.push_str(replacement);

        last_end = match_end;
        start = match_start + 1;
    }

    result.push_str(&content[last_end..]);
    result
}

fn byte_offset_to_character_offset(line: &str, byte_offset: usize) -> usize {
    line.get(..byte_offset)
        .map(|prefix| prefix.chars().count())
        .unwrap_or_else(|| line.chars().count())
}

fn preview_lines(result: &SearchResult) -> Vec<(usize, String)> {
    let mut lines =
        Vec::with_capacity(result.context_before.len() + 1 + result.context_after.len());
    lines.extend(result.context_before.iter().cloned());
    lines.push((result.line_number, result.line_content.clone()));
    lines.extend(result.context_after.iter().cloned());
    lines
}

fn preview_text(lines: &[(usize, String)]) -> String {
    lines
        .iter()
        .map(|(_, line)| line.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn language_for_path(path: &Path) -> String {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("psql") => "sql".to_string(),
        Some(ext) if !ext.is_empty() => ext.to_string(),
        _ => "txt".to_string(),
    }
}

impl PreviewData {
    fn from_result(root: &Path, result: &SearchResult) -> Self {
        let lines = preview_lines(result);
        let selected_line_ix = result.context_before.len();
        let text = preview_text(&lines);
        let relative_path = result
            .file
            .strip_prefix(root)
            .unwrap_or(&result.file)
            .to_string_lossy()
            .to_string();

        Self {
            key: PreviewKey {
                file: result.file.clone(),
                line_number: result.line_number,
                match_start: result.match_start,
                match_end: result.match_end,
                text: text.clone(),
            },
            relative_path,
            language: language_for_path(&result.file),
            text,
            line_numbers: lines
                .iter()
                .map(|(line_number, _)| *line_number)
                .collect::<Vec<_>>(),
            selected_line_ix,
        }
    }
}

impl Render for ProjectSearch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }

        let selected_preview = self.selected_preview_data();
        self.queue_preview_update(selected_preview.as_ref(), window, cx);

        let total_results = self.results.len();
        let result_text = if self.searching {
            "Searching...".to_string()
        } else if total_results == 0 {
            let query = self.search_input.read(cx).value();
            if query.is_empty() {
                "Type to search".to_string()
            } else {
                "No results found".to_string()
            }
        } else {
            let suffix = if self.results_truncated { "+" } else { "" };
            format!("{}{} results", total_results, suffix)
        };

        v_flex()
            .id("project-search")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(ProjectSearch::on_action_toggle))
            .on_action(cx.listener(ProjectSearch::on_action_close))
            .on_action(cx.listener(ProjectSearch::on_action_confirm))
            .on_action(cx.listener(ProjectSearch::on_action_select_previous))
            .on_action(cx.listener(ProjectSearch::on_action_select_next))
            .on_action(cx.listener(ProjectSearch::on_action_toggle_replace))
            .on_action(cx.listener(ProjectSearch::on_action_toggle_case_sensitive))
            .on_action(cx.listener(ProjectSearch::on_action_toggle_regex))
            .on_action(cx.listener(ProjectSearch::on_action_toggle_whole_word))
            .on_action(cx.listener(ProjectSearch::on_action_toggle_fuzzy))
            .w(px(960.))
            .max_h(px(600.))
            .overflow_hidden()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .shadow_md()
            .child(
                // Search input row
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div().flex_1().child(
                                    Input::new(&self.search_input).suffix(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Button::new("case-sensitive")
                                                    .selected(self.case_sensitive)
                                                    .xsmall()
                                                    .compact()
                                                    .ghost()
                                                    .icon(IconName::CaseSensitive)
                                                    .tooltip("Match Case (Alt+C)")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.case_sensitive = !this.case_sensitive;
                                                        this.perform_search(cx);
                                                    })),
                                            )
                                            .child(
                                                Button::new("whole-word")
                                                    .selected(self.whole_word)
                                                    .xsmall()
                                                    .compact()
                                                    .ghost()
                                                    .label("W")
                                                    .tooltip("Whole Word (Alt+W)")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.whole_word = !this.whole_word;
                                                        this.perform_search(cx);
                                                    })),
                                            )
                                            .child(
                                                Button::new("regex")
                                                    .selected(self.use_regex)
                                                    .xsmall()
                                                    .compact()
                                                    .ghost()
                                                    .label(".*")
                                                    .tooltip("Use Regex (Alt+R)")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.use_regex = !this.use_regex;
                                                        if this.use_regex {
                                                            this.use_fuzzy = false;
                                                        }
                                                        this.perform_search(cx);
                                                    })),
                                            )
                                            .child(
                                                Button::new("fuzzy")
                                                    .selected(self.use_fuzzy)
                                                    .xsmall()
                                                    .compact()
                                                    .ghost()
                                                    .label("fz")
                                                    .tooltip("Fuzzy Search")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.use_fuzzy = !this.use_fuzzy;
                                                        if this.use_fuzzy {
                                                            this.use_regex = false;
                                                        }
                                                        this.perform_search(cx);
                                                    })),
                                            ),
                                    ),
                                ),
                            )
                            .child(
                                Button::new("replace-toggle")
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Replace)
                                    .selected(self.replace_mode)
                                    .tooltip("Toggle Replace")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_replace(window, cx);
                                    })),
                            ),
                    ),
            )
            .when(self.replace_mode, |this| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(div().flex_1().child(Input::new(&self.replace_input)))
                                .child(
                                    Button::new("replace-next")
                                        .xsmall()
                                        .label("Replace")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.replace_next(cx);
                                        })),
                                )
                                .child(Button::new("replace-all").xsmall().label("All").on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.replace_all(cx);
                                    }),
                                )),
                        ),
                )
            })
            .child(
                // Results header
                div()
                    .px_3()
                    .py_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(result_text),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Button::new("prev-result")
                                            .xsmall()
                                            .ghost()
                                            .icon(IconName::ChevronUp)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.select_previous(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("next-result")
                                            .xsmall()
                                            .ghost()
                                            .icon(IconName::ChevronDown)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.select_next(cx);
                                            })),
                                    ),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .h(px(RESULT_LIST_HEIGHT))
                    .overflow_hidden()
                    .items_start()
                    .child(
                        div()
                            .id("project-search-results-list")
                            .w(px(360.))
                            .h_full()
                            .relative()
                            .border_r_1()
                            .border_color(cx.theme().border)
                            .child(
                                div()
                                    .id("project-search-results-scroll")
                                    .size_full()
                                    .track_scroll(&self.results_scroll_handle)
                                    .overflow_y_scroll()
                                    .children({
                                        let mut children: Vec<gpui::AnyElement> = Vec::new();

                                        if self.results.is_empty() && !self.searching {
                                            children.push(
                                                div()
                                                    .px_3()
                                                    .py_6()
                                                    .text_sm()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("Enter a search term to find in project")
                                                    .into_any_element(),
                                            );
                                        } else {
                                            for (ix, result_ix) in
                                                self.filtered_results.iter().enumerate()
                                            {
                                                if let Some(result) = self.results.get(*result_ix) {
                                                    children.push(self.render_result_row(
                                                        result,
                                                        ix,
                                                        ix == self.selected_ix,
                                                        cx,
                                                    ));
                                                }
                                            }
                                        }

                                        children
                                    }),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .right_0()
                                    .bottom_0()
                                    .child(Scrollbar::vertical(&self.results_scroll_handle)),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .overflow_hidden()
                            .child(self.render_preview(selected_preview.as_ref(), cx)),
                    ),
            )
            .into_any_element()
    }
}

impl ProjectSearch {
    fn selected_preview_data(&self) -> Option<PreviewData> {
        self.filtered_results
            .get(self.selected_ix)
            .and_then(|ix| self.results.get(*ix))
            .and_then(|result| {
                self.root
                    .as_deref()
                    .map(|root| PreviewData::from_result(root, result))
            })
    }

    fn queue_preview_update(
        &mut self,
        preview: Option<&PreviewData>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next_key = preview.map(|preview| preview.key.clone());
        if self.preview_key == next_key {
            return;
        }

        self.preview_key = next_key;
        self.preview_generation = self.preview_generation.wrapping_add(1);
        let generation = self.preview_generation;
        let preview = preview.cloned();

        cx.defer_in(window, move |this, window, cx| {
            if this.preview_generation != generation {
                return;
            }

            let Some(preview) = preview else {
                this.preview_language = None;
                this.preview_input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                });
                return;
            };

            let language_changed =
                this.preview_language.as_deref() != Some(preview.language.as_str());
            if language_changed {
                this.preview_language = Some(preview.language.clone());
            }

            this.preview_input.update(cx, |input, cx| {
                if language_changed {
                    input.set_highlighter(preview.language, cx);
                }
                input.set_value(preview.text, window, cx);
            });
        });
    }

    fn render_result_row(
        &self,
        result: &SearchResult,
        ix: usize,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let line_number_str = format!("{:>4}", result.line_number);
        let relative_path = result
            .file
            .strip_prefix(self.root.as_deref().unwrap_or(&result.file))
            .unwrap_or(&result.file)
            .to_string_lossy()
            .to_string();

        div()
            .id(format!("project-search-row-{}", ix))
            .w_full()
            .h(px(RESULT_ROW_HEIGHT))
            .overflow_hidden()
            .px_3()
            .py_1()
            .cursor_pointer()
            .when(selected, |this| {
                this.bg(cx.theme().accent)
                    .text_color(cx.theme().accent_foreground)
            })
            .when(!selected, |this| {
                this.text_color(cx.theme().foreground)
                    .hover(|style| style.bg(cx.theme().accent.opacity(0.12)))
            })
            .child(
                v_flex()
                    .w_full()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .truncate()
                                    .flex_1()
                                    .child(relative_path),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .flex_none()
                                    .text_color(if selected {
                                        cx.theme().accent_foreground.opacity(0.7)
                                    } else {
                                        cx.theme().muted_foreground
                                    })
                                    .child(line_number_str),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .flex_1()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_xs()
                            .text_color(if selected {
                                cx.theme().accent_foreground.opacity(0.78)
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(result.line_content.clone()),
                    ),
            )
            .on_click({
                cx.listener(move |this: &mut ProjectSearch, _, window, cx| {
                    this.selected_ix = ix;
                    this.navigate_to_selected(window, cx);
                })
            })
            .into_any_element()
    }

    fn render_preview(
        &self,
        preview: Option<&PreviewData>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(preview) = preview else {
            return div()
                .px_4()
                .py_6()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No preview")
                .into_any_element();
        };

        v_flex()
            .w_full()
            .h_full()
            .overflow_hidden()
            .child(
                div()
                    .overflow_hidden()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_sm()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .truncate()
                    .child(preview.relative_path.clone()),
            )
            .child(
                h_flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .w(px(52.))
                            .flex_none()
                            .overflow_hidden()
                            .py_2()
                            .border_r_1()
                            .border_color(cx.theme().border.opacity(0.5))
                            .children(
                                preview
                                    .line_numbers
                                    .iter()
                                    .enumerate()
                                    .map(|(ix, line_number)| {
                                        div()
                                            .h(px(20.))
                                            .px_2()
                                            .text_xs()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_color(if ix == preview.selected_line_ix {
                                                cx.theme().foreground
                                            } else {
                                                cx.theme().muted_foreground
                                            })
                                            .child(format!("{line_number:>4}"))
                                            .into_any_element()
                                    })
                                    .collect::<Vec<_>>(),
                            ),
                    )
                    .child(
                        div().flex_1().h_full().overflow_hidden().child(
                            Input::new(&self.preview_input)
                                .h_full()
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .disabled(true)
                                .tab_index(-1)
                                .p_0(),
                        ),
                    ),
            )
            .into_any_element()
    }
}

impl Focusable for ProjectSearch {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// Action handlers
impl ProjectSearch {
    fn on_action_toggle(
        &mut self,
        _: &ToggleProjectSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle(window, cx);
    }

    fn on_action_close(
        &mut self,
        _: &CloseProjectSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close(window, cx);
    }

    fn on_action_confirm(
        &mut self,
        _: &ConfirmProjectSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_to_selected(window, cx);
    }

    fn on_action_select_previous(
        &mut self,
        _: &SelectPreviousResult,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_previous(cx);
    }

    fn on_action_select_next(
        &mut self,
        _: &SelectNextResult,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_next(cx);
    }

    fn on_action_toggle_replace(
        &mut self,
        _: &ToggleReplace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_replace(window, cx);
    }

    fn on_action_toggle_case_sensitive(
        &mut self,
        _: &ToggleCaseSensitive,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.case_sensitive = !self.case_sensitive;
        self.perform_search(cx);
    }

    fn on_action_toggle_regex(
        &mut self,
        _: &ToggleRegex,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.use_regex = !self.use_regex;
        if self.use_regex {
            self.use_fuzzy = false;
        }
        self.perform_search(cx);
    }

    fn on_action_toggle_whole_word(
        &mut self,
        _: &ToggleWholeWord,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.whole_word = !self.whole_word;
        self.perform_search(cx);
    }

    fn on_action_toggle_fuzzy(
        &mut self,
        _: &ToggleFuzzy,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.use_fuzzy = !self.use_fuzzy;
        if self.use_fuzzy {
            self.use_regex = false;
        }
        self.perform_search(cx);
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::{PreviewData, SearchResult, byte_offset_to_character_offset, search_in_directory};

    #[test]
    fn converts_match_byte_offset_to_character_offset() {
        let line = "select café from orders";
        let byte_offset = line.find("from").unwrap();

        assert_eq!(byte_offset_to_character_offset(line, byte_offset), 12);
    }

    #[test]
    fn builds_preview_data_with_context_and_real_line_numbers() {
        let root = PathBuf::from("/workspace");
        let result = SearchResult {
            file: root.join("queries/report.psql"),
            line_number: 10,
            line_content: "select * from orders;".to_string(),
            match_start: 0,
            match_end: 6,
            score: 10,
            context_before: vec![
                (8, "with orders as (".to_string()),
                (9, "  select 1".to_string()),
            ],
            context_after: vec![(11, ")".to_string())],
        };

        let preview = PreviewData::from_result(&root, &result);

        assert_eq!(preview.relative_path, "queries/report.psql");
        assert_eq!(preview.language, "sql");
        assert_eq!(preview.line_numbers, vec![8, 9, 10, 11]);
        assert_eq!(preview.selected_line_ix, 2);
        assert_eq!(
            preview.text,
            "with orders as (\n  select 1\nselect * from orders;\n)"
        );
    }

    #[test]
    fn search_respects_gitignore_and_prunes_git_directory() {
        let root =
            std::env::temp_dir().join(format!("sqlab-project-search-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("ignored-dir")).unwrap();
        fs::write(root.join(".gitignore"), "ignored.sql\nignored-dir/\n").unwrap();
        fs::write(root.join("visible.sql"), "select from visible;").unwrap();
        fs::write(root.join("ignored.sql"), "select from ignored;").unwrap();
        fs::write(
            root.join("ignored-dir").join("nested.sql"),
            "select from nested;",
        )
        .unwrap();
        fs::write(
            root.join(".git").join("config"),
            "select from git metadata;",
        )
        .unwrap();

        let default_result =
            search_in_directory(&root, "select", false, false, false, false, false);
        let default_files = default_result
            .results
            .iter()
            .map(|result| result.file.strip_prefix(&root).unwrap().to_path_buf())
            .collect::<Vec<_>>();
        assert_eq!(default_files, vec![PathBuf::from("visible.sql")]);

        let include_ignored_result =
            search_in_directory(&root, "select", false, false, false, false, true);
        let include_ignored_files = include_ignored_result
            .results
            .iter()
            .map(|result| result.file.strip_prefix(&root).unwrap().to_path_buf())
            .collect::<Vec<_>>();
        assert!(include_ignored_files.contains(&PathBuf::from("visible.sql")));
        assert!(include_ignored_files.contains(&PathBuf::from("ignored.sql")));
        assert!(include_ignored_files.contains(&PathBuf::from("ignored-dir/nested.sql")));
        assert!(!include_ignored_files.contains(&PathBuf::from(".git/config")));

        fs::remove_dir_all(root).unwrap();
    }
}
