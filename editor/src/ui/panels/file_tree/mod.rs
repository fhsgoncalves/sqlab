use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui::{
    App, AppContext, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, actions, div, hsla, prelude::FluentBuilder,
};
use gpui_component::{
    ActiveTheme, Icon, IconName, WindowExt,
    dock::PanelControl,
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::ContextMenuExt,
    tree::TreeItem,
    v_flex,
};

actions!(
    file_tree,
    [
        NewFile,
        NewFolder,
        RenameFile,
        DeleteFile,
        CutFile,
        PasteFile,
        OpenSelectedFile,
        SelectPreviousItem,
        SelectNextItem,
        ExpandSelectedItem,
        CollapseSelectedItem,
        CopySelectedName,
        CancelInlineEdit
    ]
);

const CONTEXT: &str = "FileTree";

#[derive(Clone, Debug)]
pub struct OpenFileEvent {
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct RootChangedEvent;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingNewKind {
    File,
    Folder,
}

#[derive(Clone)]
struct PendingNewEntry {
    id: String,
    parent: PathBuf,
    kind: PendingNewKind,
    input: Entity<InputState>,
}

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
    root: Option<PathBuf>,
    items: Vec<TreeItem>,
    folder_ids: HashSet<String>,
    cut_buffer: Option<PathBuf>,
    selected_id: Option<String>,
    renaming_id: Option<String>,
    rename_input: Option<Entity<InputState>>,
    pending_new: Option<PendingNewEntry>,
    active_editor_path: Option<String>,
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
    pub fn new(root: Option<PathBuf>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (items, folder_ids) = root
            .as_deref()
            .map(|root| build_file_items(root, &HashSet::new()))
            .unwrap_or_default();

        Self {
            focus_handle: cx.focus_handle(),
            context_target: None,
            root,
            items,
            folder_ids,
            cut_buffer: None,
            selected_id: None,
            renaming_id: None,
            rename_input: None,
            pending_new: None,
            active_editor_path: None,
        }
    }

    pub fn root(&self) -> Option<&PathBuf> {
        self.root.as_ref()
    }

