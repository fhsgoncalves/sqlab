use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyBinding, KeyDownEvent, ParentElement, Render, ScrollHandle,
    StatefulInteractiveElement, Styled, Window, actions, div, prelude::FluentBuilder, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};

actions!(keymap, [ToggleKeymap, CloseKeymap]);

const CONTEXT: &str = "KeymapPanel";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", CloseKeymap, Some(CONTEXT))]);
}

pub struct KeymapDescriptor {
    pub label: &'static str,
    pub category: &'static str,
    pub action_id: &'static str,
    pub default_key: &'static str,
    #[allow(dead_code)]
    pub context: Option<&'static str>,
}

pub const ALL_DESCRIPTORS: &[KeymapDescriptor] = &[
    // File
    KeymapDescriptor {
        label: "Open Recent Folder",
        category: "File",
        action_id: "open_recent_folders",
        default_key: "cmd-o",
        context: None,
    },
    KeymapDescriptor {
        label: "Open Folder...",
        category: "File",
        action_id: "open_folder",
        default_key: "cmd-shift-o",
        context: None,
    },
    KeymapDescriptor {
        label: "Save File",
        category: "File",
        action_id: "save_file",
        default_key: "cmd-s",
        context: Some("Input"),
    },
    // Editor
    KeymapDescriptor {
        label: "Execute Query",
        category: "Editor",
        action_id: "execute_query",
        default_key: "cmd-enter",
        context: Some("Input"),
    },
    KeymapDescriptor {
        label: "Format Query",
        category: "Editor",
        action_id: "format_query",
        default_key: "cmd-alt-l",
        context: Some("Input"),
    },
    KeymapDescriptor {
        label: "Find",
        category: "Editor",
        action_id: "toggle_editor_search",
        default_key: "cmd-f",
        context: Some("Input"),
    },
    KeymapDescriptor {
        label: "Find in Files",
        category: "Editor",
        action_id: "toggle_project_search",
        default_key: "cmd-shift-f",
        context: None,
    },
    KeymapDescriptor {
        label: "Replace",
        category: "Editor",
        action_id: "toggle_editor_replace",
        default_key: "cmd-shift-h",
        context: None,
    },
    KeymapDescriptor {
        label: "Toggle Comment",
        category: "Editor",
        action_id: "toggle_comment_lines",
        default_key: "cmd-/",
        context: Some("Input"),
    },
    KeymapDescriptor {
        label: "Indent Lines",
        category: "Editor",
        action_id: "indent_lines",
        default_key: "tab",
        context: Some("file_editor"),
    },
    KeymapDescriptor {
        label: "Outdent Lines",
        category: "Editor",
        action_id: "outdent_lines",
        default_key: "shift-tab",
        context: Some("file_editor"),
    },
    KeymapDescriptor {
        label: "Cut Line",
        category: "Editor",
        action_id: "cut_editor_line",
        default_key: "cmd-x",
        context: Some("file_editor"),
    },
    // Navigation
    KeymapDescriptor {
        label: "Go Back",
        category: "Navigation",
        action_id: "navigate_back",
        default_key: "cmd-[",
        context: None,
    },
    KeymapDescriptor {
        label: "Go Forward",
        category: "Navigation",
        action_id: "navigate_forward",
        default_key: "cmd-]",
        context: None,
    },
    KeymapDescriptor {
        label: "Search Files",
        category: "Navigation",
        action_id: "toggle_file_search",
        default_key: "cmd-e",
        context: None,
    },
    KeymapDescriptor {
        label: "Cycle Tab Forward",
        category: "Navigation",
        action_id: "cycle_tab_forward",
        default_key: "ctrl-tab",
        context: None,
    },
    KeymapDescriptor {
        label: "Cycle Tab Backward",
        category: "Navigation",
        action_id: "cycle_tab_backward",
        default_key: "ctrl-shift-tab",
        context: None,
    },
    KeymapDescriptor {
        label: "Close Active Tab",
        category: "Navigation",
        action_id: "close_active_tab",
        default_key: "cmd-w",
        context: None,
    },
    // Results
    KeymapDescriptor {
        label: "Copy Results",
        category: "Results",
        action_id: "copy_result_selection",
        default_key: "cmd-c",
        context: None,
    },
    KeymapDescriptor {
        label: "Edit Result Cell",
        category: "Results",
        action_id: "edit_result_cell",
        default_key: "enter",
        context: Some("DataTable"),
    },
    // Terminal
    KeymapDescriptor {
        label: "New Terminal Tab",
        category: "Terminal",
        action_id: "new_terminal_tab",
        default_key: "cmd-t",
        context: Some("terminal_panel"),
    },
    KeymapDescriptor {
        label: "Copy Terminal Selection",
        category: "Terminal",
        action_id: "copy_terminal_selection",
        default_key: "cmd-c",
        context: Some("terminal_panel"),
    },
    KeymapDescriptor {
        label: "Paste in Terminal",
        category: "Terminal",
        action_id: "paste_terminal",
        default_key: "cmd-v",
        context: Some("terminal_panel"),
    },
    // Panels
    KeymapDescriptor {
        label: "Toggle Left Panel",
        category: "Panels",
        action_id: "toggle_left_dock",
        default_key: "cmd-b",
        context: None,
    },
    KeymapDescriptor {
        label: "Toggle Right Panel",
        category: "Panels",
        action_id: "toggle_right_dock",
        default_key: "cmd-shift-b",
        context: None,
    },
    KeymapDescriptor {
        label: "Toggle Terminal",
        category: "Panels",
        action_id: "toggle_terminal",
        default_key: "cmd-shift-t",
        context: None,
    },
    KeymapDescriptor {
        label: "Toggle Results",
        category: "Panels",
        action_id: "toggle_results_panel",
        default_key: "cmd-shift-r",
        context: None,
    },
    KeymapDescriptor {
        label: "Toggle Bottom Panel",
        category: "Panels",
        action_id: "toggle_bottom_panel",
        default_key: "cmd-j",
        context: None,
    },
    KeymapDescriptor {
        label: "Keyboard Shortcuts",
        category: "Panels",
        action_id: "toggle_keymap",
        default_key: "cmd-,",
        context: None,
    },
];

