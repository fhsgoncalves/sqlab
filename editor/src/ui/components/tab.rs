use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext, ClickEvent, InteractiveElement, IntoElement, MouseButton,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, div, hsla,
    prelude::FluentBuilder, px,
};
use gpui_component::{ActiveTheme, Icon, IconName, h_flex};

#[derive(Clone)]
struct TabDragData(usize);

impl gpui::Render for TabDragData {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A Tab element for the [`TabBar`].
#[derive(IntoElement)]
pub struct Tab {
    id: Option<SharedString>,
    label: Option<SharedString>,
    icon: Option<Icon>,
    prefix: Option<AnyElement>,
    suffix: Option<AnyElement>,
    selected: bool,
    closable: bool,
    index: Option<usize>,
    on_click: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
    on_close: Option<Rc<dyn Fn(&mut Window, &mut App) + 'static>>,
    on_tab_drop: Option<Rc<dyn Fn(usize, usize, &mut Window, &mut App) + 'static>>,
}

impl Default for Tab {
    fn default() -> Self {
        Self {
            id: None,
            label: None,
            icon: None,
            prefix: None,
            suffix: None,
            selected: false,
            closable: false,
            index: None,
            on_click: None,
            on_close: None,
            on_tab_drop: None,
        }
    }
}

impl Tab {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(dead_code)]
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn closable(mut self, closable: bool) -> Self {
        self.closable = closable;
        self
    }

    pub fn on_click(
        mut self,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(on_click));
        self
    }

    pub fn on_close(mut self, on_close: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(on_close));
        self
    }

    pub fn index(mut self, index: usize) -> Self {
        self.index = Some(index);
        self
    }

    pub fn on_tab_drop(
        mut self,
        on_tab_drop: impl Fn(usize, usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_tab_drop = Some(Rc::new(on_tab_drop));
        self
    }
}

impl ParentElement for Tab {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        // Tab doesn't support children directly; label/icon are used instead
        let _ = elements;
    }
}

impl RenderOnce for Tab {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let active_accent = if cx.theme().is_dark() {
            hsla(0.74, 0.78, 0.58, 1.0)
        } else {
            hsla(0.74, 0.78, 0.46, 1.0)
        };

        let (bg, fg, border_color) = if self.selected {
            (
                cx.theme().tab_active,
                cx.theme().tab_active_foreground,
                cx.theme().border,
            )
        } else {
            (
                cx.theme().transparent,
                cx.theme().tab_foreground,
                cx.theme().transparent,
            )
        };

        let hover_bg = if self.selected {
            cx.theme().tab_active
        } else {
            cx.theme().tab_bar
        };

        let tab_index = self.index.unwrap_or(0);
        let on_tab_drop = self.on_tab_drop.clone();

