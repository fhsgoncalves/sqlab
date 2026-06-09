use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyBinding, KeyDownEvent, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Window, actions, div, prelude::FluentBuilder, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{ContextMenuExt, DropdownMenu as _, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::{ActiveTheme, IconName, Sizable, Theme, ThemeRegistry, h_flex, v_flex};

use crate::shortcuts::{
    CustomKeymap, ShortcutDefinition, display_key, load_custom_keymap, save_custom_keymap,
    settings_path,
};

actions!(keymap, [ToggleKeymap, CloseKeymap]);

const CONTEXT: &str = "KeymapPanel";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", CloseKeymap, Some(CONTEXT))]);
}

pub use crate::shortcuts::ALL_SHORTCUTS as ALL_DESCRIPTORS;
pub type KeymapDescriptor = ShortcutDefinition;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SettingsSection {
    Keymap,
    Themes,
}

fn is_modifier_key(key: &str) -> bool {
    matches!(
        key,
        "shift"
            | "control"
            | "ctrl"
            | "alt"
            | "option"
            | "command"
            | "cmd"
            | "meta"
            | "super"
            | "hyper"
            | "fn"
            | "caps_lock"
            | "capslock"
    )
}

fn keystroke_to_binding_str(k: &gpui::Keystroke) -> String {
    let mut s = String::new();
    if k.modifiers.control {
        s.push_str("ctrl-");
    }
    if k.modifiers.alt {
        s.push_str("alt-");
    }
    if k.modifiers.shift {
        s.push_str("shift-");
    }
    if k.modifiers.platform {
        s.push_str("cmd-");
    }
    s.push_str(&k.key);
    s
}

pub struct KeymapPanel {
    search_input: Entity<InputState>,
    /// List index whose remap modal is currently open.
    recording_ix: Option<usize>,
    recording_error: Option<String>,
    filtered_indices: Vec<usize>,
    custom_keymap: CustomKeymap,
    active_section: SettingsSection,
    visible: bool,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    _search_subscription: gpui::Subscription,
}

pub enum KeymapPanelEvent {
    Closed,
    KeymapChanged(CustomKeymap),
    RecordingStarted,
    RecordingStopped(CustomKeymap),
    ThemeChanged(String),
}

impl EventEmitter<KeymapPanelEvent> for KeymapPanel {}

