use std::collections::HashSet;

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window,
    div, prelude::FluentBuilder, px, rgb,
};
use gpui_component::{
    ActiveTheme, Icon, IconName, Sizable, WindowExt,
    button::{Button, ButtonVariants as _},
    dock::{Panel, PanelEvent, PanelState},
    h_flex,
    input::{Input, InputState},
    menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
    tree::TreeItem,
    v_flex,
};

use crate::data_source::manager::{DataSourceManager, IntrospectionStatus};
use crate::data_source::{
    ConnectionStatus, DataSourceConfig, DataSourceError, TableKind, create_data_source,
};
use crate::schema_cache;
use crate::ui::activity::ActivityTracker;

pub struct ConnectionPanel {
    manager: Entity<DataSourceManager>,
    activity_tracker: Entity<ActivityTracker>,
    focus_handle: FocusHandle,
    expanded_connections: HashSet<String>,
    expanded_nodes: HashSet<String>,
    selected_node: Option<String>,
    selected_connection: Option<String>,
    shown_errors: HashSet<String>,
    shown_credential_errors: HashSet<String>,
    shown_global_credential_error: bool,
}

impl EventEmitter<PanelEvent> for ConnectionPanel {}

impl ConnectionPanel {
    pub fn new(
        manager: Entity<DataSourceManager>,
        activity_tracker: Entity<ActivityTracker>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&manager, |_, _, cx| {
            cx.notify();
        })
        .detach();