pub type CustomKeymap = HashMap<String, String>;

fn keymap_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sqlab")
        .join("keymap.json")
}

pub fn load_custom_keymap() -> CustomKeymap {
    let path = keymap_path();
    let Ok(content) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn save_custom_keymap(keymap: &CustomKeymap) {
    let path = keymap_path();
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!("failed to create keymap directory: {}", e);
        return;
    }
    match serde_json::to_string_pretty(keymap) {
        Ok(content) => {
            if let Err(e) = std::fs::write(&path, content) {
                eprintln!("failed to save keymap: {}", e);
            }
        }
        Err(e) => eprintln!("failed to serialize keymap: {}", e),
    }
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
    filtered_indices: Vec<usize>,
    custom_keymap: CustomKeymap,
    visible: bool,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    _search_subscription: gpui::Subscription,
}

pub enum KeymapPanelEvent {
    Closed,
    KeymapChanged(CustomKeymap),
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
        let filtered_indices = (0..ALL_DESCRIPTORS.len()).collect();

        Self {
            search_input,
            recording_ix: None,
            filtered_indices,
            custom_keymap,
            visible: false,
            focus_handle,
            scroll_handle: ScrollHandle::default(),
            _search_subscription: search_subscription,
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
        self.custom_keymap = load_custom_keymap();
        self.recording_ix = None;
        self.filter_results(cx);
        cx.notify();
        window.focus(&self.search_input.read(cx).focus_handle(cx), cx);
    }

