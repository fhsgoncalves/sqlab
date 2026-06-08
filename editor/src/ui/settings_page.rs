use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, IconName, Selectable, Sizable, StyledExt, ThemeRegistry,
    button::{Button, ButtonVariants as _},
    dock::{Panel, PanelControl, PanelEvent, PanelState},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem},
    v_flex,
};

use crate::{
    app_settings::{self, AppSettings, display_path, gpui_keystroke_to_setting},
    app_theme,
    keymap::{SHORTCUTS, ShortcutDefinition},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsSection {
    Keymap,
    Themes,
}

pub struct SettingsPage {
    focus_handle: FocusHandle,
    section: SettingsSection,
    search_input: Entity<InputState>,
    settings: AppSettings,
    recording_action: Option<String>,
    intercept_subscription: Option<Subscription>,
    _search_subscription: Subscription,
}

impl EventEmitter<PanelEvent> for SettingsPage {}

impl SettingsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search actions or settings..."));
        let search_subscription = cx.subscribe_in(
            &search_input,
            window,
            |_: &mut Self, _, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            },
        );

        Self {
            focus_handle: cx.focus_handle(),
            section: SettingsSection::Keymap,
            search_input,
            settings: AppSettings::load(),
            recording_action: None,
            intercept_subscription: None,
            _search_subscription: search_subscription,
        }
    }

    fn select_section(&mut self, section: SettingsSection, cx: &mut Context<Self>) {
        self.section = section;
        cx.notify();
    }

    fn start_recording(&mut self, action_name: String, cx: &mut Context<Self>) {
        self.recording_action = Some(action_name);
        self.intercept_subscription = None;

        let settings_page = cx.entity().downgrade();
        self.intercept_subscription = Some(cx.intercept_keystrokes(move |event, _window, cx| {
            cx.stop_propagation();
            let keystroke = event.keystroke.unparse();
            let _ = settings_page.update(cx, |settings_page, cx| {
                settings_page.finish_recording(keystroke.clone(), cx);
            });
        }));
        cx.notify();
    }

    fn finish_recording(&mut self, keystroke: String, cx: &mut Context<Self>) {
        if keystroke == "escape" {
            self.recording_action = None;
            self.intercept_subscription = None;
            cx.notify();
            return;
        }

        if let Some(action_name) = self.recording_action.take() {
            let previous = shortcut_definition(&action_name).map(|definition| {
                (
                    definition.effective_keystroke(&self.settings),
                    definition.effective_context(&self.settings),
                )
            });
            self.settings.set_keymap_value(&action_name, &keystroke);
            self.settings.save();
            if let (Some(definition), Some((previous_keystroke, previous_context))) =
                (shortcut_definition(&action_name), previous)
            {
                definition.rebind(
                    &previous_keystroke,
                    previous_context.as_deref(),
                    &self.settings,
                    cx,
                );
            }
        }

        self.intercept_subscription = None;
        cx.notify();
    }

    fn reset_shortcut(&mut self, action_name: &str, cx: &mut Context<Self>) {
        let Some(definition) = shortcut_definition(action_name) else {
            return;
        };
        let previous_keystroke = definition.effective_keystroke(&self.settings);
        let previous_context = definition.effective_context(&self.settings);

        self.settings.keymap.remove(action_name);
        self.settings.keymap_context.remove(action_name);
        self.settings.save();

        definition.rebind(
            &previous_keystroke,
            previous_context.as_deref(),
            &self.settings,
            cx,
        );
        cx.notify();
    }

    fn reset_all_shortcuts(&mut self, cx: &mut Context<Self>) {
        let previous = SHORTCUTS
            .iter()
            .filter(|definition| self.shortcut_is_custom(definition))
            .map(|definition| {
                (
                    definition,
                    definition.effective_keystroke(&self.settings),
                    definition.effective_context(&self.settings),
                )
            })
            .collect::<Vec<_>>();

        self.settings.keymap.clear();
        self.settings.keymap_context.clear();
        self.settings.save();

        for (definition, previous_keystroke, previous_context) in previous {
            definition.rebind(
                &previous_keystroke,
                previous_context.as_deref(),
                &self.settings,
                cx,
            );
        }
        cx.notify();
    }

    fn shortcut_is_custom(&self, definition: &ShortcutDefinition) -> bool {
        self.settings.keymap.contains_key(definition.name)
            || self.settings.keymap_context.contains_key(definition.name)
    }

    fn render_nav_item(
        &self,
        label: &'static str,
        section: SettingsSection,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.section == section;
        Button::new(format!("settings-nav-{label}"))
            .label(label)
            .icon(match section {
                SettingsSection::Keymap => IconName::Settings,
                SettingsSection::Themes => IconName::Palette,
            })
            .small()
            .ghost()
            .w_full()
            .justify_start()
            .when(selected, |button| button.selected(true))
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.select_section(section, cx);
            }))
    }

    fn render_themes(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let current_theme_name = cx.theme().theme_name().to_string();
        let themes = ThemeRegistry::global(cx)
            .sorted_themes()
            .into_iter()
            .map(|theme| theme.name.clone())
            .collect::<Vec<_>>();

        v_flex()
            .size_full()
            .gap_4()
            .child(section_header("Appearance", "Look & Feel"))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .py_4()
                    .border_t_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_lg().font_semibold().child("Theme"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Choose the active application theme."),
                            ),
                    )
                    .child(
                        Button::new("settings-theme-dropdown")
                            .label(current_theme_name.clone())
                            .small()
                            .dropdown_caret(true)
                            .dropdown_menu(move |menu, _window, _cx| {
                                let mut menu = menu;
                                for theme_name in &themes {
                                    let theme_name = theme_name.clone();
                                    let checked = theme_name.as_ref() == current_theme_name;
                                    menu = menu.item(
                                        PopupMenuItem::new(theme_name.clone())
                                            .checked(checked)
                                            .on_click({
                                                let theme_name = theme_name.clone();
                                                move |_, window, cx| {
                                                    let theme_name = theme_name.to_string();
                                                    if app_theme::apply_theme_by_name(
                                                        &theme_name,
                                                        Some(window),
                                                        cx,
                                                    ) {
                                                        app_theme::persist_selected_theme(
                                                            &theme_name,
                                                        );
                                                    }
                                                }
                                            }),
                                    );
                                }
                                menu
                            }),
                    ),
            )
    }

    fn render_keymap(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = self.settings.clone();
        let recording_action = self.recording_action.clone();
        let settings_page = cx.entity().downgrade();
        let query = self
            .search_input
            .read(cx)
            .value()
            .trim()
            .to_ascii_lowercase();
        let visible_shortcuts = SHORTCUTS
            .iter()
            .filter(|definition| shortcut_matches_query(definition, &query))
            .collect::<Vec<_>>();
        let is_empty = visible_shortcuts.is_empty();

        v_flex()
            .size_full()
            .gap_3()
            .child(
                h_flex()
                    .items_end()
                    .justify_between()
                    .child(section_header("Keymap", "Shortcuts"))
                    .child(
                        Button::new("reset-all-shortcuts")
                            .label("Reset All")
                            .small()
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.reset_all_shortcuts(cx);
                            })),
                    ),
            )
            .child(Input::new(&self.search_input))
            .child(
                h_flex()
                    .h(px(32.))
                    .items_center()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().w(px(520.)).child("Action"))
                    .child(div().w(px(180.)).child("Keystroke"))
                    .child(div().w(px(180.)).child("Context"))
                    .child(div().w(px(90.)).child("Source")),
            )
            .child(
                v_flex()
                    .id("settings-keymap-shortcuts-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .when(is_empty, |list| {
                        list.child(
                            div()
                                .h(px(96.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No shortcuts found."),
                        )
                    })
                    .children(visible_shortcuts.into_iter().map(|definition| {
                        let action_name = definition.name.to_string();
                        let is_recording = recording_action.as_deref() == Some(definition.name);
                        let effective = definition.effective_keystroke(&settings);
                        let source = if settings.keymap.contains_key(definition.name)
                            || settings.keymap_context.contains_key(definition.name)
                        {
                            "User"
                        } else {
                            "Default"
                        };
                        let context = definition
                            .effective_context(&settings)
                            .unwrap_or_else(|| "Global".to_string());
                        let keystroke_label = if is_recording {
                            "Type shortcut...".to_string()
                        } else {
                            gpui_keystroke_to_setting(&effective)
                        };
                        let reset_action_name = definition.name.to_string();
                        let settings_page_for_menu = settings_page.clone();
                        h_flex()
                            .id(format!("settings-keymap-row-{}", definition.name))
                            .min_h(px(38.))
                            .items_center()
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.55))
                            .text_sm()
                            .child(
                                div().w(px(520.)).pr_3().child(
                                    Button::new(format!(
                                        "settings-keymap-action-{}",
                                        definition.name
                                    ))
                                    .label(definition.label)
                                    .small()
                                    .ghost()
                                    .w_full()
                                    .justify_start()
                                    .tooltip(definition.name),
                                ),
                            )
                            .child(
                                div().w(px(180.)).child(
                                    Button::new(format!("record-shortcut-{}", definition.name))
                                        .label(keystroke_label)
                                        .xsmall()
                                        .when(is_recording, |button| button.selected(true))
                                        .on_click(cx.listener(move |this, _, _window, cx| {
                                            this.start_recording(action_name.clone(), cx);
                                        }))
                                        .context_menu(move |menu, _window, _cx| {
                                            let reset_action_name = reset_action_name.clone();
                                            let settings_page = settings_page_for_menu.clone();
                                            menu.item(
                                                PopupMenuItem::new("Reset to Default").on_click(
                                                    move |_, _window, cx| {
                                                        let _ =
                                                            settings_page.update(cx, |this, cx| {
                                                                this.reset_shortcut(
                                                                    &reset_action_name,
                                                                    cx,
                                                                );
                                                            });
                                                    },
                                                ),
                                            )
                                        }),
                                ),
                            )
                            .child(
                                div()
                                    .w(px(180.))
                                    .pl_3()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_color(cx.theme().muted_foreground)
                                    .child(context),
                            )
                            .child(div().w(px(90.)).child(source))
                    })),
            )
    }
}