    pub fn set_active_editor_path(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        self.active_editor_path = path.map(|path| path.to_string_lossy().to_string());
        cx.notify();
    }

    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.root = Some(root);
        self.refresh_tree(cx);
        cx.emit(RootChangedEvent);
    }

    fn refresh_tree(&mut self, cx: &mut Context<Self>) {
        let mut expanded_ids = HashSet::new();
        Self::collect_expanded_ids(&self.items, &mut expanded_ids);
        if let Some(root) = self.root.as_deref() {
            let (items, folder_ids) = build_file_items(root, &expanded_ids);
            self.items = items;
            self.folder_ids = folder_ids;
        } else {
            self.items.clear();
            self.folder_ids.clear();
        }
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

    fn set_expanded(items: &mut Vec<TreeItem>, id: &str, expanded: bool) -> bool {
        for item in items {
            if item.id == id {
                *item = item.clone().expanded(expanded);
                return true;
            }
            if Self::set_expanded(&mut item.children, id, expanded) {
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
        let Some(target) = self
            .selected_or_context_target()
            .or_else(|| self.root.clone())
        else {
            return;
        };
        let parent = self.parent_for(&target);
        self.start_inline_new(parent, PendingNewKind::File, window, cx);
    }

    fn on_action_new_folder(&mut self, _: &NewFolder, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self
            .selected_or_context_target()
            .or_else(|| self.root.clone())
        else {
            return;
        };
        let parent = self.parent_for(&target);
        self.start_inline_new(parent, PendingNewKind::Folder, window, cx);
    }

    fn on_action_rename(&mut self, _: &RenameFile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.selected_or_context_target() else {
            return;
        };
        self.start_inline_rename(target, window, cx);
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

    fn on_action_cancel_inline_edit(
        &mut self,
        _: &CancelInlineEdit,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_inline_edit(cx);
    }

    fn cancel_inline_edit(&mut self, cx: &mut Context<Self>) {
        if self.renaming_id.is_some() || self.pending_new.is_some() {
            self.renaming_id = None;
            self.rename_input = None;
            self.pending_new = None;
            cx.notify();
        }
    }

    fn selected_or_context_target(&self) -> Option<PathBuf> {
        self.selected_id
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| self.context_target.clone())
    }

    fn visible_entries(&self) -> Vec<(TreeItem, usize)> {
        let mut entries = Vec::new();
        Self::flatten_items(&self.items, &mut entries, 0);
        entries
    }

    fn select_item(&mut self, id: impl Into<String>, path: PathBuf, cx: &mut Context<Self>) {
        let id = id.into();
        self.selected_id = Some(id);
        self.context_target = Some(path);
        self.renaming_id = None;
        self.rename_input = None;
        self.pending_new = None;
        cx.notify();
    }

    fn open_or_expand_selected(&mut self, cx: &mut Context<Self>) {
        if self.renaming_id.is_some() || self.pending_new.is_some() {
            return;
        }
        let selected_id = match self.selected_id.clone() {
            Some(selected_id) => selected_id,
            None => return,
        };
        let path = PathBuf::from(&selected_id);
        if self.folder_ids.contains(selected_id.as_str()) {
            if let Some(item) = Self::find_item(&self.items, &selected_id) {
                if !item.is_expanded() {
                    Self::toggle_expanded(&mut self.items, &selected_id);
                    self.refresh_tree(cx);
                }
            }
        } else {
            cx.emit(OpenFileEvent { path });
            cx.notify();
        }
    }

    fn on_action_open_selected(
        &mut self,
        _: &OpenSelectedFile,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_or_expand_selected(cx);
    }

    fn on_action_select_previous(
        &mut self,
        _: &SelectPreviousItem,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_relative(-1, cx);
    }

    fn on_action_select_next(
        &mut self,
        _: &SelectNextItem,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_relative(1, cx);
    }

    fn on_action_expand_selected(
        &mut self,
        _: &ExpandSelectedItem,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selected_id) = self.selected_id.clone() else {
            return;
        };
        if !self.folder_ids.contains(selected_id.as_str()) {
            return;
        }
        if let Some(item) = Self::find_item(&self.items, &selected_id) {
            if !item.is_expanded() {
                Self::toggle_expanded(&mut self.items, &selected_id);
                self.refresh_tree(cx);
            }
        }
    }

    fn on_action_collapse_selected(
        &mut self,
        _: &CollapseSelectedItem,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selected_id) = self.selected_id.clone() else {
            return;
        };
        if !self.folder_ids.contains(selected_id.as_str()) {
            return;
        }
        if let Some(item) = Self::find_item(&self.items, &selected_id) {
            if item.is_expanded() {
                Self::toggle_expanded(&mut self.items, &selected_id);
                cx.notify();
            }
        }
    }

    fn on_action_copy_selected_name(
        &mut self,
        _: &CopySelectedName,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selected_id) = self.selected_id.as_ref() else {
            return;
        };
        if self.pending_new.is_some() {
            return;
        }
        let path = PathBuf::from(selected_id);
        let value = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(selected_id)
            .to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(value));
    }

    fn select_relative(&mut self, offset: isize, cx: &mut Context<Self>) {
        let entries = self.visible_entries();
        if entries.is_empty() {
            return;
        }

        let current_ix = self.selected_id.as_ref().and_then(|selected_id| {
            entries
                .iter()
                .position(|(item, _)| item.id == selected_id.as_str())
        });
        if current_ix.is_none() {
            let next_ix = if offset < 0 { entries.len() - 1 } else { 0 };
            let Some((selected, _)) = entries.get(next_ix) else {
                return;
            };
            self.selected_id = Some(selected.id.to_string());
            self.context_target = Some(PathBuf::from(selected.id.as_str()));
            self.renaming_id = None;
            self.rename_input = None;
            cx.notify();
            return;
        }
        let Some(current_ix) = current_ix else {
            return;
        };
        let next_ix = if offset < 0 {
            if current_ix == 0 {
                entries.len() - 1
            } else {
                current_ix - 1
            }
        } else if current_ix + 1 >= entries.len() {
            0
        } else {
            current_ix + 1
        };

        let Some((selected, _)) = entries.get(next_ix) else {
            return;
        };
        self.selected_id = Some(selected.id.to_string());
        self.context_target = Some(PathBuf::from(selected.id.as_str()));
        self.renaming_id = None;
        self.rename_input = None;
        cx.notify();
    }

    fn find_item<'a>(items: &'a [TreeItem], id: &str) -> Option<&'a TreeItem> {
        for item in items {
            if item.id == id {
                return Some(item);
            }
            if let Some(found) = Self::find_item(&item.children, id) {
                return Some(found);
            }
        }
        None
    }

    fn start_inline_new(
        &mut self,
        parent: PathBuf,
        kind: PendingNewKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let parent_id = parent.to_string_lossy().to_string();
        Self::set_expanded(&mut self.items, &parent_id, true);

        let id = format!(
            "{}::__sqlab_pending_new_{}__",
            parent_id,
            if kind == PendingNewKind::File {
                "file"
            } else {
                "folder"
            }
        );
        let input_state = cx.new(|cx| {
            InputState::new(window, cx).placeholder(if kind == PendingNewKind::File {
                "File name..."
            } else {
                "Folder name..."
            })
        });
        let input_state_for_subscribe = input_state.clone();
        let id_for_subscribe = id.clone();
        cx.subscribe_in(
            &input_state_for_subscribe,
            window,
            move |panel, input, event: &InputEvent, _window, cx| match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    panel.finish_inline_new(id_for_subscribe.clone(), input, cx);
                }
                _ => {}
            },
        )
        .detach();

        self.renaming_id = None;
        self.rename_input = None;
        self.selected_id = Some(id.clone());
        self.context_target = Some(parent.clone());
        self.pending_new = Some(PendingNewEntry {
            id,
            parent,
            kind,
            input: input_state.clone(),
        });
        cx.notify();
        window.focus(&input_state.read(cx).focus_handle(cx), cx);
    }

    fn finish_inline_new(
        &mut self,
        pending_id: String,
        input_state: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.pending_new.clone() else {
            return;
        };
        if pending.id != pending_id {
            return;
        }

        let name = input_state.read(cx).value().trim().to_string();
        self.pending_new = None;

        if name.is_empty() {
            cx.notify();
            return;
        }

        let new_path = pending.parent.join(&name);
        let result = if pending.kind == PendingNewKind::File {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&new_path)
                .map(|_| ())
        } else {
            std::fs::create_dir(&new_path)
        };

        if let Err(e) = result {
            println!(
                "Failed to create {}: {}",
                if pending.kind == PendingNewKind::File {
                    "file"
                } else {
                    "folder"
                },
                e
            );
        } else {
            self.selected_id = Some(new_path.to_string_lossy().to_string());
            self.context_target = Some(new_path.clone());
            if pending.kind == PendingNewKind::File {
                cx.emit(OpenFileEvent { path: new_path });
            }
        }
        self.refresh_tree(cx);
    }

    fn start_inline_rename(
        &mut self,
        target: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = target.to_string_lossy().to_string();
        let current_name = target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if current_name.is_empty() {
            return;
        }

        self.pending_new = None;

        let input_state = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(current_name.clone())
                .placeholder("New name...")
        });
        let input_state_for_subscribe = input_state.clone();
        let id_for_subscribe = id.clone();
        cx.subscribe_in(
            &input_state_for_subscribe,
            window,
            move |panel, input, event: &InputEvent, _window, cx| match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    panel.finish_inline_rename(id_for_subscribe.clone(), input, cx);
                }
                _ => {}
            },
        )
        .detach();

        self.selected_id = Some(id.clone());
        self.context_target = Some(target);
        self.renaming_id = Some(id);
        self.rename_input = Some(input_state.clone());
        cx.notify();
        window.focus(&input_state.read(cx).focus_handle(cx), cx);
    }

    fn finish_inline_rename(
        &mut self,
        target_id: String,
        input_state: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) {
        if self.renaming_id.as_deref() != Some(target_id.as_str()) {
            return;
        }

        let target = PathBuf::from(&target_id);
        let current_name = target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let name = input_state.read(cx).value().trim().to_string();
        self.renaming_id = None;
        self.rename_input = None;

        if name.is_empty() || name == current_name {
            cx.notify();
            return;
        }

        let new_path = target.parent().unwrap_or(&target).join(&name);
        if let Err(e) = std::fs::rename(&target, &new_path) {
            println!("Failed to rename: {}", e);
        } else {
            if self.root.as_ref() == Some(&target) {
                self.root = Some(new_path.clone());
                cx.emit(RootChangedEvent);
            }
            self.selected_id = Some(new_path.to_string_lossy().to_string());
            self.context_target = Some(new_path);
        }
        self.refresh_tree(cx);
    }
}