        Self {
            manager,
            activity_tracker,
            focus_handle: cx.focus_handle(),
            expanded_connections: HashSet::new(),
            expanded_nodes: HashSet::new(),
            selected_node: None,
            selected_connection: None,
            shown_errors: HashSet::new(),
            shown_credential_errors: HashSet::new(),
            shown_global_credential_error: false,
        }
    }

    fn open_create_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_config_dialog(None, DataSourceConfig::default(), window, cx);
    }

    fn open_edit_dialog(
        &mut self,
        old_name: String,
        config: DataSourceConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let recovery_error = self.manager.update(cx, |manager, cx| {
            let error = manager.ensure_password_loaded(&old_name).err();
            cx.notify();
            error
        });
        if let Some(error) = recovery_error {
            let title = format!("Keychain Access Error: {}", old_name);
            window.open_alert_dialog(cx, move |alert, _, _| {
                alert.title(title.clone()).description(error.clone())
            });
        }

        let config = self
            .manager
            .read(cx)
            .configs()
            .iter()
            .find(|config| config.name == old_name)
            .cloned()
            .unwrap_or(config);
        self.open_config_dialog(Some(old_name), config, window, cx);
    }

    fn open_config_dialog(
        &mut self,
        old_name: Option<String>,
        config: DataSourceConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let manager = self.manager.clone();
        let name = cx.new(|cx| InputState::new(window, cx).default_value(config.name));
        let db_type = cx.new(|cx| InputState::new(window, cx).default_value(config.db_type));
        let host = cx.new(|cx| InputState::new(window, cx).default_value(config.host));
        let port = cx.new(|cx| InputState::new(window, cx).default_value(config.port.to_string()));
        let user = cx.new(|cx| InputState::new(window, cx).default_value(config.user));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(config.password)
                .masked(true)
        });
        let database = cx.new(|cx| InputState::new(window, cx).default_value(config.database));
        let schema = cx.new(|cx| InputState::new(window, cx).default_value(config.schema));
        let query_string =
            cx.new(|cx| InputState::new(window, cx).default_value(config.query_string));

        let title = if old_name.is_some() {
            "Edit Data Source"
        } else {
            "Add Data Source"
        };

        let view = cx.entity();

        window.open_alert_dialog(cx, move |alert, _window, _cx| {
            alert
                .title(title)
                .child(
                    v_flex()
                        .gap_2()
                        .w(px(420.))
                        .child(form_field("Name", Input::new(&name)))
                        .child(form_field("Type", Input::new(&db_type)))
                        .child(form_field("Host", Input::new(&host)))
                        .child(form_field("Port", Input::new(&port)))
                        .child(form_field("User", Input::new(&user)))
                        .child(form_field("Password", Input::new(&password)))
                        .child(form_field("Database", Input::new(&database)))
                        .child(form_field("Schema", Input::new(&schema)))
                        .child(form_field("Query String", Input::new(&query_string))),
                )
                .show_cancel(true)
                .on_ok({
                    let manager = manager.clone();
                    let old_name = old_name.clone();
                    let name_for_ok = name.clone();
                    let db_type_for_ok = db_type.clone();
                    let host_for_ok = host.clone();
                    let port_for_ok = port.clone();
                    let user_for_ok = user.clone();
                    let password_for_ok = password.clone();
                    let database_for_ok = database.clone();
                    let schema_for_ok = schema.clone();
                    let query_string_for_ok = query_string.clone();
                    let view = view.clone();
                    move |_, window: &mut Window, cx: &mut App| {
                        let name = name_for_ok.read(cx).value().trim().to_string();
                        let db_type = db_type_for_ok.read(cx).value().trim().to_string();
                        let host = host_for_ok.read(cx).value().trim().to_string();
                        let port = port_for_ok
                            .read(cx)
                            .value()
                            .trim()
                            .parse::<u16>()
                            .unwrap_or(5432);
                        let user = user_for_ok.read(cx).value().trim().to_string();
                        let password = password_for_ok.read(cx).value().to_string();
                        let database = database_for_ok.read(cx).value().trim().to_string();
                        let schema = schema_for_ok.read(cx).value().trim().to_string();
                        let query_string = query_string_for_ok.read(cx).value().trim().to_string();

                        if name.is_empty()
                            || db_type.is_empty()
                            || host.is_empty()
                            || user.is_empty()
                            || database.is_empty()
                        {
                            return false;
                        }

                        let duplicate = manager.read(cx).configs().iter().any(|config| {
                            config.name == name && old_name.as_deref() != Some(config.name.as_str())
                        });
                        if duplicate {
                            return false;
                        }

                        let config = DataSourceConfig {
                            name: name.clone(),
                            db_type,
                            host,
                            port,
                            user,
                            password,
                            database,
                            schema,
                            query_string,
                        };

                        let is_new = old_name.is_none();
                        let save_result = manager.update(cx, |manager, cx| {
                            if let Some(old_name) = old_name.as_deref() {
                                manager.update(old_name, config.clone());
                            } else {
                                manager.add(config.clone());
                            }
                            let save_result = manager.save();
                            match &save_result {
                                Ok(()) => manager.clear_credential_error(&name),
                                Err(error) => {
                                    manager.set_credential_error(&name, error.to_string());
                                }
                            }
                            cx.notify();
                            save_result.map_err(|error| error.to_string())
                        });
                        if let Err(error) = save_result {
                            let title = format!("Keychain Access Error: {}", name);
                            window.open_alert_dialog(cx, move |alert, _, _| {
                                alert.title(title.clone()).description(error.clone())
                            });
                            return false;
                        }

                        if is_new {
                            let _ = view.update(cx, |panel, cx| {
                                panel.expanded_connections.insert(name.clone());
                                panel.introspect_schema(config, cx);
                            });
                        }

                        true
                    }
                })
        });
    }

    fn delete_connection(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let manager = self.manager.clone();
        window.open_alert_dialog(cx, move |alert, _window, _cx| {
            alert
                .title("Delete Data Source")
                .child(format!("Delete \"{}\"?", name))
                .show_cancel(true)
                .on_ok({
                    let manager = manager.clone();
                    let name = name.clone();
                    move |_, window: &mut Window, cx: &mut App| {
                        let save_result = manager.update(cx, |manager, cx| {
                            manager.remove(&name);
                            let save_result = manager.save();
                            cx.notify();
                            save_result.map_err(|error| error.to_string())
                        });
                        if let Err(error) = save_result {
                            window.open_alert_dialog(cx, move |alert, _, _| {
                                alert
                                    .title("Keychain Access Error")
                                    .description(error.clone())
                            });
                            return false;
                        }
                        true
                    }
                })
        });
    }

    fn connection_context_menu(
        manager: Entity<DataSourceManager>,
        menu_name: String,
        menu_config: DataSourceConfig,
        view: Entity<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        move |menu, window, _cx| {
            let menu_name_for_refresh = menu_name.clone();
            let menu_name_for_duplicate = menu_name.clone();
            let menu_name_for_delete = menu_name.clone();
            let menu_name_for_configure = menu_name.clone();
            let menu_config_for_configure = menu_config.clone();
            let view_for_refresh = view.clone();
            let view_for_duplicate = view.clone();
            let view_for_delete = view.clone();
            let view_for_configure = view.clone();

            menu.item(
                PopupMenuItem::new("Refresh Schema")
                    .icon(IconName::Redo)
                    .on_click(window.listener_for(&view_for_refresh, {
                        let menu_name = menu_name_for_refresh.clone();
                        move |this, _, window, cx| {
                            this.refresh_schema(menu_name.clone(), window, cx);
                        }
                    })),
            )
            .item(
                PopupMenuItem::new("Duplicate")
                    .icon(IconName::Copy)
                    .on_click(window.listener_for(&view_for_duplicate, {
                        let manager = manager.clone();
                        let menu_name = menu_name_for_duplicate.clone();
                        move |_this, _, window, cx| {
                            let save_result = manager.update(cx, |manager, cx| {
                                manager.duplicate(&menu_name);
                                let save_result = manager.save();
                                if let Err(error) = &save_result {
                                    manager.set_credential_error(&menu_name, error.to_string());
                                }
                                cx.notify();
                                save_result.map_err(|error| error.to_string())
                            });
                            if let Err(error) = save_result {
                                window.open_alert_dialog(cx, move |alert, _, _| {
                                    alert.title("Keychain Access Error").description(error.clone())
                                });
                            }
                        }
                    })),
            )
            .item(
                PopupMenuItem::new("Delete")
                    .icon(IconName::Delete)
                    .on_click(window.listener_for(&view_for_delete, {
                        let menu_name = menu_name_for_delete.clone();
                        move |this, _, window, cx| {
                            this.delete_connection(menu_name.clone(), window, cx);
                        }
                    })),
            )
            .item(
                PopupMenuItem::new("Configure")
                    .icon(IconName::Settings)
                    .on_click(window.listener_for(&view_for_configure, {
                        let menu_name = menu_name_for_configure.clone();
                        let menu_config = menu_config_for_configure.clone();
                        move |this, _, window, cx| {
                            this.open_edit_dialog(
                                menu_name.clone(),
                                menu_config.clone(),
                                window,
                                cx,
                            );
                        }
                    })),
            )
        }
    }

    fn introspect_schema(&mut self, config: DataSourceConfig, cx: &mut Context<Self>) {
        let manager = self.manager.clone();
        let activity_tracker = self.activity_tracker.clone();
        let cache_key = schema_cache::cache_key(&config);
        let name = config.name.clone();
        let name_for_cache = name.clone();
        let activity_label = format!("Refreshing schema: {}", name);
        let activity_id = self
            .activity_tracker
            .update(cx, |tracker, cx| tracker.begin(activity_label, cx));

        manager.update(cx, |m, _cx| {
            m.set_introspection_status(&name, IntrospectionStatus::Running);
        });
        cx.notify();

        cx.spawn(async move |_this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut source = create_data_source(&config)?;
                    source.connect().await?;
                    let schema = source.introspect_schema().await?;
                    source.disconnect().await?;
                    schema_cache::save(&cache_key, &name_for_cache, &schema)?;
                    Ok::<_, anyhow::Error>(())
                })
                .await;

            cx.update_entity(&manager, |manager, cx| {
                match result {
                    Ok(_) => manager.set_introspection_status(&name, IntrospectionStatus::Cached),
                    Err(e) => {
                        eprintln!("Schema introspection failed for {}: {}", name, e);
                        manager.set_introspection_status(&name, IntrospectionStatus::Failed);
                    }
                }
                cx.notify();
            });

            cx.update_entity(&activity_tracker, |tracker, cx| {
                tracker.finish(activity_id, cx);
            });
        })
        .detach();
    }

    fn test_connection(&mut self, name: String, cx: &mut Context<Self>) {
        let manager = self.manager.clone();
        let activity_tracker = self.activity_tracker.clone();
        let config_name = name.clone();

        let Some(config) = manager.update(cx, |manager, cx| {
            manager.set_active(Some(config_name.clone()));
            manager.set_status(&config_name, ConnectionStatus::Idle);
            manager.clear_last_error(&config_name);
            if let Err(error) = manager.ensure_password_loaded(&config_name) {
                manager.set_status(&config_name, ConnectionStatus::Failed);
                manager.set_last_error(&config_name, error);
                cx.notify();
                return None;
            }
            let config = manager
                .configs()
                .iter()
                .find(|config| config.name == config_name)
                .cloned();
            cx.notify();
            config
        }) else {
            return;
        };

        let activity_label = format!("Connecting: {}", config_name);
        let activity_id = self
            .activity_tracker
            .update(cx, |tracker, cx| tracker.begin(activity_label, cx));

        cx.spawn(async move |_this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut source = create_data_source(&config)?;
                    source.connect().await?;
                    source.disconnect().await?;
                    Ok::<(), DataSourceError>(())
                })
                .await;

            cx.update_entity(&manager, move |manager, cx| {
                match result {
                    Ok(_) => {
                        manager.set_status(&config_name, ConnectionStatus::Connected);
                        manager.clear_last_error(&config_name);
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        manager.set_status(&config_name, ConnectionStatus::Failed);
                        manager.set_last_error(&config_name, msg);
                    }
                }
                cx.notify();
            });

            cx.update_entity(&activity_tracker, |tracker, cx| {
                tracker.finish(activity_id, cx);
            });
        })
        .detach();
    }

    fn refresh_schema(&mut self, name: String, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(config) = self
            .manager
            .read(cx)
            .configs()
            .iter()
            .find(|c| c.name == name)
            .cloned()
        else {
            return;
        };

        self.introspect_schema(config, cx);
    }

    fn toggle_connection_expanded(&mut self, name: &str) {
        if self.expanded_connections.contains(name) {
            self.expanded_connections.remove(name);
        } else {
            self.expanded_connections.insert(name.to_string());
        }
    }

    fn toggle_node_expanded(&mut self, id: &str) {
        if self.expanded_nodes.contains(id) {
            self.expanded_nodes.remove(id);
        } else {
            self.expanded_nodes.insert(id.to_string());
        }
    }

    fn build_schema_tree_items(
        connection_name: &str,
        database_name: &str,
        schema: &crate::data_source::DatabaseSchema,
        expanded: &HashSet<String>,
    ) -> Vec<TreeItem> {
        let mut root_items = Vec::new();

        // Group tables by schema and by kind
        let mut schema_tables: std::collections::HashMap<
            String,
            Vec<&crate::data_source::TableInfo>,
        > = std::collections::HashMap::new();
        let mut schema_views: std::collections::HashMap<
            String,
            Vec<&crate::data_source::TableInfo>,
        > = std::collections::HashMap::new();
        for table in &schema.tables {
            if matches!(table.kind, TableKind::View | TableKind::MaterializedView) {
                schema_views
                    .entry(table.schema.clone())
                    .or_default()
                    .push(table);
            } else {
                schema_tables
                    .entry(table.schema.clone())
                    .or_default()
                    .push(table);
            }
        }

        let mut schema_functions: std::collections::HashMap<
            String,
            Vec<&crate::data_source::FunctionInfo>,
        > = std::collections::HashMap::new();
        for func in &schema.functions {
            schema_functions
                .entry(func.schema.clone())
                .or_default()
                .push(func);
        }

        let mut schema_sequences: std::collections::HashMap<
            String,
            Vec<&crate::data_source::SequenceInfo>,
        > = std::collections::HashMap::new();
        for seq in &schema.sequences {
            schema_sequences
                .entry(seq.schema.clone())
                .or_default()
                .push(seq);
        }

        let mut schema_indexes: std::collections::HashMap<
            String,
            Vec<&crate::data_source::IndexInfo>,
        > = std::collections::HashMap::new();
        for idx in &schema.indexes {
            schema_indexes
                .entry(idx.schema.clone())
                .or_default()
                .push(idx);
        }

        let mut schema_triggers: std::collections::HashMap<
            String,
            Vec<&crate::data_source::TriggerInfo>,
        > = std::collections::HashMap::new();
        for trig in &schema.triggers {
            schema_triggers
                .entry(trig.schema.clone())
                .or_default()
                .push(trig);
        }

        // Schemas folder
        let schemas_id = format!("conn:{}:schemas", connection_name);
        let mut schemas_item = TreeItem::new(schemas_id.clone(), SharedString::from(database_name));

        for schema_info in &schema.schemas {
            let schema_name = &schema_info.name;
            let schema_id = format!("conn:{}:schema:{}", connection_name, schema_name);
            let mut schema_item =
                TreeItem::new(schema_id.clone(), SharedString::from(schema_name.clone()));

            // Tables folder
            if let Some(tables) = schema_tables.get(schema_name) {
                let tables_id = format!("{}:tables", schema_id);
                let mut tables_item =
                    TreeItem::new(tables_id.clone(), SharedString::from("Tables"));
                for table in tables {
                    let table_id = format!("{}:table:{}", schema_id, table.name);
                    let mut table_item =
                        TreeItem::new(table_id.clone(), SharedString::from(table.name.clone()));
                    for col in &table.columns {
                        let mut col_id = format!("{}:col:{}", table_id, col.name);
                        if col.is_pk {
                            col_id.push_str(":pk");
                        }
                        if col.is_fk {
                            col_id.push_str(":fk");
                        }
                        let label = format!(
                            "{} : {}{}",
                            col.name,
                            col.data_type,
                            if col.nullable { "" } else { " NOT NULL" }
                        );
                        table_item =
                            table_item.child(TreeItem::new(col_id, SharedString::from(label)));
                    }
                    if expanded.contains(&table_id) {
                        table_item = table_item.expanded(true);
                    }
                    tables_item = tables_item.child(table_item);
                }
                if expanded.contains(&tables_id) {
                    tables_item = tables_item.expanded(true);
                }
                schema_item = schema_item.child(tables_item);
            }

            // Views folder
            if let Some(views) = schema_views.get(schema_name) {
                let views_id = format!("{}:views", schema_id);
                let mut views_item = TreeItem::new(views_id.clone(), SharedString::from("Views"));
                for view in views {
                    let view_id = format!("{}:view:{}", schema_id, view.name);
                    let label = format!("{} ({})", view.name, table_kind_label(&view.kind));
                    views_item =
                        views_item.child(TreeItem::new(view_id, SharedString::from(label)));
                }
                if expanded.contains(&views_id) {
                    views_item = views_item.expanded(true);
                }
                schema_item = schema_item.child(views_item);
            }

            // Sequences folder
            if let Some(sequences) = schema_sequences.get(schema_name) {
                let seqs_id = format!("{}:sequences", schema_id);
                let mut seqs_item = TreeItem::new(seqs_id.clone(), SharedString::from("Sequences"));
                for seq in sequences {
                    let seq_id = format!("{}:seq:{}", schema_id, seq.name);
                    seqs_item = seqs_item
                        .child(TreeItem::new(seq_id, SharedString::from(seq.name.clone())));
                }
                if expanded.contains(&seqs_id) {
                    seqs_item = seqs_item.expanded(true);
                }
                schema_item = schema_item.child(seqs_item);
            }

            // Indexes folder
            if let Some(indexes) = schema_indexes.get(schema_name) {
                let idxs_id = format!("{}:indexes", schema_id);
                let mut idxs_item = TreeItem::new(idxs_id.clone(), SharedString::from("Indexes"));
                for idx in indexes {
                    let idx_id = format!("{}:idx:{}", schema_id, idx.name);
                    let label = format!(
                        "{}{}",
                        idx.name,
                        if idx.is_primary {
                            " (primary)"
                        } else if idx.is_unique {
                            " (unique)"
                        } else {
                            ""
                        }
                    );
                    idxs_item = idxs_item.child(TreeItem::new(idx_id, SharedString::from(label)));
                }
                if expanded.contains(&idxs_id) {
                    idxs_item = idxs_item.expanded(true);
                }
                schema_item = schema_item.child(idxs_item);
            }

            // Triggers folder
            if let Some(triggers) = schema_triggers.get(schema_name) {
                let trigs_id = format!("{}:triggers", schema_id);
                let mut trigs_item =
                    TreeItem::new(trigs_id.clone(), SharedString::from("Triggers"));
                for trig in triggers {
                    let trig_id = format!("{}:trig:{}", schema_id, trig.name);
                    trigs_item = trigs_item.child(TreeItem::new(
                        trig_id,
                        SharedString::from(trig.name.clone()),
                    ));
                }
                if expanded.contains(&trigs_id) {
                    trigs_item = trigs_item.expanded(true);
                }
                schema_item = schema_item.child(trigs_item);
            }

            // Functions folder
            if let Some(functions) = schema_functions.get(schema_name) {
                let funcs_id = format!("{}:functions", schema_id);
                let mut funcs_item =
                    TreeItem::new(funcs_id.clone(), SharedString::from("Functions"));
                for func in functions {
                    let func_id = format!("{}:func:{}", schema_id, func.name);
                    let label =
                        format!("{}({}) -> {}", func.name, func.arguments, func.return_type);
                    funcs_item =
                        funcs_item.child(TreeItem::new(func_id, SharedString::from(label)));
                }
                if expanded.contains(&funcs_id) {
                    funcs_item = funcs_item.expanded(true);
                }
                schema_item = schema_item.child(funcs_item);
            }

            if expanded.contains(&schema_id) {
                schema_item = schema_item.expanded(true);
            }
            schemas_item = schemas_item.child(schema_item);
        }

        schemas_item = schemas_item.expanded(true);
        root_items.push(schemas_item);

        root_items
    }

    fn flatten_items(items: &[TreeItem], result: &mut Vec<(TreeItem, usize)>, depth: usize) {
        for item in items {
            result.push((item.clone(), depth));
            if item.is_expanded() {
                Self::flatten_items(&item.children, result, depth + 1);
            }
        }
    }

    fn node_icon(id: &str) -> IconName {
        if id.contains(":col:") {
            if id.contains(":pk:") {
                IconName::CircleCheck
            } else if id.contains(":fk:") {
                IconName::ArrowRight
            } else {
                IconName::Minimize
            }
        } else if id.contains(":view:") {
            IconName::Inbox
        } else if id.contains(":table:") {
            IconName::File
        } else if id.contains(":func:") {
            IconName::Cpu
        } else if id.contains(":seq:") {
            IconName::ArrowDown
        } else if id.contains(":idx:") {
            IconName::Search
        } else if id.contains(":trig:") {
            IconName::Bell
        } else if id.contains(":schema:") {
            IconName::FolderOpen
        } else if id.contains(":schemas")
            || id.ends_with(":tables")
            || id.ends_with(":views")
            || id.ends_with(":sequences")
            || id.ends_with(":indexes")
            || id.ends_with(":triggers")
            || id.ends_with(":functions")
        {
            IconName::Folder
        } else {
            IconName::HardDrive
        }
    }

    fn node_icon_path(id: &str) -> Option<&'static str> {
        if id.contains(":col:") {
            if id.contains(":pk:") {
                Some("icons/primary_key.svg")
            } else if id.contains(":fk:") {
                Some("icons/column.svg")
            } else {
                Some("icons/column.svg")
            }
        } else if id.contains(":table:") {
            Some("icons/table.svg")
        } else if id.contains(":view:") {
            Some("icons/table.svg")
        } else if id.contains(":schema:") {
            Some("icons/schema.svg")
        } else if id.contains(":schemas")
            || id.ends_with(":tables")
            || id.ends_with(":views")
            || id.ends_with(":sequences")
            || id.ends_with(":indexes")
            || id.ends_with(":triggers")
            || id.ends_with(":functions")
        {
            Some("icons/schema.svg")
        } else {
            None
        }
    }

    fn is_leaf_node(id: &str) -> bool {
        id.contains(":col:")
            || id.contains(":seq:")
            || id.contains(":idx:")
            || id.contains(":trig:")
            || id.contains(":func:")
            || id.contains(":view:")
    }

    fn copyable_name(id: &str, label: &str) -> Option<String> {
        if id.contains(":col:") {
            // label is like "id bigint not null", extract first word
            label.split_whitespace().next().map(|s| s.to_string())
        } else if id.contains(":table:") {
            id.split(":table:").nth(1).map(|s| s.to_string())
        } else if id.contains(":view:") {
            id.split(":view:")
                .nth(1)
                .map(|s| s.split_whitespace().next().unwrap_or(s).to_string())
        } else if id.contains(":seq:") {
            id.split(":seq:").nth(1).map(|s| s.to_string())
        } else if id.contains(":idx:") {
            id.split(":idx:").nth(1).map(|s| s.to_string())
        } else if id.contains(":trig:") {
            id.split(":trig:").nth(1).map(|s| s.to_string())
        } else if id.contains(":func:") {
            id.split(":func:").nth(1).map(|s| s.to_string())
        } else {
            None
        }
    }
}