impl KeymapPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search shortcuts..."));

        let search_subscription = cx.subscribe_in(
            &search_input,
            window,
            |this: &mut KeymapPanel, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.filter_results(cx);
                }
            },
        );

        let custom_keymap = load_custom_keymap();
        let filtered_indices = ALL_DESCRIPTORS
            .iter()
            .enumerate()
            .filter_map(|(ix, definition)| definition.visible.then_some(ix))
            .collect();

        Self {
            search_input,
            recording_ix: None,
            recording_error: None,
            filtered_indices,
            custom_keymap,
            active_section: SettingsSection::Keymap,
            visible: false,
            focus_handle,
            scroll_handle: ScrollHandle::default(),
            _search_subscription: search_subscription,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn is_recording(&self) -> bool {
        self.recording_ix.is_some()
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
        self.custom_keymap = load_custom_keymap();
        self.recording_ix = None;
        self.recording_error = None;
        self.filter_results(cx);
        cx.notify();
        window.focus(&self.search_input.read(cx).focus_handle(cx), cx);
    }

    pub fn close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        self.visible = false;
        let was_recording = self.recording_ix.is_some();
        self.recording_ix = None;
        self.recording_error = None;
        if was_recording {
            cx.emit(KeymapPanelEvent::RecordingStopped(
                self.custom_keymap.clone(),
            ));
        }
        cx.emit(KeymapPanelEvent::Closed);
        cx.notify();
    }

    fn filter_results(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value().to_lowercase();
        if query.is_empty() {
            self.filtered_indices = ALL_DESCRIPTORS
                .iter()
                .enumerate()
                .filter_map(|(ix, definition)| definition.visible.then_some(ix))
                .collect();
        } else {
            self.filtered_indices = ALL_DESCRIPTORS
                .iter()
                .enumerate()
                .filter(|(_, d)| {
                    d.visible
                        && (d.label.to_lowercase().contains(&query)
                            || d.category.to_lowercase().contains(&query)
                            || d.id.to_lowercase().contains(&query)
                            || d.default_key().to_lowercase().contains(&query)
                            || d.context
                                .is_some_and(|context| context.to_lowercase().contains(&query)))
                })
                .map(|(i, _)| i)
                .collect();
        }
        self.recording_ix = None;
        self.recording_error = None;
        cx.notify();
    }

    fn current_key(&self, descriptor: &KeymapDescriptor) -> String {
        self.custom_keymap.key_for_definition(descriptor)
    }

    fn is_customized(&self, descriptor: &KeymapDescriptor) -> bool {
        self.custom_keymap.has_custom_key(descriptor)
    }

    fn current_context(&self, descriptor: &KeymapDescriptor) -> String {
        self.custom_keymap
            .context_for_definition(descriptor)
            .unwrap_or_else(|| "global".to_string())
    }

    fn start_recording(&mut self, list_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(&descriptor_ix) = self.filtered_indices.get(list_ix) else {
            return;
        };
        let Some(_) = ALL_DESCRIPTORS.get(descriptor_ix) else {
            return;
        };
        self.recording_ix = Some(list_ix);
        self.recording_error = None;
        cx.emit(KeymapPanelEvent::RecordingStarted);
        cx.notify();
        window.focus(&self.focus_handle, cx);
    }

    fn handle_key_capture(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = &event.keystroke.key;
        if is_modifier_key(key) {
            return;
        }
        let has_modifier = event.keystroke.modifiers.control
            || event.keystroke.modifiers.alt
            || event.keystroke.modifiers.shift
            || event.keystroke.modifiers.platform;
        if key == "escape" && !has_modifier {
            self.cancel_recording(window, cx);
            return;
        }
        let binding_str = keystroke_to_binding_str(&event.keystroke);
        self.save_recorded_key(binding_str, window, cx);
    }

    fn save_recorded_key(&mut self, key_str: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(list_ix) = self.recording_ix else {
            return;
        };
        let Some(&descriptor_ix) = self.filtered_indices.get(list_ix) else {
            self.cancel_recording(window, cx);
            return;
        };
        let Some(descriptor) = ALL_DESCRIPTORS.get(descriptor_ix) else {
            self.cancel_recording(window, cx);
            return;
        };

        let normalized_key = crate::shortcuts::normalize_key(&key_str);
        if normalized_key != self.custom_keymap.key_for_definition(descriptor)
            && let Some(conflict) = self.conflicting_descriptor(descriptor, &normalized_key)
        {
            self.recording_error = Some(format!(
                "{} is already used by {}",
                display_key(&normalized_key),
                conflict.label
            ));
            cx.notify();
            window.focus(&self.focus_handle, cx);
            return;
        }

        self.custom_keymap.set_key(descriptor, key_str);

        save_custom_keymap(&self.custom_keymap);
        cx.emit(KeymapPanelEvent::KeymapChanged(self.custom_keymap.clone()));
        self.recording_ix = None;
        self.recording_error = None;
        cx.notify();
        window.focus(&self.search_input.read(cx).focus_handle(cx), cx);
    }

    pub fn filtered_indices(&self) -> &[usize] {
        &self.filtered_indices
    }

    pub fn recording_descriptor(&self) -> Option<(&KeymapDescriptor, String, bool)> {
        let list_ix = self.recording_ix?;
        let &descriptor_ix = self.filtered_indices.get(list_ix)?;
        let descriptor = ALL_DESCRIPTORS.get(descriptor_ix)?;
        let current_key = self.current_key(descriptor);
        let customized = self.is_customized(descriptor);
        Some((descriptor, current_key, customized))
    }

    pub fn recording_error(&self) -> Option<&str> {
        self.recording_error.as_deref()
    }

    pub fn cancel_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let was_recording = self.recording_ix.is_some();
        self.recording_ix = None;
        self.recording_error = None;
        if was_recording {
            cx.emit(KeymapPanelEvent::RecordingStopped(
                self.custom_keymap.clone(),
            ));
        }
        cx.notify();
        window.focus(&self.search_input.read(cx).focus_handle(cx), cx);
    }

    pub fn reset_to_default(&mut self, list_ix: usize, cx: &mut Context<Self>) {
        let Some(&descriptor_ix) = self.filtered_indices.get(list_ix) else {
            return;
        };
        let Some(descriptor) = ALL_DESCRIPTORS.get(descriptor_ix) else {
            return;
        };
        self.custom_keymap.reset_key(descriptor);
        save_custom_keymap(&self.custom_keymap);
        cx.emit(KeymapPanelEvent::KeymapChanged(self.custom_keymap.clone()));
        cx.notify();
    }

    fn reset_all_to_defaults(&mut self, cx: &mut Context<Self>) {
        self.recording_ix = None;
        self.recording_error = None;
        self.custom_keymap.reset_all();
        save_custom_keymap(&self.custom_keymap);
        cx.emit(KeymapPanelEvent::KeymapChanged(self.custom_keymap.clone()));
        cx.notify();
    }

    fn conflicting_descriptor(
        &self,
        target: &KeymapDescriptor,
        normalized_key: &str,
    ) -> Option<&'static KeymapDescriptor> {
        let target_context = self.custom_keymap.context_for_definition(target);

        ALL_DESCRIPTORS.iter().find(|descriptor| {
            descriptor.id != target.id
                && descriptor.visible
                && self.custom_keymap.key_for_definition(descriptor) == normalized_key
                && contexts_conflict(
                    target_context.as_deref(),
                    self.custom_keymap
                        .context_for_definition(descriptor)
                        .as_deref(),
                )
        })
    }
}