    pub fn close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.recording_ix = None;
        cx.emit(KeymapPanelEvent::Closed);
        cx.notify();
    }

    fn filter_results(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value().to_lowercase();
        if query.is_empty() {
            self.filtered_indices = (0..ALL_DESCRIPTORS.len()).collect();
        } else {
            self.filtered_indices = ALL_DESCRIPTORS
                .iter()
                .enumerate()
                .filter(|(_, d)| {
                    d.label.to_lowercase().contains(&query)
                        || d.category.to_lowercase().contains(&query)
                        || d.default_key.to_lowercase().contains(&query)
                })
                .map(|(i, _)| i)
                .collect();
        }
        self.recording_ix = None;
        cx.notify();
    }

    fn current_key(&self, descriptor: &KeymapDescriptor) -> String {
        self.custom_keymap
            .get(descriptor.action_id)
            .cloned()
            .unwrap_or_else(|| descriptor.default_key.to_string())
    }

    fn is_customized(&self, descriptor: &KeymapDescriptor) -> bool {
        self.custom_keymap.contains_key(descriptor.action_id)
    }

    fn start_recording(&mut self, list_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(&descriptor_ix) = self.filtered_indices.get(list_ix) else {
            return;
        };
        let Some(_) = ALL_DESCRIPTORS.get(descriptor_ix) else {
            return;
        };
        self.recording_ix = Some(list_ix);
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
        // Pure Escape → cancel via CloseKeymap action.
        let has_modifier = event.keystroke.modifiers.control
            || event.keystroke.modifiers.alt
            || event.keystroke.modifiers.shift
            || event.keystroke.modifiers.platform;
        if key == "escape" && !has_modifier {
            return;
        }
        let binding_str = keystroke_to_binding_str(&event.keystroke);
        self.save_recorded_key(binding_str, window, cx);
    }

    fn save_recorded_key(
        &mut self,
        key_str: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

        if !key_str.is_empty() && key_str != descriptor.default_key {
            self.custom_keymap
                .insert(descriptor.action_id.to_string(), key_str);
        } else {
            self.custom_keymap.remove(descriptor.action_id);
        }

        save_custom_keymap(&self.custom_keymap);
        cx.emit(KeymapPanelEvent::KeymapChanged(self.custom_keymap.clone()));
        self.recording_ix = None;
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

    pub fn cancel_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.recording_ix = None;
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
        self.custom_keymap.remove(descriptor.action_id);
        save_custom_keymap(&self.custom_keymap);
        cx.emit(KeymapPanelEvent::KeymapChanged(self.custom_keymap.clone()));
        cx.notify();
    }
}

impl Render for KeymapPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }

        let filtered = self.filtered_indices.clone();

        v_flex()
            .id("keymap-panel")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_action_toggle))
            .on_action(cx.listener(Self::on_action_close))
            .when(self.recording_ix.is_some(), |el| {
                el.on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    this.handle_key_capture(event, window, cx);
                }))
            })
            .w(px(580.))
            .max_h(px(520.))
            .overflow_hidden()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .shadow_md()
            .child(
                v_flex()
                    .flex_shrink_0()
                    .px_3()
                    .py_2()
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
                                    .child("Keyboard Shortcuts"),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("keymap-close")
                                    .icon(IconName::Close)
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Close")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.close(window, cx);
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
                            .flex_1()
                            .track_scroll(&self.scroll_handle)
                            .overflow_y_scroll()
                            .children(self.render_grouped_rows(&filtered, cx)),
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
}

impl KeymapPanel {
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
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                if event.click_count() == 2 {
                    this.start_recording(list_ix, window, cx);
                }
            }))
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
                    PopupMenuItem::new("Remove shortcut")
                        .disabled(!is_customized)
                        .on_click(move |_, _window, cx| {
                            entity_r.update(cx, |this, cx| {
                                this.reset_to_default(list_ix, cx);
                            });
                        }),
                )
            })
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(descriptor.label),
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
    let parts: Vec<String> = key.split('-').map(|part| part.to_string()).collect();

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