fn build_dir_items(
    root: &Path,
    path: &Path,
    expanded_ids: &HashSet<String>,
    folder_ids: &mut HashSet<String>,
) -> Vec<TreeItem> {
    let mut items = Vec::new();
    let mut builder = ignore::WalkBuilder::new(path);
    builder
        .max_depth(Some(1))
        .hidden(false)
        .require_git(false)
        .parents(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true);

    for entry in builder
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.depth() == 1)
    {
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) == Some(".git") {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unknown")
            .to_string();
        let id = path.to_string_lossy().to_string();
        let is_dir = entry
            .file_type()
            .is_some_and(|file_type| file_type.is_dir());
        if is_dir {
            folder_ids.insert(id.clone());
            let is_expanded = expanded_ids.contains(&id);
            let children = if is_expanded {
                build_dir_items(root, path, expanded_ids, folder_ids)
            } else {
                Vec::new()
            };
            let mut item = TreeItem::new(id.clone(), file_name).children(children);
            if is_expanded {
                item = item.expanded(true);
            }
            items.push((item, true));
        } else {
            items.push((TreeItem::new(id, file_name), false));
        }
    }

    items
        .sort_by(|(a, a_is_dir), (b, b_is_dir)| b_is_dir.cmp(a_is_dir).then(a.label.cmp(&b.label)));
    items.into_iter().map(|(item, _)| item).collect()
}