impl Panel for SettingsPage {
    fn panel_name(&self) -> &'static str {
        "SettingsPage"
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some("Settings".into())
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        "Settings"
    }

    fn closable(&self, _cx: &App) -> bool {
        true
    }

    fn zoomable(&self, _cx: &App) -> Option<PanelControl> {
        None
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState::new(self)
    }
}

impl Focusable for SettingsPage {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .id("settings-page")
            .size_full()
            .track_focus(&self.focus_handle)
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                v_flex()
                    .w(px(220.))
                    .h_full()
                    .gap_2()
                    .p_3()
                    .border_r_1()
                    .border_color(cx.theme().border)
                    .child(self.render_nav_item("Keymap", SettingsSection::Keymap, cx))
                    .child(self.render_nav_item("Appearance", SettingsSection::Themes, cx))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(display_path(&app_settings::settings_path())),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .h_full()
                    .p_6()
                    .overflow_hidden()
                    .map(|content| match self.section {
                        SettingsSection::Keymap => content.child(self.render_keymap(cx)),
                        SettingsSection::Themes => content.child(self.render_themes(cx)),
                    }),
            )
    }
}

fn section_header(title: &'static str, subtitle: &'static str) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_2xl().font_semibold().child(title))
        .child(div().text_sm().child(subtitle))
}

fn shortcut_definition(action_name: &str) -> Option<&'static ShortcutDefinition> {
    SHORTCUTS
        .iter()
        .find(|definition| definition.name == action_name)
}

fn shortcut_matches_query(definition: &ShortcutDefinition, query: &str) -> bool {
    query.is_empty()
        || definition.name.contains(query)
        || definition.label.to_ascii_lowercase().contains(query)
}
