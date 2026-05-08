use std::collections::HashSet;
use std::path::PathBuf;

use gpui::{
    App, AppContext, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Window, actions, div, prelude::FluentBuilder,
};
use gpui_component::{
    ActiveTheme, Icon, IconName, WindowExt,
    dock::PanelControl,
    h_flex,
    input::{Input, InputState},
    menu::ContextMenuExt,
    tree::TreeItem,
    v_flex,
};

actions!(
    file_tree,
    [
        NewFile, NewFolder, RenameFile, DeleteFile, CutFile, PasteFile
    ]
);

#[derive(Clone, Debug)]
pub struct OpenFileEvent {
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct RootChangedEvent;

#[derive(Clone)]
struct DragPreview {
    label: SharedString,
}

impl Render for DragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(self.label.clone())
            .px_2()
            .py_1()
            .rounded_md()
            .bg(cx.theme().accent)
            .text_color(cx.theme().accent_foreground)
    }
}

pub struct FileTreePanel {
    focus_handle: FocusHandle,
    context_target: Option<PathBuf>,
    root: PathBuf,
    items: Vec<TreeItem>,
    folder_ids: HashSet<String>,
    cut_buffer: Option<PathBuf>,
    selected_id: Option<String>,
}

impl EventEmitter<OpenFileEvent> for FileTreePanel {}
impl EventEmitter<RootChangedEvent> for FileTreePanel {}
impl EventEmitter<gpui_component::dock::PanelEvent> for FileTreePanel {}

impl gpui_component::dock::Panel for FileTreePanel {
    fn panel_name(&self) -> &'static str {
        "FileTreePanel"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        ""
    }

    fn closable(&self, _cx: &App) -> bool {
        false
    }

    fn dump(&self, _cx: &App) -> gpui_component::dock::PanelState {
        gpui_component::dock::PanelState::new(self)
    }

    fn zoomable(&self, _cx: &App) -> Option<PanelControl> {
        None
    }
}