fn build_file_items(
    root: &Path,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "sqlab-file-tree-{name}-{}-{suffix}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            _ = fs::remove_dir_all(&self.path);
        }
    }

    fn child_by_label<'a>(item: &'a TreeItem, label: &str) -> Option<&'a TreeItem> {
        item.children.iter().find(|child| child.label == label)
    }

    #[test]
    fn build_file_items_respects_gitignore() {
        let dir = TestDir::new("gitignore");
        fs::create_dir(dir.path.join(".git")).unwrap();
        fs::write(dir.path.join(".gitignore"), "node_modules/\n").unwrap();
        fs::create_dir_all(dir.path.join("node_modules/package")).unwrap();
        fs::create_dir_all(dir.path.join("src")).unwrap();

        let (items, folder_ids) = build_file_items(&dir.path, &HashSet::new());
        let root = &items[0];

        assert!(child_by_label(root, "src").is_some());
        assert!(child_by_label(root, ".git").is_none());
        assert!(child_by_label(root, "node_modules").is_none());
        assert!(folder_ids.contains(dir.path.join("src").to_string_lossy().as_ref()));
        assert!(!folder_ids.contains(dir.path.join("node_modules").to_string_lossy().as_ref()));
    }

    #[test]
    fn build_file_items_does_not_read_collapsed_directories() {
        let dir = TestDir::new("lazy");
        let large_dir = dir.path.join("large");
        fs::create_dir_all(large_dir.join("nested")).unwrap();
        fs::write(large_dir.join("nested").join("query.sql"), "select 1").unwrap();

        let (items, folder_ids) = build_file_items(&dir.path, &HashSet::new());
        let root = &items[0];
        let large = child_by_label(root, "large").unwrap();

        assert!(folder_ids.contains(large_dir.to_string_lossy().as_ref()));
        assert!(large.children.is_empty());

        let mut expanded_ids = HashSet::new();
        expanded_ids.insert(large_dir.to_string_lossy().to_string());
        let (items, _) = build_file_items(&dir.path, &expanded_ids);
        let root = &items[0];
        let large = child_by_label(root, "large").unwrap();

        assert!(child_by_label(large, "nested").is_some());
    }
}