fn contexts_conflict(left: Option<&str>, right: Option<&str>) -> bool {
    left == right || left.is_none() || right.is_none()
}

fn display_settings_path() -> String {
    let path = settings_path();
    if let Some(home) = std::env::var_os("HOME")
        && let Ok(relative_path) = path.strip_prefix(home)
    {
        return format!("~/{}", relative_path.display());
    }
    path.display().to_string()
}

impl Render for KeymapPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }

        let filtered = self.filtered_indices.clone();
        let active_section = self.active_section;
        let content = match active_section {
            SettingsSection::Keymap => self.render_keymap_section(&filtered, cx),
            SettingsSection::Themes => self.render_themes_section(cx),
        };

        v_flex()
            .id("settings-panel")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_action_toggle))
            .on_action(cx.listener(Self::on_action_close))
            .when(self.recording_ix.is_some(), |el| {
                el.on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.handle_key_capture(event, window, cx);
                }))
            })
            .w(px(820.))
            .h(px(560.))
            .overflow_hidden()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .shadow_md()
            .child(
                h_flex()
                    .flex_shrink_0()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(cx.theme().foreground)
                            .child("Settings"),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("settings-close")
                            .icon(IconName::Close)
                            .xsmall()
                            .ghost()
                            .tooltip("Close")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close(window, cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .w(px(160.))
                            .h_full()
                            .flex_none()
                            .p_2()
                            .gap_1()
                            .border_r_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().tab_bar)
                            .child(self.render_section_button(
                                "settings-section-keymap",
                                "Keymap",
                                SettingsSection::Keymap,
                                active_section,
                                cx,
                            ))
                            .child(self.render_section_button(
                                "settings-section-themes",
                                "Appearance",
                                SettingsSection::Themes,
                                active_section,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .min_w(px(0.))
                            .min_h(px(0.))
                            .overflow_hidden()
                            .child(content),
                    ),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .h(px(32.))
                    .px_4()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().flex_1())
                    .child(display_settings_path()),
            )
            .into_any_element()
    }
}