impl FileTreePanel {
    pub fn new(root: PathBuf, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (items, folder_ids) = build_file_items(&root, &HashSet::new());

        Self {
            focus_handle: cx.focus_handle(),
            context_target: None,
            root,
            items,
            folder_ids,
            cut_buffer: None,
            selected_id: None,
        }
    }

    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.root = root;
        self.refresh_tree(cx);
        cx.emit(RootChangedEvent);
    }

    fn refresh_tree(&mut self, cx: &mut Context<Self>) {
        let mut expanded_ids = HashSet::new();
        Self::collect_expanded_ids(&self.items, &mut expanded_ids);
        let (items, folder_ids) = build_file_items(&self.root, &expanded_ids);
        self.items = items;
        self.folder_ids = folder_ids;
        cx.notify();
    }

    fn collect_expanded_ids(items: &[TreeItem], ids: &mut HashSet<String>) {
        for item in items {
            if item.is_expanded() {
                ids.insert(item.id.to_string());
            }
            Self::collect_expanded_ids(&item.children, ids);
        }
    }

    fn flatten_items(items: &[TreeItem], result: &mut Vec<(TreeItem, usize)>, depth: usize) {
        for item in items {
            result.push((item.clone(), depth));
            if item.is_expanded() {
                Self::flatten_items(&item.children, result, depth + 1);
            }
        }
    }

    fn toggle_expanded(items: &mut Vec<TreeItem>, id: &str) -> bool {
        for item in items {
            if item.id == id {
                let expanded = !item.is_expanded();
                *item = item.clone().expanded(expanded);
                return true;
            }
            if Self::toggle_expanded(&mut item.children, id) {
                return true;
            }
        }
        false
    }

    fn parent_for(&self, path: &PathBuf) -> PathBuf {
        if path.is_dir() {
            path.clone()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        }
    }

    fn on_action_new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.context_target.clone() else {
            return;
        };
        let parent = self.parent_for(&target);
        let view = cx.entity();
        let input_state = cx.new(|cx| InputState::new(window, cx).placeholder("File name..."));
        let input_state_for_ok = input_state.clone();
        let input_state_for_focus = input_state.clone();
        let parent_for_ok = parent.clone();
        let view_for_ok = view.clone();

        window.open_alert_dialog(cx, move |alert, _window, _cx| {
            alert
                .title("New File")
                .child(Input::new(&input_state))
                .on_ok({
                    let input_state_for_ok = input_state_for_ok.clone();
                    let parent_for_ok = parent_for_ok.clone();
                    let view_for_ok = view_for_ok.clone();
                    move |_, _window, cx| {
                        let name = input_state_for_ok.read(cx).value();
                        if name.is_empty() {
                            return false;
                        }
                        let new_path = parent_for_ok.join(name.as_ref());
                        if let Err(e) = std::fs::File::create(&new_path) {
                            println!("Failed to create file: {}", e);
                        }
                        _ = view_for_ok.update(cx, |panel, cx| panel.refresh_tree(cx));
                        true
                    }
                })
        });
        window.focus(&input_state_for_focus.read(cx).focus_handle(cx), cx);
    }

    fn on_action_new_folder(&mut self, _: &NewFolder, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.context_target.clone() else {
            return;
        };
        let parent = self.parent_for(&target);
        let view = cx.entity();
        let input_state = cx.new(|cx| InputState::new(window, cx).placeholder("Folder name..."));
        let input_state_for_ok = input_state.clone();
        let input_state_for_focus = input_state.clone();
        let parent_for_ok = parent.clone();
        let view_for_ok = view.clone();

        window.open_alert_dialog(cx, move |alert, _window, _cx| {
            alert
                .title("New Folder")
                .child(Input::new(&input_state))
                .on_ok({
                    let input_state_for_ok = input_state_for_ok.clone();
                    let parent_for_ok = parent_for_ok.clone();
                    let view_for_ok = view_for_ok.clone();
                    move |_, _window, cx| {
                        let name = input_state_for_ok.read(cx).value();
                        if name.is_empty() {
                            return false;
                        }
                        let new_path = parent_for_ok.join(name.as_ref());
                        if let Err(e) = std::fs::create_dir(&new_path) {
                            println!("Failed to create folder: {}", e);
                        }
                        _ = view_for_ok.update(cx, |panel, cx| panel.refresh_tree(cx));
                        true
                    }
                })
        });
        window.focus(&input_state_for_focus.read(cx).focus_handle(cx), cx);
    }

    fn on_action_rename(&mut self, _: &RenameFile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.context_target.clone() else {
            return;
        };
        let current_name = target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let view = cx.entity();
        let input_state = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(current_name.clone())
                .placeholder("New name...")
        });
        let input_state_for_ok = input_state.clone();
        let current_name_for_ok = current_name.clone();
        let target_for_ok = target.clone();
        let view_for_ok = view.clone();

        window.open_alert_dialog(cx, move |alert, _window, _cx| {
            alert
                .title("Rename")
                .child(Input::new(&input_state))
                .on_ok({
                    let input_state_for_ok = input_state_for_ok.clone();
                    let current_name_for_ok = current_name_for_ok.clone();
                    let target_for_ok = target_for_ok.clone();
                    let view_for_ok = view_for_ok.clone();
                    move |_, _window, cx| {
                        let name = input_state_for_ok.read(cx).value();
                        if name.is_empty() || name == current_name_for_ok {
                            return true;
                        }
                        let new_path = target_for_ok
                            .parent()
                            .unwrap_or(&target_for_ok)
                            .join(name.as_ref());
                        if let Err(e) = std::fs::rename(&target_for_ok, &new_path) {
                            println!("Failed to rename: {}", e);
                        } else {
                            _ = view_for_ok.update(cx, |panel, _| {
                                if panel.root == target_for_ok {
                                    panel.root = new_path.clone();
                                }
                            });
                        }
                        _ = view_for_ok.update(cx, |panel, cx| panel.refresh_tree(cx));
                        true
                    }
                })
        });
    }

    fn on_action_delete(&mut self, _: &DeleteFile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.context_target.clone() else {
            return;
        };
        let name = target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("this item")
            .to_string();
        let view = cx.entity();
        let target_for_ok = target.clone();
        let view_for_ok = view.clone();

        window.open_alert_dialog(cx, move |alert, _window, _cx| {
            alert
                .title("Delete")
                .description(format!("Are you sure you want to delete '{}'?,", name))
                .confirm()
                .on_ok({
                    let target_for_ok = target_for_ok.clone();
                    let view_for_ok = view_for_ok.clone();
                    move |_, _window, cx| {
                        let result = if target_for_ok.is_dir() {
                            std::fs::remove_dir_all(&target_for_ok)
                        } else {
                            std::fs::remove_file(&target_for_ok)
                        };
                        if let Err(e) = result {
                            println!("Failed to delete: {}", e);
                        }
                        _ = view_for_ok.update(cx, |panel, cx| {
                            panel.cut_buffer = None;
                            panel.refresh_tree(cx)
                        });
                        true
                    }
                })
        });
    }

    fn on_action_cut_file(&mut self, _: &CutFile, _window: &mut Window, _cx: &mut Context<Self>) {
        if let Some(target) = self.context_target.clone() {
            self.cut_buffer = Some(target);
        }
    }

    fn on_action_paste_file(
        &mut self,
        _: &PasteFile,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self.cut_buffer.take() else {
            return;
        };
        let Some(target) = self.context_target.clone() else {
            return;
        };

        let dest_dir = self.parent_for(&target);
        let file_name = match source.file_name() {
            Some(name) => name.to_os_string(),
            None => return,
        };
        let dest = dest_dir.join(&file_name);

        if source.is_dir() {
            if dest.starts_with(&source) {
                println!("Cannot move a folder into itself");
                self.cut_buffer = Some(source);
                return;
            }
        }

        if source == dest {
            return;
        }

        if let Err(e) = std::fs::rename(&source, &dest) {
            println!("Failed to move: {}", e);
            self.cut_buffer = Some(source);
        } else {
            self.refresh_tree(cx);
        }
    }
}