impl Render for FileTreePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _view = cx.entity();
        let mut entries = Vec::new();
        Self::flatten_items(&self.items, &mut entries, 0);
        if let Some(pending) = self.pending_new.clone() {
            let parent_id = pending.parent.to_string_lossy().to_string();
            let insert_at = entries
                .iter()
                .position(|(item, _)| item.id == parent_id)
                .map(|ix| ix + 1)
                .unwrap_or(entries.len());
            let depth = entries
                .get(insert_at.saturating_sub(1))
                .and_then(|(item, depth)| {
                    if item.id == parent_id {
                        Some(depth + 1)
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            entries.insert(insert_at, (TreeItem::new(pending.id.clone(), ""), depth));
        }

        let focus_handle = self.focus_handle.clone();
        let can_paste = self.cut_buffer.is_some();

        v_flex()
            .id("file-tree-panel")
            .size_full()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_action_new_file))
            .on_action(cx.listener(Self::on_action_new_folder))
            .on_action(cx.listener(Self::on_action_rename))
            .on_action(cx.listener(Self::on_action_delete))
            .on_action(cx.listener(Self::on_action_cut_file))
            .on_action(cx.listener(Self::on_action_paste_file))
            .on_action(cx.listener(Self::on_action_open_selected))
            .on_action(cx.listener(Self::on_action_select_previous))
            .on_action(cx.listener(Self::on_action_select_next))
            .on_action(cx.listener(Self::on_action_expand_selected))
            .on_action(cx.listener(Self::on_action_collapse_selected))
            .on_action(cx.listener(Self::on_action_copy_selected_name))
            .on_action(cx.listener(Self::on_action_cancel_inline_edit))
            .context_menu(move |menu, _window, _cx| {
                menu.action_context(focus_handle.clone())
                    .menu("New File", Box::new(NewFile))
                    .menu("New Folder", Box::new(NewFolder))
                    .separator()
                    .menu("Cut", Box::new(CutFile))
                    .menu_with_disabled("Paste", Box::new(PasteFile), !can_paste)
                    .menu("Copy Name", Box::new(CopySelectedName))
                    .separator()
                    .menu("Rename", Box::new(RenameFile))
                    .menu("Delete", Box::new(DeleteFile))
            })
            .child(
                v_flex()
                    .id("tree-inner")
                    .overflow_y_scroll()
                    .children(entries.into_iter().enumerate().map(|(ix, (item, depth))| {
                        let pending = self.pending_new.clone();
                        let pending = pending.filter(|pending| pending.id == item.id.as_str());
                        let path = PathBuf::from(item.id.as_str());
                        let is_pending = pending.is_some();
                        let is_dir = pending
                            .as_ref()
                            .map(|pending| pending.kind == PendingNewKind::Folder)
                            .unwrap_or_else(|| self.folder_ids.contains(item.id.as_str()));
                        let is_selected = self
                            .selected_id
                            .as_ref()
                            .map(|s| s == item.id.as_ref())
                            .unwrap_or(false);
                        let is_active_editor =
                            self.active_editor_path.as_deref() == Some(item.id.as_str());
                        let is_expanded = item.is_expanded();
                        let is_cut = self
                            .cut_buffer
                            .as_ref()
                            .map(|b| b.to_string_lossy() == item.id.as_str())
                            .unwrap_or(false);
                        let is_renaming = self
                            .renaming_id
                            .as_ref()
                            .map(|id| id == item.id.as_ref())
                            .unwrap_or(false);
                        let active_input = pending
                            .as_ref()
                            .map(|pending| pending.input.clone())
                            .or_else(|| self.rename_input.clone());

                        let icon = if !is_dir {
                            let is_sql = path
                                .extension()
                                .and_then(|ext| ext.to_str())
                                .map(|ext| ext.eq_ignore_ascii_case("sql"))
                                .unwrap_or(false);
                            if is_sql {
                                Icon::new(IconName::File).path("icons/file-sql.svg")
                            } else {
                                Icon::new(IconName::File).path("icons/file.svg")
                            }
                        } else if is_expanded {
                            Icon::new(IconName::FolderOpen)
                        } else {
                            Icon::new(IconName::Folder)
                        }
                        .size(gpui::px(16.))
                        .flex_none();

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
                            .when(is_selected, |this| {
                                this.bg(if cx.theme().is_dark() {
                                    hsla(0.74, 0.48, 0.38, 0.58)
                                } else {
                                    hsla(0.74, 0.42, 0.70, 0.58)
                                })
                            })
                            .when(is_cut, |this| this.text_color(cx.theme().muted_foreground))
                            .child(h_flex().gap_2().child(icon).child(
                                if is_renaming || is_pending {
                                    active_input
                                        .as_ref()
                                        .map(|input| Input::new(input).into_any_element())
                                        .unwrap_or_else(|| {
                                            div().child(item.label.clone()).into_any_element()
                                        })
                                } else {
                                    div()
                                        .when(is_active_editor, |this| {
                                            this.text_color(if cx.theme().is_dark() {
                                                hsla(0.74, 0.88, 0.74, 1.0)
                                            } else {
                                                hsla(0.74, 0.78, 0.42, 1.0)
                                            })
                                        })
                                        .child(item.label.clone())
                                        .into_any_element()
                                },
                            ))
                            .on_click(cx.listener(
                                move |panel, event: &gpui::ClickEvent, window, cx| {
                                    if is_renaming || is_pending {
                                        return;
                                    }
                                    window.focus(&panel.focus_handle, cx);
                                    panel.select_item(
                                        item_id_click.to_string(),
                                        path_click.clone(),
                                        cx,
                                    );
                                    if event.click_count() == 2 {
                                        if is_dir {
                                            Self::toggle_expanded(&mut panel.items, &item_id_click);
                                            panel.refresh_tree(cx);
                                        } else {
                                            cx.emit(OpenFileEvent {
                                                path: path_click.clone(),
                                            });
                                            cx.notify();
                                        }
                                    }
                                },
                            ))
                            .when(!is_pending, |this| {
                                this.on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |panel, _, window, cx| {
                                        window.focus(&panel.focus_handle, cx);
                                        panel.context_target = Some(path_right.clone());
                                        panel.selected_id = Some(item_id_right.to_string());
                                        cx.notify();
                                    }),
                                )
                                .on_drag(path_drag.clone(), move |path, _, _, cx| {
                                    cx.new(|_| DragPreview {
                                        label: path
                                            .file_name()
                                            .map(|name| name.to_string_lossy().into())
                                            .unwrap_or_else(|| path.display().to_string().into()),
                                    })
                                })
                                .on_drop(cx.listener(
                                    move |panel, dragged_path: &PathBuf, _, cx| {
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
                                    },
                                ))
                                .drag_over(
                                    |style, _path: &PathBuf, _, cx| {
                                        style.bg(cx.theme().accent.opacity(0.3))
                                    },
                                )
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