impl Panel for ConnectionPanel {
    fn panel_name(&self) -> &'static str {
        "ConnectionPanel"
    }

    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .pl(px(4.))
            .child("Connections")
            .child(
                Button::new("add-connection-title")
                    .icon(IconName::Plus)
                    .xsmall()
                    .ghost()
                    .tooltip("Add Connection")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_create_dialog(window, cx);
                    })),
            )
    }

    fn closable(&self, _cx: &App) -> bool {
        false
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState::new(self)
    }
}

impl Render for ConnectionPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let configs = self.manager.read(cx).configs().to_vec();
        let active_name = self.manager.read(cx).active_name().map(str::to_string);
        let manager = self.manager.clone();

        let mut children: Vec<gpui::AnyElement> = Vec::new();

        // Header
        children.push(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .pl(px(4.))
                .pb_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child("Connections"),
                )
                .child(
                    Button::new("add-connection")
                        .icon(IconName::Plus)
                        .xsmall()
                        .ghost()
                        .tooltip("Add Connection")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_create_dialog(window, cx);
                        })),
                )
                .into_any_element(),
        );

        if configs.is_empty() {
            children.push(
                div()
                    .p_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("No data sources configured.")
                    .into_any_element(),
            );
        }

        for config in &configs {
            let is_active = active_name.as_deref() == Some(config.name.as_str());
            let is_selected = self.selected_connection.as_deref() == Some(config.name.as_str());
            let status = manager.read(cx).status(&config.name);
            let status_color = if is_active {
                match status {
                    ConnectionStatus::Connected => rgb(0x16a34a),
                    ConnectionStatus::Failed => rgb(0xef4444),
                    ConnectionStatus::Idle => rgb(0x9ca3af),
                }
            } else {
                rgb(0x9ca3af)
            };

            let row_name = config.name.clone();
            let menu_name = config.name.clone();
            let menu_config = config.clone();
            let manager = manager.clone();
            let row_manager = manager.clone();
            let view = cx.entity();

            let is_expanded = self.expanded_connections.contains(&config.name);
            let introspection_status = manager.read(cx).introspection_status(&config.name);

            // Connection row
            let row_name_for_active = row_name.clone();
            let row_name_for_expand = row_name.clone();
            children.push(
                h_flex()
                    .id(format!("connection-row-{}", row_name))
                    .w_full()
                    .px_1()
                    .py_0p5()
                    .gap_1()
                    .rounded(cx.theme().radius)
                    .hover(|style| style.bg(cx.theme().accent.opacity(0.1)))
                    .when(is_selected, |this| this.bg(cx.theme().accent.opacity(0.15)))
                    .child(
                        div()
                            .id(format!("connection-expand-icon-{}", row_name))
                            .size(px(16.))
                            .flex_none()
                            .child(
                                Icon::new(if is_expanded {
                                    IconName::ArrowDown
                                } else {
                                    IconName::ArrowRight
                                })
                                .size(px(14.))
                                .text_color(cx.theme().muted_foreground),
                            )
                            .cursor_pointer()
                            .on_click(cx.listener({
                                let row_name = row_name_for_expand.clone();
                                move |this, _, _, cx| {
                                    this.toggle_connection_expanded(&row_name);
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            })),
                    )
                    .child(div().id(format!("connection-icon-{}", row_name)).child(
                        if config.db_type == "postgres" {
                            Icon::new(IconName::File)
                                .path("icons/pg.svg")
                                .size(px(28.))
                                .text_color(rgb(0x336791))
                                .into_any_element()
                        } else {
                            Icon::new(IconName::HardDrive)
                                .size(px(26.))
                                .text_color(status_color)
                                .into_any_element()
                        },
                    ))
                    .child(
                        h_flex()
                            .items_center()
                            .overflow_hidden()
                            .id(format!("connection-label-{}", row_name))
                            .child(
                                div()
                                    .text_base()
                                    .truncate()
                                    .when(is_active, |this| {
                                        this.font_weight(gpui::FontWeight::BOLD)
                                    })
                                    .child(config.name.clone()),
                            )
                            .on_click(cx.listener({
                                let row_manager = row_manager.clone();
                                let row_name = row_name_for_active;
                                move |this, event: &gpui::ClickEvent, _, cx| {
                                    this.selected_connection = Some(row_name.clone());
                                    this.selected_node = None;

                                    // GPUI ClickEvent has click_count() method
                                    if event.click_count() == 2 {
                                        let current_active = row_manager
                                            .read(cx)
                                            .active_name()
                                            .map(|n| n.to_string());
                                        if current_active.as_deref() != Some(row_name.as_str()) {
                                            this.test_connection(row_name.clone(), cx);
                                        }
                                    }
                                    cx.notify();
                                }
                            }))
                            .context_menu(Self::connection_context_menu(
                                manager.clone(),
                                menu_name.clone(),
                                menu_config.clone(),
                                view.clone(),
                            )),
                    )
                    .into_any_element(),
            );
            // Schema tree
            if is_expanded {
                let cache_key = schema_cache::cache_key(&config);
                let schema_opt = schema_cache::load(&cache_key).ok().flatten();

                if let Some(schema) = schema_opt {
                    let tree_items = Self::build_schema_tree_items(
                        &config.name,
                        &config.database,
                        &schema,
                        &self.expanded_nodes,
                    );
                    let mut entries = Vec::new();
                    Self::flatten_items(&tree_items, &mut entries, 1);

                    for (item, depth) in entries {
                        let id = item.id.to_string();
                        let label = item.label.to_string();
                        let is_selected = self.selected_node.as_deref() == Some(&id);
                        let icon = Self::node_icon(&id);
                        let icon_path = Self::node_icon_path(&id);
                        let is_leaf = Self::is_leaf_node(&id);
                        let is_node_expanded = item.is_expanded();
                        let id_click = id.clone();
                        let id_toggle = id.clone();

                        let (name, data_type) = if id.contains(":col:") {
                            if let Some(pos) = label.find(" : ") {
                                (label[..pos].to_string(), Some(label[pos + 3..].to_string()))
                            } else {
                                (label.clone(), None)
                            }
                        } else {
                            (label.clone(), None)
                        };

                        children.push(
                            div()
                                .id(id.clone())
                                .w_full()
                                .py_0p5()
                                .px_1()
                                .pl(px(12.) * depth as f32 + px(4.))
                                .rounded(cx.theme().radius)
                                .when(is_selected, |this| this.bg(cx.theme().accent.opacity(0.15)))
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .child(
                                            div()
                                                .id(format!("expand-{}", id_toggle))
                                                .size(px(14.))
                                                .flex_none()
                                                .child(if !is_leaf {
                                                    Icon::new(if is_node_expanded {
                                                        IconName::ArrowDown
                                                    } else {
                                                        IconName::ArrowRight
                                                    })
                                                    .size(px(14.))
                                                    .text_color(cx.theme().muted_foreground)
                                                    .into_any_element()
                                                } else {
                                                    div().size_full().into_any_element()
                                                })
                                                .cursor_pointer()
                                                .on_click(cx.listener({
                                                    let id_toggle = id_toggle.clone();
                                                    move |this, _, _, cx| {
                                                        if !is_leaf {
                                                            this.toggle_node_expanded(&id_toggle);
                                                            cx.stop_propagation();
                                                            cx.notify();
                                                        }
                                                    }
                                                })),
                                        )
                                        .child(if let Some(path) = icon_path {
                                            let icon_color = if cx.theme().is_dark() {
                                                cx.theme().muted_foreground
                                            } else {
                                                cx.theme().foreground
                                            };
                                            Icon::new(IconName::File)
                                                .path(path)
                                                .size(px(16.))
                                                .flex_none()
                                                .text_color(icon_color)
                                                .into_any_element()
                                        } else {
                                            Icon::new(icon)
                                                .size(px(16.))
                                                .flex_none()
                                                .text_color(cx.theme().muted_foreground)
                                                .into_any_element()
                                        })
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .child(div().text_base().child(name))
                                                .when_some(data_type, |this, dt| {
                                                    this.child(
                                                        div()
                                                            .text_base()
                                                            .text_color(
                                                                cx.theme()
                                                                    .muted_foreground
                                                                    .opacity(0.6),
                                                            )
                                                            .child(dt),
                                                    )
                                                }),
                                        ),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.selected_node = Some(id_click.clone());
                                    this.selected_connection = None;
                                    if is_leaf {
                                        if let Some(name) = Self::copyable_name(&id_click, &label) {
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                                name,
                                            ));
                                        }
                                    }
                                    cx.notify();
                                }))
                                .into_any_element(),
                        );
                    }
                } else {
                    let msg = match introspection_status {
                        IntrospectionStatus::Running => "Refreshing schema...",
                        IntrospectionStatus::Failed => {
                            "Schema refresh failed. Click Refresh to retry."
                        }
                        _ => "Schema not cached. Click Refresh to load.",
                    };
                    children.push(
                        div()
                            .pl(px(32.))
                            .py_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(msg)
                            .into_any_element(),
                    );
                }
            }
        }

        if let Some(error) = manager.read(cx).global_credential_error() {
            if !self.shown_global_credential_error {
                self.shown_global_credential_error = true;
                let error = error.to_string();
                window.open_alert_dialog(cx, move |alert, _, _| {
                    alert
                        .title("Keychain Access Error")
                        .description(error.clone())
                });
            }
        } else {
            self.shown_global_credential_error = false;
        }

        let credential_error_names: HashSet<String> = configs
            .iter()
            .filter(|config| manager.read(cx).credential_error(&config.name).is_some())
            .map(|config| config.name.clone())
            .collect();
        self.shown_credential_errors
            .retain(|name| credential_error_names.contains(name));

        for config in &configs {
            if let Some(error) = manager.read(cx).credential_error(&config.name) {
                if !self.shown_credential_errors.contains(&config.name) {
                    self.shown_credential_errors.insert(config.name.clone());
                    let name = config.name.clone();
                    let error = error.to_string();
                    window.open_alert_dialog(cx, move |alert, _, _| {
                        alert
                            .title(format!("Keychain Access Error: {}", name))
                            .description(error.clone())
                    });
                }
            }
        }

        // Auto-show connection errors for newly failed connections
        let failed_names: HashSet<String> = configs
            .iter()
            .filter(|c| manager.read(cx).status(&c.name) == ConnectionStatus::Failed)
            .map(|c| c.name.clone())
            .collect();
        self.shown_errors.retain(|name| failed_names.contains(name));

        for config in &configs {
            if manager.read(cx).status(&config.name) == ConnectionStatus::Failed {
                if manager.read(cx).credential_error(&config.name).is_some() {
                    continue;
                }
                if let Some(error) = manager.read(cx).last_error(&config.name) {
                    if !self.shown_errors.contains(&config.name) {
                        self.shown_errors.insert(config.name.clone());
                        let name = config.name.clone();
                        let error = error.to_string();
                        window.open_alert_dialog(cx, move |alert, _, _| {
                            alert
                                .title(format!("Connection Error: {}", name))
                                .description(error.clone())
                        });
                    }
                }
            }
        }

        v_flex()
            .id("database-panel")
            .size_full()
            .overflow_y_scroll()
            .overflow_x_scroll()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "right" => {
                        if let Some(id) = &this.selected_node {
                            if !this.expanded_nodes.contains(id) {
                                this.expanded_nodes.insert(id.clone());
                                cx.notify();
                            }
                        } else if let Some(id) = &this.selected_connection {
                            if !this.expanded_connections.contains(id) {
                                this.expanded_connections.insert(id.clone());
                                cx.notify();
                            }
                        }
                    }
                    "left" => {
                        if let Some(id) = &this.selected_node {
                            if this.expanded_nodes.contains(id) {
                                this.expanded_nodes.remove(id);
                                cx.notify();
                            }
                        } else if let Some(id) = &this.selected_connection {
                            if this.expanded_connections.contains(id) {
                                this.expanded_connections.remove(id);
                                cx.notify();
                            }
                        }
                    }
                    _ => {}
                }
            }))
            .child(v_flex().children(children).text_sm().p_1().w_full())
    }
}

impl Focusable for ConnectionPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn form_field(label: &'static str, input: impl IntoElement) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_xs().child(label))
        .child(input)
}

fn table_kind_label(kind: &TableKind) -> &'static str {
    match kind {
        TableKind::Table => "table",
        TableKind::View => "view",
        TableKind::MaterializedView => "materialized view",
        TableKind::ForeignTable => "foreign table",
    }
}