impl KeymapPanel {
    fn render_section_button(
        &self,
        id: &'static str,
        label: &'static str,
        section: SettingsSection,
        active_section: SettingsSection,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        h_flex()
            .id(id)
            .h(px(32.))
            .px_2()
            .items_center()
            .rounded_md()
            .text_sm()
            .font_weight(if section == active_section {
                gpui::FontWeight::MEDIUM
            } else {
                gpui::FontWeight::NORMAL
            })
            .text_color(if section == active_section {
                cx.theme().accent_foreground
            } else {
                cx.theme().foreground
            })
            .when(section == active_section, |this| this.bg(cx.theme().accent))
            .when(section != active_section, |this| {
                this.hover(|style| style.bg(cx.theme().accent.opacity(0.08)))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.active_section = section;
                if this.recording_ix.is_some() {
                    this.cancel_recording(window, cx);
                }
                cx.notify();
                if section == SettingsSection::Keymap {
                    window.focus(&this.search_input.read(cx).focus_handle(cx), cx);
                } else {
                    window.focus(&this.focus_handle, cx);
                }
            }))
            .child(label)
            .into_any_element()
    }

    fn render_keymap_section(
        &self,
        filtered: &[usize],
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        v_flex()
            .w_full()
            .h_full()
            .overflow_hidden()
            .child(
                v_flex()
                    .flex_shrink_0()
                    .px_4()
                    .py_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(cx.theme().foreground)
                                    .child("Keymap"),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("keymap-reset-all")
                                    .label("Reset all")
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Reset all shortcuts to defaults")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.reset_all_to_defaults(cx);
                                    })),
                            ),
                    )
                    .child(Input::new(&self.search_input)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h(px(0.))
                    .relative()
                    .overflow_hidden()
                    .child(
                        div()
                            .id("keymap-scroll")
                            .size_full()
                            .track_scroll(&self.scroll_handle)
                            .overflow_y_scroll()
                            .children(self.render_grouped_rows(filtered, cx)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .child(Scrollbar::vertical(&self.scroll_handle)),
                    ),
            )
            .into_any_element()
    }

    fn render_themes_section(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let current_theme_name = Theme::global(cx).theme_name().clone();
        let theme_names = ThemeRegistry::global(cx)
            .sorted_themes()
            .into_iter()
            .map(|theme| theme.name.clone())
            .collect::<Vec<SharedString>>();
        let entity = cx.entity();

        v_flex()
            .w_full()
            .h_full()
            .px_4()
            .py_3()
            .items_start()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child("Theme"),
                    )
                    .child(
                        Button::new("settings-theme-dropdown")
                            .label(current_theme_name.to_string())
                            .w(px(260.))
                            .small()
                            .ghost()
                            .dropdown_menu(move |menu, _window, _cx| {
                                let mut menu = menu;
                                for theme_name in theme_names.clone() {
                                    let selected = theme_name == current_theme_name;
                                    let theme_name_for_click = theme_name.to_string();
                                    let entity = entity.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(theme_name).checked(selected).on_click(
                                            move |_, _window, cx| {
                                                entity.update(cx, |_, cx| {
                                                    cx.emit(KeymapPanelEvent::ThemeChanged(
                                                        theme_name_for_click.clone(),
                                                    ));
                                                    cx.notify();
                                                });
                                            },
                                        ),
                                    );
                                }
                                menu
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_grouped_rows(
        &self,
        filtered: &[usize],
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut children: Vec<gpui::AnyElement> = Vec::new();
        let mut last_category: Option<&'static str> = None;

        for (list_ix, &descriptor_ix) in filtered.iter().enumerate() {
            let Some(descriptor) = ALL_DESCRIPTORS.get(descriptor_ix) else {
                continue;
            };

            if last_category != Some(descriptor.category) {
                last_category = Some(descriptor.category);
                children.push(
                    div()
                        .px_3()
                        .pt_3()
                        .pb_1()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(cx.theme().muted_foreground)
                        .child(descriptor.category.to_uppercase())
                        .into_any_element(),
                );
            }

            children.push(self.render_binding_row(list_ix, descriptor, cx));
        }

        if children.is_empty() {
            children.push(
                div()
                    .px_3()
                    .py_6()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No matching shortcuts found")
                    .into_any_element(),
            );
        }

        children.push(div().h(px(8.)).into_any_element());
        children
    }

    fn render_binding_row(
        &self,
        list_ix: usize,
        descriptor: &KeymapDescriptor,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_customized = self.is_customized(descriptor);
        let current_key = self.current_key(descriptor);
        let current_context = self.current_context(descriptor);
        let is_active_recording = self.recording_ix == Some(list_ix);

        let entity = cx.entity();
        let entity_edit = entity.clone();
        let entity_remove = entity.clone();

        h_flex()
            .id(format!("keymap-row-{}", list_ix))
            .w_full()
            .px_3()
            .py_1p5()
            .gap_2()
            .items_center()
            .when(is_active_recording, |this| {
                this.bg(cx.theme().primary.opacity(0.06))
            })
            .when(!is_active_recording, |this| {
                this.hover(|style| style.bg(cx.theme().accent.opacity(0.08)))
            })
            // Double-click opens the remap modal.
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    if event.click_count() == 2 {
                        this.start_recording(list_ix, window, cx);
                    }
                }),
            )
            // Right-click / secondary click opens the context menu.
            .context_menu(move |menu, _window, _cx| {
                let entity_e = entity_edit.clone();
                let entity_r = entity_remove.clone();
                menu.item(
                    PopupMenuItem::new("Edit shortcut").on_click(move |_, window, cx| {
                        entity_e.update(cx, |this, cx| {
                            this.start_recording(list_ix, window, cx);
                        });
                    }),
                )
                .item(
                    PopupMenuItem::new("Reset shortcut")
                        .disabled(!is_customized)
                        .on_click(move |_, _window, cx| {
                            entity_r.update(cx, |this, cx| {
                                this.reset_to_default(list_ix, cx);
                            });
                        }),
                )
            })
            .child(
                v_flex()
                    .flex_1()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(descriptor.label),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(descriptor.id)
                            .child("·")
                            .child(current_context),
                    ),
            )
            .child(render_key_badge(&current_key, is_customized, cx))
            .into_any_element()
    }
}