        div()
            .id(self.id.clone().unwrap_or_else(|| SharedString::from("tab")))
            .group("tab-group")
            .flex()
            .items_center()
            .gap_0p5()
            .flex_shrink_0()
            .relative()
            .h(px(32.))
            .pl_3()
            .pr_1()
            .bg(bg)
            .border_b_1()
            .border_color(border_color)
            .text_color(fg)
            .text_sm()
            .cursor_pointer()
            .when(!self.selected, |this| {
                this.hover(|style| style.bg(hover_bg))
            })
            .when(self.selected, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .bg(active_accent),
                )
            })
            .when_some(self.prefix, |this, prefix| this.child(prefix))
            .child(
                h_flex()
                    .flex_1()
                    .gap_1()
                    .items_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when_some(self.icon, |this, icon| this.child(icon.size_4()))
                    .when_some(self.label, |this, label| this.child(label)),
            )
            .when_some(self.suffix, |this, suffix| this.child(suffix))
            .when(self.closable, |this| {
                this.child(
                    div()
                        .id(self
                            .id
                            .clone()
                            .map(|id| format!("{}-close", id))
                            .unwrap_or_else(|| String::from("tab-close")))
                        .flex_shrink_0()
                        .w(px(14.))
                        .h(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .invisible()
                        .group_hover("tab-group", |style| {
                            style
                                .visible()
                                .bg(cx.theme().muted.opacity(0.2))
                                .text_color(cx.theme().foreground)
                        })
                        .text_color(cx.theme().muted_foreground)
                        .child(Icon::new(IconName::Close).size_3())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .when_some(self.on_close.clone(), |this, on_close| {
                            this.on_click(move |_, window, cx| on_close(window, cx))
                        }),
                )
            })
            .when_some(self.on_click, |this, on_click| {
                this.on_click(move |event, window, cx| on_click(event, window, cx))
            })
            .when_some(on_tab_drop.clone(), |this, on_tab_drop| {
                this.on_drag(TabDragData(tab_index), move |dragged, _, _, cx| {
                    cx.new(|_| dragged.clone())
                })
                .on_drop(move |dragged: &TabDragData, window, cx| {
                    on_tab_drop(dragged.0, tab_index, window, cx);
                })
                .drag_over(|style, _: &TabDragData, _, cx| style.bg(cx.theme().accent.opacity(0.3)))
            })
    }
}

/// A TabBar container.
#[derive(IntoElement)]
pub struct TabBar {
    id: SharedString,
    tabs: Vec<Tab>,
    selected_index: usize,
    on_click: Option<Rc<dyn Fn(&usize, &mut Window, &mut App) + 'static>>,
    prefix: Option<AnyElement>,
    suffix: Option<AnyElement>,
    on_reorder: Option<Rc<dyn Fn(&(usize, usize), &mut Window, &mut App) + 'static>>,
}

impl TabBar {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            tabs: Vec::new(),
            selected_index: 0,
            on_click: None,
            prefix: None,
            suffix: None,
            on_reorder: None,
        }
    }

    pub fn child(mut self, tab: impl Into<Tab>) -> Self {
        self.tabs.push(tab.into());
        self
    }

    pub fn selected_index(mut self, index: usize) -> Self {
        self.selected_index = index;
        self
    }

    pub fn on_click(mut self, on_click: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(on_click));
        self
    }

    pub fn prefix(mut self, prefix: impl IntoElement) -> Self {
        self.prefix = Some(prefix.into_any_element());
        self
    }

    pub fn suffix(mut self, suffix: impl IntoElement) -> Self {
        self.suffix = Some(suffix.into_any_element());
        self
    }

    pub fn on_reorder(
        mut self,
        on_reorder: impl Fn(&(usize, usize), &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_reorder = Some(Rc::new(on_reorder));
        self
    }
}

impl ParentElement for TabBar {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        let _ = elements;
    }
}

impl RenderOnce for TabBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let on_reorder = self.on_reorder.clone();

        div()
            .id(self.id)
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(px(36.))
            .bg(cx.theme().tab_bar)
            .border_b_1()
            .border_color(cx.theme().border)
            .overflow_hidden()
            .when_some(self.prefix, |this, prefix| this.child(prefix))
            .children(self.tabs.into_iter().enumerate().map(|(ix, tab)| {
                let selected = ix == self.selected_index;
                let on_click = self.on_click.clone();
                let on_reorder = on_reorder.clone();
                tab.id(format!("tab-{}", ix))
                    .selected(selected)
                    .index(ix)
                    .on_click({
                        let on_click = on_click.clone();
                        move |_, window, cx| {
                            if let Some(on_click) = on_click.as_ref() {
                                on_click(&ix, window, cx);
                            }
                        }
                    })
                    .when_some(on_reorder, |this, on_reorder| {
                        this.on_tab_drop(move |from_ix, to_ix, window, cx| {
                            on_reorder(&(from_ix, to_ix), window, cx);
                        })
                    })
            }))
            .child(div().flex_1().h_full())
            .when_some(self.suffix, |this, suffix| this.child(suffix))
    }
}
