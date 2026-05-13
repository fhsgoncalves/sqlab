use std::path::{Path, PathBuf};

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyBinding, ParentElement, Render, StatefulInteractiveElement, Styled, Window,
    actions, div, prelude::FluentBuilder, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme, h_flex, v_flex};

actions!(
    file_search,
    [
        ToggleFileSearch,
        CloseFileSearch,
        ConfirmFileSearch,
        SelectPreviousFile,
        SelectNextFile
    ]
);

const CONTEXT: &str = "FileSearch";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPreviousFile, Some(CONTEXT)),
        KeyBinding::new("down", SelectNextFile, Some(CONTEXT)),
        KeyBinding::new("enter", ConfirmFileSearch, Some(CONTEXT)),
        KeyBinding::new("escape", CloseFileSearch, Some(CONTEXT)),
    ]);
}

pub struct FileSearch {
    input: Entity<InputState>,
    all_files: Vec<PathBuf>,
    recent_files: Vec<PathBuf>,
    filtered_indices: Vec<usize>,
    selected_ix: usize,
    visible: bool,
    root: PathBuf,
    include_ignored: bool,
    focus_handle: FocusHandle,
    _input_subscription: gpui::Subscription,
}

pub enum FileSearchEvent {
    OpenFile(PathBuf),
    Closed,
}

impl EventEmitter<FileSearchEvent> for FileSearch {}

impl FileSearch {
    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search files by name...")
        });

        let all_files = Self::collect_files(&root, false);

        let input_subscription = cx.subscribe_in(&input, window, {
            move |this: &mut FileSearch, _input, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::Change => {
                        this.filter_results(cx);
                    }
                    InputEvent::PressEnter { .. } => {
                        this.confirm_selection(window, cx);
                    }
                    _ => {}
                }
            }
        });

        let file_count = all_files.len();

        Self {
            input,
            all_files,
            recent_files: Vec::new(),
            filtered_indices: (0..file_count).collect(),
            selected_ix: 0,
            visible: false,
            root,
            include_ignored: false,
            focus_handle,
            _input_subscription: input_subscription,
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
        self.all_files = Self::collect_files(&self.root, self.include_ignored);
        self.filter_results(cx);
        cx.notify();
        window.focus(&self.input.read(cx).focus_handle(cx), cx);
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        cx.emit(FileSearchEvent::Closed);
        cx.notify();
    }

    pub fn add_recent(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.recent_files.retain(|p| p != &path);
        self.recent_files.insert(0, path);
        self.recent_files.truncate(20);
        cx.notify();
    }

    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.root = root;
        self.all_files = Self::collect_files(&self.root, self.include_ignored);
        self.filter_results(cx);
        cx.notify();
    }

    fn collect_files(root: &Path, include_ignored: bool) -> Vec<PathBuf> {
        let mut builder = ignore::WalkBuilder::new(root);
        builder.git_ignore(!include_ignored);
        builder.git_global(!include_ignored);
        builder.git_exclude(!include_ignored);
        builder.ignore(!include_ignored);
        builder.hidden(false);
        builder.require_git(false);
        builder.follow_links(false);

        let mut files = Vec::new();
        for entry in builder.build().flatten() {
            if entry.file_type().map_or(false, |ft| ft.is_file()) {
                files.push(entry.into_path());
            }
        }
        files.sort_by(|a, b| {
            a.to_string_lossy()
                .to_lowercase()
                .cmp(&b.to_string_lossy().to_lowercase())
        });
        files
    }

    fn filter_results(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value();
        if query.is_empty() {
            self.filtered_indices = (0..self.all_files.len()).collect();
            self.selected_ix = 0;
            cx.notify();
            return;
        }

        let mut matcher = nucleo::Matcher::new(nucleo::Config::DEFAULT);
        let pattern = nucleo::pattern::Pattern::new(
            &query,
            nucleo::pattern::CaseMatching::Ignore,
            nucleo::pattern::Normalization::Smart,
            nucleo::pattern::AtomKind::Fuzzy,
        );

        let mut scored: Vec<(usize, Option<u32>)> = self
            .all_files
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let relative = path
                    .strip_prefix(&self.root)
                    .unwrap_or(path)
                    .to_string_lossy();
                let file_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&relative);

                // Match against filename first, boost score
                let mut buf = Vec::new();
                let haystack = nucleo::Utf32Str::new(file_name, &mut buf);
                let file_score = pattern.score(haystack, &mut matcher);

                match file_score {
                    Some(score) => (i, Some(score.saturating_add(100))),
                    None => {
                        // Try matching against the full relative path
                        let mut buf = Vec::new();
                        let haystack = nucleo::Utf32Str::new(&relative, &mut buf);
                        let path_score = pattern.score(haystack, &mut matcher);
                        (i, path_score)
                    }
                }
            })
            .collect();

        scored.retain(|(_, score)| score.is_some());
        scored.sort_by(|a, b| b.1.unwrap_or(0).cmp(&a.1.unwrap_or(0)));

        self.filtered_indices = scored.into_iter().map(|(i, _)| i).collect();
        self.selected_ix = 0;
        cx.notify();
    }

    fn select_previous(&mut self, cx: &mut Context<Self>) {
        if self.filtered_indices.is_empty() {
            return;
        }
        if self.selected_ix == 0 {
            self.selected_ix = self.filtered_indices.len().saturating_sub(1);
        } else {
            self.selected_ix = self.selected_ix.saturating_sub(1);
        }
        cx.notify();
    }

    fn select_next(&mut self, cx: &mut Context<Self>) {
        if self.filtered_indices.is_empty() {
            return;
        }
        self.selected_ix = (self.selected_ix + 1).min(self.filtered_indices.len().saturating_sub(1));
        cx.notify();
    }

    fn confirm_selection(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let path = if self.input.read(cx).value().is_empty() {
            // When query is empty, select from recent files if available
            let existing_recent: Vec<&PathBuf> = self
                .recent_files
                .iter()
                .filter(|p| p.exists())
                .collect();
            if let Some(p) = existing_recent.get(self.selected_ix) {
                Some((**p).clone())
            } else {
                self.filtered_indices
                    .get(self.selected_ix)
                    .and_then(|&i| self.all_files.get(i).cloned())
            }
        } else {
            self.filtered_indices
                .get(self.selected_ix)
                .and_then(|&i| self.all_files.get(i).cloned())
        };

        if let Some(path) = path {
            self.add_recent(path.clone(), cx);
            cx.emit(FileSearchEvent::OpenFile(path));
        }
        self.close(_window, cx);
    }

    fn relative_path(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string()
    }
}