fn build_dir_items(
    root: &PathBuf,
    path: &PathBuf,
    expanded_ids: &HashSet<String>,
    folder_ids: &mut HashSet<String>,
) -> Vec<TreeItem> {
    let mut items = Vec::new();

    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let path = entry.path();
            let relative_path = path.strip_prefix(root).unwrap_or(&path);
            if relative_path.ends_with(".git") || relative_path.ends_with("target") {
                continue;
            }
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown")
                .to_string();
            let id = path.to_string_lossy().to_string();
            let is_dir = path.is_dir();
            if is_dir {
                folder_ids.insert(id.clone());
                let children = build_dir_items(root, &path, expanded_ids, folder_ids);
                let mut item = TreeItem::new(id.clone(), file_name).children(children);
                if expanded_ids.contains(&id) {
                    item = item.expanded(true);
                }
                items.push((item, true));
            } else {
                items.push((TreeItem::new(id, file_name), false));
            }
        }
    }

    items
        .sort_by(|(a, a_is_dir), (b, b_is_dir)| b_is_dir.cmp(a_is_dir).then(a.label.cmp(&b.label)));
    items.into_iter().map(|(item, _)| item).collect()
}

fn build_file_items(
    root: &PathBuf,
    expanded_ids: &HashSet<String>,
) -> (Vec<TreeItem>, HashSet<String>) {
    let mut folder_ids = HashSet::new();
    let items = build_dir_items(root, root, expanded_ids, &mut folder_ids);
    let root_name = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_else(|| root.to_str().unwrap_or("Project"))
        .to_string();
    let root_id = root.to_string_lossy().to_string();
    folder_ids.insert(root_id.clone());
    let mut root_item = TreeItem::new(root_id.clone(), root_name)
        .children(items)
        .expanded(true);
    if expanded_ids.contains(&root_id) {
        root_item = root_item.expanded(true);
    }
    (vec![root_item], folder_ids)
}