fn render_key_badge(
    key: &str,
    is_customized: bool,
    cx: &mut Context<KeymapPanel>,
) -> gpui::AnyElement {
    let display = display_key(key);
    let parts: Vec<String> = display.split('+').map(|part| part.to_string()).collect();

    h_flex()
        .gap_0p5()
        .items_center()
        .children(parts.into_iter().map(|part| {
            div()
                .px_1()
                .py_0p5()
                .rounded_sm()
                .border_1()
                .border_color(if is_customized {
                    cx.theme().primary.opacity(0.6)
                } else {
                    cx.theme().border
                })
                .bg(cx.theme().secondary)
                .text_xs()
                .font_weight(if is_customized {
                    gpui::FontWeight::MEDIUM
                } else {
                    gpui::FontWeight::NORMAL
                })
                .text_color(if is_customized {
                    cx.theme().primary
                } else {
                    cx.theme().muted_foreground
                })
                .child(part)
        }))
        .into_any_element()
}

impl Focusable for KeymapPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl KeymapPanel {
    fn on_action_toggle(&mut self, _: &ToggleKeymap, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording_ix.is_some() {
            return;
        }
        self.toggle(window, cx);
    }

    fn on_action_close(&mut self, _: &CloseKeymap, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording_ix.is_some() {
            self.cancel_recording(window, cx);
        } else {
            self.close(window, cx);
        }
    }
}