impl Render for FileSearch {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }

        let results = &self.filtered_indices;
        let query_is_empty = self.input.read(cx).value().is_empty();

        let recent_entries: Vec<(usize, PathBuf)> = self
            .recent_files
            .iter()
            .enumerate()
            .filter(|(_, p)| p.exists())
            .take(20)
            .map(|(i, p)| (i, p.clone()))
            .collect();

        let show_recent_section = query_is_empty && !recent_entries.is_empty();

        v_flex()
            .id("file-search")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(FileSearch::on_action_toggle))
            .on_action(cx.listener(FileSearch::on_action_close))
            .on_action(cx.listener(FileSearch::on_action_confirm))
            .on_action(cx.listener(FileSearch::on_action_select_previous))
            .on_action(cx.listener(FileSearch::on_action_select_next))
            .w(px(560.))
            .max_h(px(420.))
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
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .flex_none()
                                    .child(">"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .child(Input::new(&self.input)),
                            ),
                    ),
            )
            .child(
                // Scrollable results
                div()
                    .flex_1()
                    .overflow_y_scrollbar()
                    .children(
                        // Recent files section
                        if show_recent_section {
                            let mut children: Vec<gpui::AnyElement> = Vec::new();

                            children.push(
                                div()
                                    .px_3()
                                    .pt_1p5()
                                    .pb_0p5()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Recent Files")
                                    .into_any_element(),
                            );

                            for (ix, path) in &recent_entries {
                                let selected = *ix == self.selected_ix;
                                children.push(
                                    self.render_file_row(
                                        path,
                                        *ix,
                                        selected,
                                        cx,
                                    ),
                                );
                            }
                            children
                        } else {
                            let mut children: Vec<gpui::AnyElement> = Vec::new();

                            let entries: Vec<(usize, PathBuf)> = results
                                .iter()
                                .enumerate()
                                .filter_map(|(ix, &file_ix)| {
                                    self.all_files
                                        .get(file_ix)
                                        .map(|p| (ix, p.clone()))
                                })
                                .collect();

                            if entries.is_empty() && !query_is_empty {
                                children.push(
                                    div()
                                        .px_3()
                                        .py_6()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("No matching files found")
                                        .into_any_element(),
                                );
                            } else {
                                for (ix, path) in &entries {
                                    let selected = *ix == self.selected_ix;
                                    children.push(
                                        self.render_file_row(
                                            path,
                                            *ix,
                                            selected,
                                            cx,
                                        ),
                                    );
                                }
                            }

                            children
                        },
                    ),
            )
            .into_any_element()
    }
}

impl FileSearch {
    fn render_file_row(
        &self,
        path: &Path,
        ix: usize,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let relative = self.relative_path(path);
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&relative)
            .to_string();

        let dir_path = path
            .parent()
            .and_then(|p| p.strip_prefix(&self.root).ok())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        div()
            .id(format!("file-search-row-{}", ix))
            .w_full()
            .px_3()
            .py_1p5()
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
                h_flex()
                    .w_full()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .flex_1()
                            .truncate()
                            .font_weight(if selected {
                                gpui::FontWeight::MEDIUM
                            } else {
                                gpui::FontWeight::NORMAL
                            })
                            .child(file_name.clone()),
                    )
                    .when(!dir_path.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .truncate()
                                .flex_none()
                                .text_color(if selected {
                                    cx.theme().accent_foreground.opacity(0.6)
                                } else {
                                    cx.theme().muted_foreground
                                })
                                .child(format!(" {}", dir_path)),
                        )
                    }),
            )
            .on_click({
                cx.listener(move |this: &mut FileSearch, _, window, cx| {
                    this.selected_ix = ix;
                    this.confirm_selection(window, cx);
                })
            })
            .into_any_element()
    }
}

impl Focusable for FileSearch {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// Action handlers
impl FileSearch {
    fn on_action_toggle(
        &mut self,
        _: &ToggleFileSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle(window, cx);
    }

    fn on_action_close(
        &mut self,
        _: &CloseFileSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close(window, cx);
    }

    fn on_action_confirm(
        &mut self,
        _: &ConfirmFileSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm_selection(window, cx);
    }

    fn on_action_select_previous(
        &mut self,
        _: &SelectPreviousFile,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_previous(cx);
    }

    fn on_action_select_next(
        &mut self,
        _: &SelectNextFile,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_next(cx);
    }
}