impl Render for FileTreePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _view = cx.entity();
        let mut entries = Vec::new();
        Self::flatten_items(&self.items, &mut entries, 0);

        let focus_handle = self.focus_handle.clone();
        let can_paste = self.cut_buffer.is_some();

        v_flex()
            .id("file-tree-panel")
            .size_full()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_action_new_file))
            .on_action(cx.listener(Self::on_action_new_folder))
            .on_action(cx.listener(Self::on_action_rename))
            .on_action(cx.listener(Self::on_action_delete))
            .on_action(cx.listener(Self::on_action_cut_file))
            .on_action(cx.listener(Self::on_action_paste_file))
            .context_menu(move |menu, _window, _cx| {
                menu.action_context(focus_handle.clone())
                    .menu("New File", Box::new(NewFile))
                    .menu("New Folder", Box::new(NewFolder))
                    .separator()
                    .menu("Cut", Box::new(CutFile))
                    .menu_with_disabled("Paste", Box::new(PasteFile), !can_paste)
                    .separator()
                    .menu("Rename", Box::new(RenameFile))
                    .menu("Delete", Box::new(DeleteFile))
            })
            .child(
                v_flex()
                    .id("tree-inner")
                    .overflow_y_scroll()
                    .children(entries.into_iter().enumerate().map(|(ix, (item, depth))| {
                        let path = PathBuf::from(item.id.as_str());
                        let is_dir = self.folder_ids.contains(item.id.as_str());
                        let is_selected = self
                            .selected_id
                            .as_ref()
                            .map(|s| s == item.id.as_ref())
                            .unwrap_or(false);
                        let is_expanded = item.is_expanded();
                        let is_cut = self
                            .cut_buffer
                            .as_ref()
                            .map(|b| b.to_string_lossy() == item.id.as_str())
                            .unwrap_or(false);

                        let icon = if !is_dir {
                            let is_sql = path
                                .extension()
                                .and_then(|ext| ext.to_str())
                                .map(|ext| ext.eq_ignore_ascii_case("sql"))
                                .unwrap_or(false);
                            if is_sql {
                                Icon::new(IconName::File).path("icons/file-sql.svg")
                            } else {
                                Icon::new(IconName::File)
                            }
                        } else if is_expanded {
                            Icon::new(IconName::FolderOpen)
                        } else {
                            Icon::new(IconName::Folder)
                        };

                        let item_id_click = item.id.clone();
                        let item_id_right = item.id.clone();
                        let path_click = path.clone();
                        let path_right = path.clone();
                        let path_drag = path.clone();
                        let path_drop = path.clone();

                        div()
                            .id(ix)
                            .w_full()
                            .py_0p5()
                            .px_2()
                            .pl(gpui::px(16.) * depth as f32 + gpui::px(8.))
                            .rounded(cx.theme().radius)
                            .when(is_selected, |this| this.bg(cx.theme().accent.opacity(0.15)))
                            .when(is_cut, |this| this.text_color(cx.theme().muted_foreground))
                            .child(h_flex().gap_2().child(icon).child(item.label.clone()))
                            .on_click(cx.listener(move |panel, _, _, cx| {
                                if is_dir {
                                    panel.selected_id = Some(item_id_click.to_string());
                                    Self::toggle_expanded(&mut panel.items, &item_id_click);
                                    panel.refresh_tree(cx);
                                } else {
                                    panel.selected_id = Some(item_id_click.to_string());
                                    cx.emit(OpenFileEvent {
                                        path: path_click.clone(),
                                    });
                                    cx.notify();
                                }
                            }))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |panel, _, _, cx| {
                                    panel.context_target = Some(path_right.clone());
                                    panel.selected_id = Some(item_id_right.to_string());
                                    cx.notify();
                                }),
                            )
                            .on_drag(path_drag.clone(), move |path, _, _, cx| {
                                cx.new(|_| DragPreview {
                                    label: path.file_name().unwrap().to_string_lossy().into(),
                                })
                            })
                            .on_drop(cx.listener(move |panel, dragged_path: &PathBuf, _, cx| {
                                let dest_dir = if is_dir {
                                    path_drop.clone()
                                } else {
                                    path_drop.parent().unwrap_or(&path_drop).to_path_buf()
                                };
                                let file_name = match dragged_path.file_name() {
                                    Some(name) => name.to_os_string(),
                                    None => return,
                                };
                                let dest = dest_dir.join(&file_name);

                                if dragged_path == &dest {
                                    return;
                                }

                                if dragged_path.is_dir() && dest.starts_with(dragged_path) {
                                    println!("Cannot move a folder into itself");
                                    return;
                                }

                                if let Err(e) = std::fs::rename(dragged_path, &dest) {
                                    println!("Failed to move: {}", e);
                                } else {
                                    panel.refresh_tree(cx);
                                }
                            }))
                            .drag_over(|style, _path: &PathBuf, _, cx| {
                                style.bg(cx.theme().accent.opacity(0.3))
                            })
                    }))
                    .text_sm()
                    .p_1()
                    .bg(cx.theme().sidebar)
                    .text_color(cx.theme().sidebar_foreground)
                    .h_full(),
            )
    }
}

impl Focusable for FileTreePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
