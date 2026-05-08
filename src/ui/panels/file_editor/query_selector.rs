use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, StatefulInteractiveElement, Styled, Window, actions, div,
    prelude::FluentBuilder, px,
};
use gpui_component::{ActiveTheme, h_flex, v_flex};

actions!(
    query_selector,
    [SelectPreviousQuery, SelectNextQuery, ConfirmSelectedQuery]
);

#[derive(Clone, Debug)]
pub struct QuerySelected {
    pub query: String,
}

pub struct QuerySelector {
    queries: Vec<String>,
    selected_ix: usize,
    focus_handle: FocusHandle,
}

impl EventEmitter<QuerySelected> for QuerySelector {}

impl QuerySelector {
    pub fn new(queries: Vec<String>, cx: &mut Context<Self>) -> Self {
        Self {
            queries,
            selected_ix: 0,
            focus_handle: cx.focus_handle(),
        }
    }

    fn select_previous(
        &mut self,
        _: &SelectPreviousQuery,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.queries.is_empty() {
            return;
        }
        self.selected_ix = if self.selected_ix == 0 {
            self.queries.len() - 1
        } else {
            self.selected_ix - 1
        };
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNextQuery, _window: &mut Window, cx: &mut Context<Self>) {
        if self.queries.is_empty() {
            return;
        }
        self.selected_ix = (self.selected_ix + 1) % self.queries.len();
        cx.notify();
    }

    fn confirm_selected(
        &mut self,
        _: &ConfirmSelectedQuery,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.emit_selected(self.selected_ix, cx);
    }

    fn emit_selected(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(query) = self.queries.get(ix).cloned() else {
            return;
        };
        cx.emit(QuerySelected { query });
    }
}

impl Render for QuerySelector {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("query-selector")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::confirm_selected))
            .w(px(620.))
            .gap_2()
            .children(self.queries.iter().enumerate().map(|(ix, query)| {
                let selected = ix == self.selected_ix;
                let preview = truncate_query(query, 140);

                h_flex()
                    .id(format!("query-selector-row-{}", ix))
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .when(selected, |this| this.border_2())
                    .border_color(if selected {
                        cx.theme().accent
                    } else {
                        cx.theme().border
                    })
                    .bg(if selected {
                        cx.theme().accent
                    } else {
                        cx.theme().background
                    })
                    .text_color(if selected {
                        cx.theme().accent_foreground
                    } else {
                        cx.theme().foreground
                    })
                    .when(!selected, |this| {
                        this.hover(|style| style.bg(cx.theme().accent.opacity(0.12)))
                    })
                    .child(
                        div()
                            .w(px(24.))
                            .text_sm()
                            .font_weight(if selected {
                                gpui::FontWeight::BOLD
                            } else {
                                gpui::FontWeight::NORMAL
                            })
                            .text_color(if selected {
                                cx.theme().accent_foreground
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(format!("{}", ix + 1)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(if selected {
                                gpui::FontWeight::MEDIUM
                            } else {
                                gpui::FontWeight::NORMAL
                            })
                            .child(preview),
                    )
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.selected_ix = ix;
                        this.emit_selected(ix, cx);
                    }))
            }))
    }
}

impl Focusable for QuerySelector {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn truncate_query(query: &str, max_chars: usize) -> String {
    let compact = query.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        compact
    } else {
        let mut truncated = compact
            .chars()
            .take(max_chars.saturating_sub(3))
            .collect::<String>();
        truncated.push_str("...");
        truncated
    }
}
