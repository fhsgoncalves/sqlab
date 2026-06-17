use std::{collections::HashSet, rc::Rc};

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, ScrollStrategy, SharedString, Size,
    StatefulInteractiveElement, Styled, Subscription, Window, div, hsla, prelude::FluentBuilder,
    px, rgb, size,
};
use gpui_component::{
    ActiveTheme, Icon, IconName, Sizable, VirtualListScrollHandle, WindowExt,
    button::{Button, ButtonVariants as _},
    dialog::DialogFooter,
    dock::{Panel, PanelEvent, PanelState},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt, DropdownMenu as _, PopupMenu, PopupMenuItem},
    scroll::{ScrollableElement as _, ScrollbarAxis},
    text::TextView,
    tree::TreeItem,
    v_flex, v_virtual_list,
};

use crate::credentials;
use crate::drivers::create_configured_data_source;
use crate::query_session::QuerySessionStore;
use crate::schema_cache;
use crate::ui::activity::ActivityTracker;
use crate::ui::panels::diagram::{DiagramScope, ShowDiagramEvent};
use crate::ui::panels::file_editor::data_editor::ShowDataEditorEvent;
use sqlab_drivers_core::ddl::create_ddl_generator;
use sqlab_drivers_core::{
    ConnectionStatus, DataSourceConfig, DataSourceError, Database, TableKind,
    manager::{DataSourceManager, IntrospectionStatus},
};

pub struct ConnectionPanel {
    manager: Entity<DataSourceManager>,
    activity_tracker: Entity<ActivityTracker>,
    query_sessions: QuerySessionStore,
    focus_handle: FocusHandle,
    expanded_connections: HashSet<String>,
    expanded_nodes: HashSet<String>,
    selected_node: Option<String>,
    selected_connection: Option<String>,
    visible_rows: Vec<ConnectionPanelRow>,
    row_sizes: Rc<Vec<Size<Pixels>>>,
    rows_dirty: bool,
    scroll_handle: VirtualListScrollHandle,
    shown_errors: HashSet<String>,
    shown_credential_errors: HashSet<String>,
    shown_global_credential_error: bool,
}

impl EventEmitter<PanelEvent> for ConnectionPanel {}
impl EventEmitter<ShowDiagramEvent> for ConnectionPanel {}
impl EventEmitter<ShowDataEditorEvent> for ConnectionPanel {}

const DATABASE_OPTIONS: [Database; 5] = [
    Database::Postgres,
    Database::MySql,
    Database::SQLite,
    Database::DuckDB,
    Database::Databend,
];

const CONNECTION_ROW_HEIGHT: Pixels = px(26.);
const MESSAGE_ROW_HEIGHT: Pixels = px(28.);
const MIN_ROW_WIDTH: Pixels = px(240.);

#[derive(Clone)]
enum ConnectionPanelRow {
    Empty,
    Connection {
        config: DataSourceConfig,
        status: ConnectionStatus,
        expanded: bool,
    },
    SchemaNode {
        id: String,
        label: String,
        depth: usize,
        config: DataSourceConfig,
        schema: Rc<sqlab_drivers_core::DatabaseSchema>,
        expanded: bool,
        leaf: bool,
    },
    Message {
        text: &'static str,
    },
}

impl ConnectionPanel {
    pub fn new(
        manager: Entity<DataSourceManager>,
        activity_tracker: Entity<ActivityTracker>,
        query_sessions: QuerySessionStore,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&manager, |this, _, cx| {
            this.rows_dirty = true;
            cx.notify();
        })
        .detach();

        Self {
            manager,
            activity_tracker,
            query_sessions,
            focus_handle: cx.focus_handle(),
            expanded_connections: HashSet::new(),
            expanded_nodes: HashSet::new(),
            selected_node: None,
            selected_connection: None,
            visible_rows: Vec::new(),
            row_sizes: Rc::new(Vec::new()),
            rows_dirty: true,
            scroll_handle: VirtualListScrollHandle::new(),
            shown_errors: HashSet::new(),
            shown_credential_errors: HashSet::new(),
            shown_global_credential_error: false,
        }
    }

    fn open_create_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut config = DataSourceConfig::default();
        config.schema.clear();
        self.open_config_dialog(None, config, window, cx);
    }

    fn open_edit_dialog(
        &mut self,
        old_name: String,
        config: DataSourceConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let recovery_error = self.manager.update(cx, |manager, cx| {
            let error = manager
                .ensure_password_loaded(&old_name, |n| {
                    credentials::load_password(n)
                        .map_err(|e| credentials::recovery_error_message(&e))
                })
                .err();
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
        let form = cx.new(|cx| ConnectionConfigForm::new(config, window, cx));

        let title = if old_name.is_some() {
            "Edit Data Source"
        } else {
            "Add Data Source"
        };

        let view = cx.entity();

        window.open_alert_dialog(cx, move |alert, _window, _cx| {
            let save_config = Rc::new({
                let manager = manager.clone();
                let old_name = old_name.clone();
                let form = form.clone();
                let view = view.clone();
                move |window: &mut Window, cx: &mut App| {
                    Self::save_config_from_dialog(
                        manager.clone(),
                        old_name.clone(),
                        form.clone(),
                        view.clone(),
                        window,
                        cx,
                    )
                }
            });
            let save_config_for_ok = save_config.clone();
            let save_config_for_footer = save_config.clone();
            let form_for_test = form.clone();
            let view_for_test = view.clone();

            alert
                .title(title)
                .width(px(624.))
                .child(v_flex().gap_2().w_full().child(form.clone()))
                .footer(
                    DialogFooter::new()
                        .child(Button::new("cancel").label("Cancel").on_click(
                            move |_, window, cx| {
                                window.close_dialog(cx);
                            },
                        ))
                        .child(
                            Button::new("test-connection")
                                .label("Test Connection")
                                .outline()
                                .on_click(_window.listener_for(
                                    &view_for_test,
                                    move |this, _, _window, cx| {
                                        let config = form_for_test.read(cx).config(cx);
                                        this.test_unsaved_connection(
                                            form_for_test.clone(),
                                            config,
                                            cx,
                                        );
                                    },
                                )),
                        )
                        .child(Button::new("ok").label("OK").primary().on_click(
                            move |_, window, cx| {
                                if save_config_for_footer(window, cx) {
                                    window.close_dialog(cx);
                                }
                            },
                        )),
                )
                .on_ok(move |_, window: &mut Window, cx: &mut App| save_config_for_ok(window, cx))
        });
    }

    fn save_config_from_dialog(
        manager: Entity<DataSourceManager>,
        old_name: Option<String>,
        form: Entity<ConnectionConfigForm>,
        view: Entity<Self>,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let config = form.read(cx).config(cx);
        let name = config.name.clone();

        if validate_connection_config(&config).is_err() {
            return false;
        }

        let duplicate =
            manager.read(cx).configs().iter().any(|config| {
                config.name == name && old_name.as_deref() != Some(config.name.as_str())
            });
        if duplicate {
            return false;
        }

        let is_new = old_name.is_none();
        let save_result = manager.update(cx, |manager, cx| {
            if let Some(old_name) = old_name.as_deref() {
                manager.update(old_name, config.clone());
            } else {
                manager.add(config.clone());
            }
            if let Err(e) = credentials::save_password(&config.name, &config.password) {
                manager.set_credential_error(&config.name, credentials::recovery_error_message(&e));
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
                let db_folder_id = format!("conn:{}:schemas", name);
                panel.expanded_nodes.insert(db_folder_id);
                panel.mark_rows_dirty();
                panel.introspect_schema(config, cx);
            });
        }

        true
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
                            manager.remove(
                                &name,
                                |config| schema_cache::cache_key(config),
                                |key| schema_cache::clear(key).map_err(|e| e.to_string()),
                                |n| credentials::delete_password(n).map_err(|e| e.to_string()),
                            );
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
            let menu_name_for_reconnect = menu_name.clone();
            let menu_name_for_duplicate = menu_name.clone();
            let menu_name_for_delete = menu_name.clone();
            let menu_name_for_configure = menu_name.clone();
            let menu_config_for_configure = menu_config.clone();
            let menu_config_for_diagram = menu_config.clone();
            let view_for_refresh = view.clone();
            let view_for_reconnect = view.clone();
            let view_for_duplicate = view.clone();
            let view_for_delete = view.clone();
            let view_for_configure = view.clone();
            let view_for_diagram = view.clone();

            menu.item(
                PopupMenuItem::new("Show diagram")
                    .icon(IconName::Network)
                    .on_click(window.listener_for(&view_for_diagram, {
                        let menu_config = menu_config_for_diagram.clone();
                        move |this, _, _window, cx| {
                            this.show_diagram(menu_config.clone(), DiagramScope::Database, cx);
                        }
                    })),
            )
            .item(
                PopupMenuItem::new("Reconnect")
                    .icon(IconName::Redo)
                    .on_click(window.listener_for(&view_for_reconnect, {
                        let menu_name = menu_name_for_reconnect.clone();
                        move |this, _, _window, cx| {
                            this.reconnect_connection(menu_name.clone(), cx);
                        }
                    })),
            )
            .item(
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
                                    alert
                                        .title("Keychain Access Error")
                                        .description(error.clone())
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
                    let mut connected = false;
                    let result = async {
                        let mut source = create_configured_data_source(&config)?;
                        source.connect().await?;
                        connected = true;
                        let schema = source.introspect_schema().await?;
                        source.disconnect().await?;
                        schema_cache::save(&cache_key, &name_for_cache, &schema)?;
                        Ok::<_, anyhow::Error>(())
                    }
                    .await;

                    result
                        .map(|_| connected)
                        .map_err(|error| (connected, error))
                })
                .await;

            cx.update_entity(&manager, |manager, cx| {
                match result {
                    Ok(_) => {
                        manager.set_status(&name, ConnectionStatus::Connected);
                        manager.clear_last_error(&name);
                        manager.set_introspection_status(&name, IntrospectionStatus::Cached);
                    }
                    Err((connected, e)) => {
                        let message = e.to_string();
                        eprintln!("Schema introspection failed for {}: {}", name, message);
                        manager.set_status(
                            &name,
                            if connected {
                                ConnectionStatus::Connected
                            } else {
                                ConnectionStatus::Failed
                            },
                        );
                        manager.set_last_error(&name, message);
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
            if let Err(error) = manager.ensure_password_loaded(&config_name, |n| {
                credentials::load_password(n).map_err(|e| credentials::recovery_error_message(&e))
            }) {
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
                    let mut source = create_configured_data_source(&config)?;
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

    fn reconnect_connection(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(config) = self.prepare_connection_operation(name.clone(), cx) else {
            return;
        };

        let manager = self.manager.clone();
        let query_sessions = self.query_sessions.clone();
        let activity_tracker = self.activity_tracker.clone();
        let config_name = name.clone();
        let config_name_for_task = config_name.clone();
        let activity_label = format!("Reconnecting: {}", config_name);
        let activity_id = self
            .activity_tracker
            .update(cx, |tracker, cx| tracker.begin(activity_label, cx));

        cx.spawn(async move |_this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    query_sessions
                        .close_connection_name(config_name_for_task.clone())
                        .await?;
                    let mut source = create_configured_data_source(&config)?;
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

    fn test_unsaved_connection(
        &mut self,
        form: Entity<ConnectionConfigForm>,
        config: DataSourceConfig,
        cx: &mut Context<Self>,
    ) {
        if let Err(message) = validate_connection_test_config(&config) {
            form.update(cx, |form, cx| {
                form.testing_connection = false;
                form.test_status = Some(ConnectionTestStatus::Failed(message));
                cx.notify();
            });
            return;
        }

        form.update(cx, |form, cx| {
            form.testing_connection = true;
            form.test_status = None;
            cx.notify();
        });

        let activity_tracker = self.activity_tracker.clone();
        let activity_label = if config.name.is_empty() {
            "Testing connection".to_string()
        } else {
            format!("Testing connection: {}", config.name)
        };
        let activity_id = self
            .activity_tracker
            .update(cx, |tracker, cx| tracker.begin(activity_label, cx));

        cx.spawn(async move |_this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut source = create_configured_data_source(&config)?;
                    source.connect().await?;
                    source.disconnect().await?;
                    Ok::<(), DataSourceError>(())
                })
                .await;

            cx.update_entity(&form, |form, cx| {
                form.testing_connection = false;
                form.test_status = Some(match result {
                    Ok(()) => ConnectionTestStatus::Succeeded,
                    Err(error) => ConnectionTestStatus::Failed(error.to_string()),
                });
                cx.notify();
            });

            cx.update_entity(&activity_tracker, |tracker, cx| {
                tracker.finish(activity_id, cx);
            });
        })
        .detach();
    }

    fn refresh_schema(&mut self, name: String, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(config) = self.prepare_connection_operation(name, cx) else {
            return;
        };

        self.introspect_schema(config, cx);
    }

    fn prepare_connection_operation(
        &mut self,
        name: String,
        cx: &mut Context<Self>,
    ) -> Option<DataSourceConfig> {
        self.manager.update(cx, |manager, cx| {
            manager.set_active(Some(name.clone()));

            if manager.status(&name) != ConnectionStatus::Connected {
                manager.set_status(&name, ConnectionStatus::Idle);
            }
            manager.clear_last_error(&name);

            if let Err(error) = manager.ensure_password_loaded(&name, |n| {
                credentials::load_password(n).map_err(|e| credentials::recovery_error_message(&e))
            }) {
                manager.set_status(&name, ConnectionStatus::Failed);
                manager.set_last_error(&name, error);
                cx.notify();
                return None;
            }

            let config = manager
                .configs()
                .iter()
                .find(|config| config.name == name)
                .cloned();
            cx.notify();
            config
        })
    }

    fn show_diagram(
        &mut self,
        config: DataSourceConfig,
        scope: DiagramScope,
        cx: &mut Context<Self>,
    ) {
        cx.emit(ShowDiagramEvent { config, scope });
    }

    fn show_data_editor(
        &mut self,
        connection_name: String,
        schema: String,
        table: String,
        cx: &mut Context<Self>,
    ) {
        let Some(config) = self.prepare_connection_operation(connection_name, cx) else {
            return;
        };
        cx.emit(ShowDataEditorEvent {
            config,
            schema,
            table,
        });
    }

    fn toggle_connection_expanded(&mut self, name: &str) {
        if self.expanded_connections.contains(name) {
            self.expanded_connections.remove(name);
        } else {
            self.expanded_connections.insert(name.to_string());
            // Auto-expand the database folder when first opening a connection
            let db_folder_id = format!("conn:{}:schemas", name);
            self.expanded_nodes.insert(db_folder_id);
        }
        self.mark_rows_dirty();
    }

    fn toggle_node_expanded(&mut self, id: &str) {
        if self.expanded_nodes.contains(id) {
            self.expanded_nodes.remove(id);
        } else {
            self.expanded_nodes.insert(id.to_string());
        }
        self.mark_rows_dirty();
    }

    fn build_schema_tree_items(
        connection_name: &str,
        database_name: &str,
        schema: &sqlab_drivers_core::DatabaseSchema,
        expanded: &HashSet<String>,
    ) -> Vec<TreeItem> {
        let mut root_items = Vec::new();

        // Group tables by schema and by kind
        let mut schema_tables: std::collections::HashMap<
            String,
            Vec<&sqlab_drivers_core::TableInfo>,
        > = std::collections::HashMap::new();
        let mut schema_views: std::collections::HashMap<
            String,
            Vec<&sqlab_drivers_core::TableInfo>,
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
            Vec<&sqlab_drivers_core::FunctionInfo>,
        > = std::collections::HashMap::new();
        for func in &schema.functions {
            schema_functions
                .entry(func.schema.clone())
                .or_default()
                .push(func);
        }

        let mut schema_sequences: std::collections::HashMap<
            String,
            Vec<&sqlab_drivers_core::SequenceInfo>,
        > = std::collections::HashMap::new();
        for seq in &schema.sequences {
            schema_sequences
                .entry(seq.schema.clone())
                .or_default()
                .push(seq);
        }

        let mut table_indexes: std::collections::HashMap<
            (String, String),
            Vec<&sqlab_drivers_core::IndexInfo>,
        > = std::collections::HashMap::new();
        for idx in &schema.indexes {
            table_indexes
                .entry((idx.schema.clone(), idx.table_name.clone()))
                .or_default()
                .push(idx);
        }

        let mut schema_triggers: std::collections::HashMap<
            String,
            Vec<&sqlab_drivers_core::TriggerInfo>,
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

                    if !table.columns.is_empty() {
                        let columns_id = format!("{}:columns", table_id);
                        let mut columns_item =
                            TreeItem::new(columns_id.clone(), SharedString::from("Columns"));
                        for col in &table.columns {
                            let mut col_id = format!("{}:col:{}", columns_id, col.name);
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
                            columns_item = columns_item
                                .child(TreeItem::new(col_id, SharedString::from(label)));
                        }
                        if expanded.contains(&columns_id) {
                            columns_item = columns_item.expanded(true);
                        }
                        table_item = table_item.child(columns_item);
                    }

                    if let Some(indexes) =
                        table_indexes.get(&(schema_name.clone(), table.name.clone()))
                    {
                        let idxs_id = format!("{}:indexes", table_id);
                        let mut idxs_item =
                            TreeItem::new(idxs_id.clone(), SharedString::from("Indexes"));
                        for idx in indexes {
                            let idx_id = format!("{}:idx:{}", idxs_id, idx.name);
                            let label = if idx.is_primary {
                                format!("PK ({})", idx.columns.join(", "))
                            } else {
                                format!(
                                    "{}{}",
                                    idx.name,
                                    if idx.is_unique { " (unique)" } else { "" }
                                )
                            };
                            idxs_item =
                                idxs_item.child(TreeItem::new(idx_id, SharedString::from(label)));
                        }
                        if expanded.contains(&idxs_id) {
                            idxs_item = idxs_item.expanded(true);
                        }
                        table_item = table_item.child(idxs_item);
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

            // Routines folder
            if let Some(functions) = schema_functions.get(schema_name) {
                let funcs_id = format!("{}:functions", schema_id);
                let mut funcs_item =
                    TreeItem::new(funcs_id.clone(), SharedString::from("Routines"));
                for func in functions {
                    let func_id = format!("{}:func:{}:{}", schema_id, func.name, func.arguments);
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

        if expanded.contains(&schemas_id) {
            schemas_item = schemas_item.expanded(true);
        }
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

    fn mark_rows_dirty(&mut self) {
        self.rows_dirty = true;
    }

    fn rebuild_visible_rows(&mut self, configs: &[DataSourceConfig], cx: &App) {
        if !self.rows_dirty {
            return;
        }

        let manager = self.manager.read(cx);
        let mut rows = Vec::new();
        if configs.is_empty() {
            rows.push(ConnectionPanelRow::Empty);
        }

        for config in configs {
            let status = manager.status(&config.name);
            let expanded = self.expanded_connections.contains(&config.name);
            rows.push(ConnectionPanelRow::Connection {
                config: config.clone(),
                status,
                expanded,
            });

            if !expanded {
                continue;
            }

            let cache_key = schema_cache::cache_key(config);
            if let Some(schema) = schema_cache::load(&cache_key).ok().flatten() {
                let schema = Rc::new(schema);
                let tree_items = Self::build_schema_tree_items(
                    &config.name,
                    &config.database,
                    &schema,
                    &self.expanded_nodes,
                );
                let mut entries = Vec::new();
                Self::flatten_items(&tree_items, &mut entries, 1);
                rows.extend(entries.into_iter().map(|(item, depth)| {
                    let id = item.id.to_string();
                    ConnectionPanelRow::SchemaNode {
                        label: item.label.to_string(),
                        expanded: item.is_expanded(),
                        leaf: Self::is_leaf_node(&id),
                        id,
                        depth,
                        config: config.clone(),
                        schema: schema.clone(),
                    }
                }));
            } else {
                let text = match manager.introspection_status(&config.name) {
                    IntrospectionStatus::Running => "Refreshing schema...",
                    IntrospectionStatus::Failed => "Schema refresh failed. Click Refresh to retry.",
                    _ => "Schema not cached. Click Refresh to load.",
                };
                rows.push(ConnectionPanelRow::Message { text });
            }
        }

        self.row_sizes = Rc::new(rows.iter().map(Self::row_size).collect());
        self.visible_rows = rows;
        self.rows_dirty = false;
    }

    fn row_size(row: &ConnectionPanelRow) -> Size<Pixels> {
        let width = match row {
            ConnectionPanelRow::Empty => MIN_ROW_WIDTH,
            ConnectionPanelRow::Connection { config, .. } => {
                estimated_row_width(0, config.name.as_str())
            }
            ConnectionPanelRow::SchemaNode { label, depth, .. } => {
                estimated_row_width(*depth, label.as_str())
            }
            ConnectionPanelRow::Message { text, .. } => estimated_row_width(1, text),
        };
        let height = match row {
            ConnectionPanelRow::Message { .. } | ConnectionPanelRow::Empty => MESSAGE_ROW_HEIGHT,
            ConnectionPanelRow::Connection { .. } | ConnectionPanelRow::SchemaNode { .. } => {
                CONNECTION_ROW_HEIGHT
            }
        };
        size(width, height)
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
        } else if id.contains(":idx:") {
            IconName::Search
        } else if id.contains(":schemas")
            || id.ends_with(":tables")
            || id.ends_with(":columns")
            || id.ends_with(":views")
            || id.ends_with(":sequences")
            || id.ends_with(":indexes")
            || id.ends_with(":triggers")
            || id.ends_with(":functions")
        {
            IconName::Folder
        } else if id.contains(":view:") {
            IconName::Inbox
        } else if id.contains(":table:") {
            IconName::File
        } else if id.contains(":func:") {
            IconName::Cpu
        } else if id.contains(":seq:") {
            IconName::ArrowDown
        } else if id.contains(":trig:") {
            IconName::Bell
        } else if id.contains(":schema:") {
            IconName::FolderOpen
        } else {
            IconName::HardDrive
        }
    }

    fn build_visible_entries(&self, configs: &[DataSourceConfig]) -> Vec<String> {
        if !self.rows_dirty {
            return self
                .visible_rows
                .iter()
                .filter_map(|row| match row {
                    ConnectionPanelRow::Connection { config, .. } => {
                        Some(format!("conn:{}", config.name))
                    }
                    ConnectionPanelRow::SchemaNode { id, .. } => Some(id.clone()),
                    ConnectionPanelRow::Empty | ConnectionPanelRow::Message { .. } => None,
                })
                .collect();
        }

        let mut entries = Vec::new();

        for config in configs {
            // Add connection itself
            entries.push(format!("conn:{}", config.name));

            // If expanded, add its schema tree nodes
            if self.expanded_connections.contains(&config.name) {
                let cache_key = schema_cache::cache_key(config);
                if let Some(schema) = schema_cache::load(&cache_key).ok().flatten() {
                    let tree_items = Self::build_schema_tree_items(
                        &config.name,
                        &config.database,
                        &schema,
                        &self.expanded_nodes,
                    );
                    let mut flat = Vec::new();
                    Self::flatten_items(&tree_items, &mut flat, 1);
                    for (item, _) in flat {
                        entries.push(item.id.to_string());
                    }
                }
            }
        }

        entries
    }

    fn select_relative(
        &mut self,
        offset: isize,
        configs: &[DataSourceConfig],
        cx: &mut Context<Self>,
    ) {
        let entries = self.build_visible_entries(configs);
        if entries.is_empty() {
            return;
        }

        // Find current selection index
        let current_ix = if let Some(selected_node) = &self.selected_node {
            entries.iter().position(|id| id == selected_node)
        } else if let Some(selected_conn) = &self.selected_connection {
            entries
                .iter()
                .position(|id| *id == format!("conn:{}", selected_conn))
        } else {
            None
        };

        let next_ix = if let Some(ix) = current_ix {
            if offset < 0 {
                if ix == 0 { entries.len() - 1 } else { ix - 1 }
            } else {
                if ix + 1 >= entries.len() { 0 } else { ix + 1 }
            }
        } else {
            if offset < 0 { entries.len() - 1 } else { 0 }
        };

        let Some(selected_id) = entries.get(next_ix) else {
            return;
        };

        // Determine if it's a connection or a node
        // Connection entries are exactly "conn:name" (2 parts), everything else is a node
        let parts: Vec<&str> = selected_id.split(':').collect();
        if let ["conn", connection_name] = parts.as_slice() {
            // It's a connection
            self.selected_connection = Some((*connection_name).to_string());
            self.selected_node = None;
        } else {
            // It's a schema node (including database folder like conn:name:schemas)
            self.selected_node = Some(selected_id.clone());
            self.selected_connection = None;
        }

        self.scroll_handle.scroll_to_item(
            next_ix,
            if offset < 0 {
                ScrollStrategy::Top
            } else {
                ScrollStrategy::Bottom
            },
        );
        cx.notify();
    }

    fn open_selected_data_editor(
        &mut self,
        configs: &[DataSourceConfig],
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(selected_node) = self.selected_node.clone() else {
            return false;
        };
        let segments = selected_node.split(':').collect::<Vec<_>>();
        let Some(connection_name) = segments
            .first()
            .filter(|segment| **segment == "conn")
            .and_then(|_| segments.get(1))
        else {
            return false;
        };
        let Some(config) = configs
            .iter()
            .find(|config| config.name == *connection_name)
        else {
            return false;
        };
        let cache_key = schema_cache::cache_key(config);
        let Some(schema) = schema_cache::load(&cache_key).ok().flatten() else {
            return false;
        };
        let Some((schema_name, table_name)) =
            Self::data_editor_target_for_node(&selected_node, &schema)
        else {
            return false;
        };

        self.show_data_editor(connection_name.to_string(), schema_name, table_name, cx);
        true
    }

    pub(crate) fn database_icon_path(database: Database) -> &'static str {
        match database {
            Database::Postgres => "icons/postgresql.svg",
            Database::MySql => "icons/mysql.svg",
            Database::SQLite => "icons/sqlite.svg",
            Database::DuckDB => "icons/duckdb.svg",
            Database::Databend => "icons/databend.svg",
        }
    }

    fn node_icon_path(id: &str, _database: Database) -> Option<&'static str> {
        if id.contains(":col:") {
            if id.contains(":pk:") {
                Some("icons/primary_key.svg")
            } else if id.contains(":fk:") {
                Some("icons/column.svg")
            } else {
                Some("icons/column.svg")
            }
        } else if id.ends_with(":tables")
            || id.ends_with(":columns")
            || id.ends_with(":views")
            || id.ends_with(":sequences")
            || id.ends_with(":indexes")
            || id.ends_with(":triggers")
            || id.ends_with(":functions")
        {
            Some("icons/schema.svg")
        } else if id.contains(":idx:") {
            None
        } else if id.contains(":table:") {
            Some("icons/table.svg")
        } else if id.contains(":view:") {
            Some("icons/table.svg")
        } else if id.contains(":schema:") {
            Some("icons/schema.svg")
        } else if id.contains(":schemas") {
            Some("icons/database-server.svg")
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
        if id.ends_with(":tables")
            || id.ends_with(":columns")
            || id.ends_with(":views")
            || id.ends_with(":sequences")
            || id.ends_with(":indexes")
            || id.ends_with(":triggers")
            || id.ends_with(":functions")
            || id.contains(":schemas")
        {
            Some(label.to_string())
        } else if id.contains(":col:") {
            // Column labels include the data type, like "id : bigint NOT NULL".
            label
                .split_once(" : ")
                .map(|(name, _)| name.to_string())
                .or_else(|| label.split_whitespace().next().map(|s| s.to_string()))
        } else if id.contains(":idx:") {
            id.split(":idx:").nth(1).map(|s| s.to_string())
        } else if id.contains(":table:") {
            id.split(":table:").nth(1).map(|s| s.to_string())
        } else if id.contains(":view:") {
            id.split(":view:").nth(1).map(|s| s.to_string())
        } else if id.contains(":seq:") {
            id.split(":seq:").nth(1).map(|s| s.to_string())
        } else if id.contains(":trig:") {
            id.split(":trig:").nth(1).map(|s| s.to_string())
        } else if id.contains(":func:") {
            id.split(":func:").nth(1).map(|s| s.to_string())
        } else if id.contains(":schema:") {
            id.split(":schema:").nth(1).map(|s| s.to_string())
        } else {
            Some(label.to_string())
        }
    }

    fn generate_ddl_for_node(
        node_id: &str,
        _label: &str,
        schema: &sqlab_drivers_core::DatabaseSchema,
    ) -> Option<String> {
        let generator = create_ddl_generator(schema.db_type);

        // Parse node_id segments
        let segments: Vec<&str> = node_id.split(':').collect();

        if node_id.contains(":col:") {
            // conn:name:schema:schema_name:table:table_name:columns:col:col_name[:pk][:fk]
            let col_name_idx = segments.iter().position(|&s| s == "col")?;
            let col_name = segments.get(col_name_idx + 1)?.split(':').next()?;
            let table_name_idx = segments.iter().position(|&s| s == "table")?;
            let table_name = segments.get(table_name_idx + 1)?;
            let schema_name_idx = segments.iter().position(|&s| s == "schema")?;
            let schema_name = segments.get(schema_name_idx + 1)?;

            let table = schema
                .tables
                .iter()
                .find(|t| t.schema == *schema_name && t.name == *table_name)?;
            let col = table.columns.iter().find(|c| c.name == col_name)?;
            return Some(generator.generate_column_ddl(table, col));
        }

        if node_id.contains(":idx:") {
            let idx_name_idx = segments.iter().position(|&s| s == "idx")?;
            let idx_name = segments.get(idx_name_idx + 1)?;
            let schema_name_idx = segments.iter().position(|&s| s == "schema")?;
            let schema_name = segments.get(schema_name_idx + 1)?;
            let table_name = segments
                .iter()
                .position(|&s| s == "table")
                .and_then(|idx| segments.get(idx + 1));

            let idx = schema.indexes.iter().find(|i| {
                i.schema == *schema_name
                    && i.name == **idx_name
                    && table_name
                        .map(|table_name| i.table_name == *table_name)
                        .unwrap_or(true)
            })?;
            return Some(generator.generate_index_ddl(idx));
        }

        if node_id.contains(":table:") {
            let table_name_idx = segments.iter().position(|&s| s == "table")?;
            let table_name = segments.get(table_name_idx + 1)?;
            let schema_name_idx = segments.iter().position(|&s| s == "schema")?;
            let schema_name = segments.get(schema_name_idx + 1)?;

            let table = schema
                .tables
                .iter()
                .find(|t| t.schema == *schema_name && t.name == *table_name)?;
            return Some(generator.generate_table_ddl(schema, table));
        }

        if node_id.contains(":view:") {
            let view_name_idx = segments.iter().position(|&s| s == "view")?;
            let view_name = segments.get(view_name_idx + 1)?.split(" (").next()?;
            let schema_name_idx = segments.iter().position(|&s| s == "schema")?;
            let schema_name = segments.get(schema_name_idx + 1)?;

            let table = schema
                .tables
                .iter()
                .find(|t| t.schema == *schema_name && t.name == view_name)?;
            return Some(generator.generate_view_ddl(schema, table));
        }

        if node_id.contains(":func:") {
            let func_name_idx = segments.iter().position(|&s| s == "func")?;
            let func_name = segments.get(func_name_idx + 1)?;
            let schema_name_idx = segments.iter().position(|&s| s == "schema")?;
            let schema_name = segments.get(schema_name_idx + 1)?;

            let func = schema
                .functions
                .iter()
                .find(|f| f.schema == *schema_name && f.name == **func_name)?;
            return Some(generator.generate_function_ddl(func));
        }

        if node_id.contains(":trig:") {
            let trig_name_idx = segments.iter().position(|&s| s == "trig")?;
            let trig_name = segments.get(trig_name_idx + 1)?;
            let schema_name_idx = segments.iter().position(|&s| s == "schema")?;
            let schema_name = segments.get(schema_name_idx + 1)?;

            let trig = schema
                .triggers
                .iter()
                .find(|t| t.schema == *schema_name && t.name == **trig_name)?;
            return Some(generator.generate_trigger_ddl(trig));
        }

        if node_id.contains(":seq:") {
            let seq_name_idx = segments.iter().position(|&s| s == "seq")?;
            let seq_name = segments.get(seq_name_idx + 1)?;
            let schema_name_idx = segments.iter().position(|&s| s == "schema")?;
            let schema_name = segments.get(schema_name_idx + 1)?;

            let seq = schema
                .sequences
                .iter()
                .find(|s| s.schema == *schema_name && s.name == **seq_name)?;
            return Some(generator.generate_sequence_ddl(seq));
        }

        // Schema node: conn:name:schema:schema_name (no further segments after schema_name)
        if let Some(schema_name_idx) = segments.iter().position(|&s| s == "schema") {
            let schema_name = segments.get(schema_name_idx + 1)?;
            // Ensure this is a leaf schema node (no other object type after it)
            let after_schema = segments.get(schema_name_idx + 2..)?;
            if after_schema.is_empty() {
                let schema_info = schema.schemas.iter().find(|s| s.name == *schema_name)?;
                return Some(generator.generate_schema_ddl(schema_info));
            }
        }

        None
    }

    fn show_quick_documentation(
        &mut self,
        node_id: String,
        _label: String,
        schema: sqlab_drivers_core::DatabaseSchema,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ddl) = Self::generate_ddl_for_node(&node_id, &_label, &schema) else {
            return;
        };
        let title = Self::node_label_for_doc(&node_id, &_label);
        let markdown = format!("```sql\n{}\n```", ddl);
        let markdown_shared: SharedString = markdown.into();
        let title_for_closure = title.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            alert
                .title(title_for_closure.clone())
                .width(px(640.))
                .child(
                    TextView::markdown("quick-doc", markdown_shared.clone())
                        .selectable(true)
                        .text_xs()
                        .max_h(px(480.)),
                )
        });
    }

    fn node_label_for_doc(node_id: &str, label: &str) -> String {
        if node_id.contains(":table:") {
            let table_name = node_id
                .split(":table:")
                .nth(1)
                .map(|s| s.to_string())
                .unwrap_or_else(|| label.to_string());
            let schema_name = node_id
                .split(":schema:")
                .nth(1)
                .map(|s| s.split(':').next().unwrap_or("").to_string())
                .unwrap_or_default();
            if schema_name.is_empty() {
                format!("Documentation: {}", table_name)
            } else {
                format!("Documentation: {}.{}", schema_name, table_name)
            }
        } else if node_id.contains(":view:") {
            let view_name = node_id
                .split(":view:")
                .nth(1)
                .map(|s| s.split(" (").next().unwrap_or(s).to_string())
                .unwrap_or_else(|| label.to_string());
            let schema_name = node_id
                .split(":schema:")
                .nth(1)
                .map(|s| s.split(':').next().unwrap_or("").to_string())
                .unwrap_or_default();
            if schema_name.is_empty() {
                format!("Documentation: {}", view_name)
            } else {
                format!("Documentation: {}.{}", schema_name, view_name)
            }
        } else {
            format!("Documentation: {}", label)
        }
    }

    fn schema_node_context_menu(
        node_id: String,
        _label: String,
        schema: sqlab_drivers_core::DatabaseSchema,
        config: DataSourceConfig,
        view: Entity<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        move |menu, window, _cx| {
            let ddl_node_id = node_id.clone();
            let ddl_label = _label.clone();
            let ddl_schema = schema.clone();
            let diagram_scope = Self::diagram_scope_for_node(&node_id);
            let diagram_config = config.clone();
            let diagram_view = view.clone();
            let data_editor_target = Self::data_editor_target_for_node(&node_id, &schema);
            let data_editor_view = view.clone();
            let data_editor_connection = config.name.clone();

            let is_folder_node = node_id.ends_with(":tables")
                || node_id.ends_with(":columns")
                || node_id.ends_with(":views")
                || node_id.ends_with(":functions")
                || node_id.ends_with(":indexes")
                || node_id.ends_with(":triggers")
                || node_id.ends_with(":sequences")
                || node_id.contains(":schemas");

            menu.item(
                PopupMenuItem::new("Copy Name")
                    .icon(IconName::Copy)
                    .on_click({
                        let name = Self::copyable_name(&node_id, &_label).unwrap_or_default();
                        move |_menu, _window, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(name.clone()));
                        }
                    }),
            )
            .when_some(diagram_scope, |menu, scope| {
                menu.item(
                    PopupMenuItem::new("Show diagram")
                        .icon(IconName::Network)
                        .on_click(window.listener_for(&diagram_view, {
                            let config = diagram_config.clone();
                            move |this, _, _window, cx| {
                                this.show_diagram(config.clone(), scope.clone(), cx);
                            }
                        })),
                )
            })
            .when_some(data_editor_target, |menu, (schema_name, table_name)| {
                menu.item(
                    PopupMenuItem::new("Edit data")
                        .icon(IconName::File)
                        .on_click(window.listener_for(&data_editor_view, {
                            let connection_name = data_editor_connection.clone();
                            move |this, _, _window, cx| {
                                this.show_data_editor(
                                    connection_name.clone(),
                                    schema_name.clone(),
                                    table_name.clone(),
                                    cx,
                                );
                            }
                        })),
                )
            })
            .when(!is_folder_node, |menu| {
                let copy_ddl_node_id = ddl_node_id.clone();
                let copy_ddl_label = ddl_label.clone();
                let copy_ddl_schema = ddl_schema.clone();
                let doc_node_id = ddl_node_id.clone();
                let doc_label = ddl_label.clone();
                let doc_schema = ddl_schema.clone();
                menu.item(
                    PopupMenuItem::new("Generate and copy DDL")
                        .icon(IconName::File)
                        .on_click({
                            move |_menu, _window, cx| {
                                if let Some(ddl) = Self::generate_ddl_for_node(
                                    &copy_ddl_node_id,
                                    &copy_ddl_label,
                                    &copy_ddl_schema,
                                ) {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(ddl));
                                }
                            }
                        }),
                )
                .item(
                    PopupMenuItem::new("Quick documentation")
                        .icon(IconName::Info)
                        .on_click(window.listener_for(&view, move |this, _, window, cx| {
                            this.show_quick_documentation(
                                doc_node_id.clone(),
                                doc_label.clone(),
                                doc_schema.clone(),
                                window,
                                cx,
                            );
                        })),
                )
            })
        }
    }

    fn diagram_scope_for_node(node_id: &str) -> Option<DiagramScope> {
        let segments = node_id.split(':').collect::<Vec<_>>();
        if node_id.ends_with(":schemas") {
            return Some(DiagramScope::Database);
        }

        let schema_name_idx = segments.iter().position(|&segment| segment == "schema")?;
        let schema_name = segments.get(schema_name_idx + 1)?;
        let after_schema = segments.get(schema_name_idx + 2..)?;
        if after_schema.is_empty() {
            return Some(DiagramScope::Schema((*schema_name).to_string()));
        }

        if let Some(table_idx) = segments.iter().position(|&segment| segment == "table") {
            let table_name = segments.get(table_idx + 1)?;
            return Some(DiagramScope::Table {
                schema: (*schema_name).to_string(),
                table: (*table_name).to_string(),
            });
        }

        None
    }

    fn data_editor_target_for_node(
        node_id: &str,
        schema: &sqlab_drivers_core::DatabaseSchema,
    ) -> Option<(String, String)> {
        let segments = node_id.split(':').collect::<Vec<_>>();
        let schema_name_idx = segments.iter().position(|&segment| segment == "schema")?;
        let schema_name = segments.get(schema_name_idx + 1)?;
        let table_idx = segments.iter().position(|&segment| segment == "table")?;
        let table_name = segments.get(table_idx + 1)?;

        schema
            .tables
            .iter()
            .any(|table| {
                table.schema == *schema_name
                    && table.name == *table_name
                    && matches!(table.kind, TableKind::Table)
            })
            .then(|| ((*schema_name).to_string(), (*table_name).to_string()))
    }

    fn render_row(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(row) = self.visible_rows.get(ix).cloned() else {
            return div().into_any_element();
        };

        match row {
            ConnectionPanelRow::Empty => div()
                .p_2()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("No data sources configured.")
                .into_any_element(),
            ConnectionPanelRow::Connection {
                config,
                status,
                expanded,
            } => self.render_connection_row(config, status, expanded, window, cx),
            ConnectionPanelRow::SchemaNode {
                id,
                label,
                depth,
                config,
                schema,
                expanded,
                leaf,
            } => self.render_schema_node_row(id, label, depth, config, schema, expanded, leaf, cx),
            ConnectionPanelRow::Message { text, .. } => div()
                .pl(px(32.))
                .py_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(text)
                .into_any_element(),
        }
    }

    fn render_connection_row(
        &mut self,
        config: DataSourceConfig,
        _status: ConnectionStatus,
        is_expanded: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_selected = self.selected_connection.as_deref() == Some(config.name.as_str());
        let icon_color = if cx.theme().is_dark() {
            cx.theme().muted_foreground
        } else {
            cx.theme().foreground
        };
        let row_name = config.name.clone();
        let menu_name = config.name.clone();
        let menu_config = config.clone();
        let manager = self.manager.clone();
        let row_manager = manager.clone();
        let view = cx.entity();
        let row_name_for_active = row_name.clone();
        let row_name_for_expand = row_name.clone();

        h_flex()
            .id(format!("connection-row-{}", row_name))
            .w_full()
            .px_1()
            .py_0p5()
            .gap_1()
            .rounded(cx.theme().radius)
            .hover(|style| style.bg(cx.theme().accent.opacity(0.1)))
            .when(is_selected, |this| {
                this.bg(if cx.theme().is_dark() {
                    hsla(0.74, 0.45, 0.32, 0.45)
                } else {
                    hsla(0.74, 0.42, 0.70, 0.58)
                })
            })
            .child(
                div()
                    .id(format!("connection-expand-icon-{}", row_name))
                    .size(px(16.))
                    .flex_none()
                    .child(
                        Icon::new(if is_expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
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
            .child(
                div().id(format!("connection-icon-{}", row_name)).child(
                    Icon::new(IconName::File)
                        .path(Self::database_icon_path(config.db_type))
                        .size(px(17.))
                        .text_color(icon_color)
                        .into_any_element(),
                ),
            )
            .child(
                h_flex()
                    .items_center()
                    .overflow_hidden()
                    .id(format!("connection-label-{}", row_name))
                    .child(div().text_base().truncate().child(config.name.clone()))
                    .on_click(cx.listener({
                        let row_manager = row_manager.clone();
                        let row_name = row_name_for_active;
                        move |this, event: &gpui::ClickEvent, _, cx| {
                            this.selected_connection = Some(row_name.clone());
                            this.selected_node = None;

                            if event.click_count() == 2 {
                                let current_active =
                                    row_manager.read(cx).active_name().map(|n| n.to_string());
                                let status = row_manager.read(cx).status(&row_name);
                                if current_active.as_deref() != Some(row_name.as_str())
                                    || status != ConnectionStatus::Connected
                                {
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
            .into_any_element()
    }

    fn render_schema_node_row(
        &mut self,
        id: String,
        label: String,
        depth: usize,
        config: DataSourceConfig,
        schema: Rc<sqlab_drivers_core::DatabaseSchema>,
        is_node_expanded: bool,
        is_leaf: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_selected = self.selected_node.as_deref() == Some(&id);
        let icon = Self::node_icon(&id);
        let icon_path = Self::node_icon_path(&id, config.db_type);
        let id_click = id.clone();
        let id_toggle = id.clone();
        let label_for_menu = label.clone();
        let data_editor_target = Self::data_editor_target_for_node(&id, &schema);
        let data_editor_connection = config.name.clone();
        let view = cx.entity();

        let (name, data_type) = if id.contains(":col:") {
            if let Some(pos) = label.find(" : ") {
                (label[..pos].to_string(), Some(label[pos + 3..].to_string()))
            } else {
                (label.clone(), None)
            }
        } else {
            (label.clone(), None)
        };

        div()
            .id(id.clone())
            .w(px(Self::row_size(&ConnectionPanelRow::SchemaNode {
                id: id.clone(),
                label: label.clone(),
                depth,
                config: config.clone(),
                schema: schema.clone(),
                expanded: is_node_expanded,
                leaf: is_leaf,
            })
            .width
            .as_f32()))
            .min_w_full()
            .py_0p5()
            .px_1()
            .pl(px(12.) * depth as f32 + px(4.))
            .rounded(cx.theme().radius)
            .whitespace_nowrap()
            .when(is_selected, |this| {
                this.bg(if cx.theme().is_dark() {
                    hsla(0.74, 0.45, 0.32, 0.45)
                } else {
                    hsla(0.74, 0.42, 0.70, 0.58)
                })
            })
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
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
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
                                        .text_color(cx.theme().muted_foreground.opacity(0.6))
                                        .child(dt),
                                )
                            }),
                    ),
            )
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                this.selected_node = Some(id_click.clone());
                this.selected_connection = None;
                if event.click_count() == 2 {
                    if let Some((schema_name, table_name)) = data_editor_target.as_ref() {
                        this.show_data_editor(
                            data_editor_connection.clone(),
                            schema_name.clone(),
                            table_name.clone(),
                            cx,
                        );
                        cx.notify();
                        return;
                    }
                }
                if let Some(name) = Self::copyable_name(&id_click, &label) {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(name));
                }
                cx.notify();
            }))
            .context_menu(Self::schema_node_context_menu(
                id.clone(),
                label_for_menu,
                (*schema).clone(),
                config.clone(),
                view.clone(),
            ))
            .into_any_element()
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
        let manager = self.manager.clone();
        self.rebuild_visible_rows(&configs, cx);
        let row_sizes = self.row_sizes.clone();
        let scroll_handle = self.scroll_handle.clone();

        // Header
        let header = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .pl(px(4.))
            .pb_1()
            .p_1()
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
            .into_any_element();

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
            .items_start()
            .track_focus(&self.focus_handle)
            .on_key_down(
                cx.listener(|this, event: &gpui::KeyDownEvent, _window, cx| {
                    match event.keystroke.key.as_str() {
                        "up" => {
                            let configs = this.manager.read(cx).configs().to_vec();
                            this.select_relative(-1, &configs, cx);
                        }
                        "down" => {
                            let configs = this.manager.read(cx).configs().to_vec();
                            this.select_relative(1, &configs, cx);
                        }
                        "right" => {
                            if let Some(id) = &this.selected_node {
                                if !this.expanded_nodes.contains(id) {
                                    this.expanded_nodes.insert(id.clone());
                                    this.mark_rows_dirty();
                                    cx.notify();
                                }
                            } else if let Some(id) = &this.selected_connection {
                                if !this.expanded_connections.contains(id) {
                                    this.expanded_connections.insert(id.clone());
                                    // Auto-expand the database folder when first opening a connection
                                    let db_folder_id = format!("conn:{}:schemas", id);
                                    this.expanded_nodes.insert(db_folder_id);
                                    this.mark_rows_dirty();
                                    cx.notify();
                                }
                            }
                        }
                        "left" => {
                            if let Some(id) = &this.selected_node {
                                if this.expanded_nodes.contains(id) {
                                    this.expanded_nodes.remove(id);
                                    this.mark_rows_dirty();
                                    cx.notify();
                                }
                            } else if let Some(id) = &this.selected_connection {
                                if this.expanded_connections.contains(id) {
                                    this.expanded_connections.remove(id);
                                    this.mark_rows_dirty();
                                    cx.notify();
                                }
                            }
                        }
                        "enter" => {
                            let configs = this.manager.read(cx).configs().to_vec();
                            if this.open_selected_data_editor(&configs, cx) {
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }
                        _ => {}
                    }
                }),
            )
            .child(header)
            .child(
                v_flex()
                    .id("connection-panel-inner")
                    .relative()
                    .flex_1()
                    .w_full()
                    .text_sm()
                    .p_1()
                    .min_w_full()
                    .child(
                        v_virtual_list(
                            cx.entity().clone(),
                            "connection-panel-rows",
                            row_sizes,
                            move |this, visible_range, window, cx| {
                                visible_range
                                    .map(|ix| this.render_row(ix, window, cx))
                                    .collect::<Vec<_>>()
                            },
                        )
                        .track_scroll(&scroll_handle)
                        .size_full(),
                    )
                    .scrollbar(&scroll_handle, ScrollbarAxis::Both),
            )
    }
}

impl Focusable for ConnectionPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

enum ConnectionTestStatus {
    Succeeded,
    Failed(String),
}

struct ConnectionConfigForm {
    selected_db_type: Database,
    advanced_open: bool,
    testing_connection: bool,
    test_status: Option<ConnectionTestStatus>,
    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    database: Entity<InputState>,
    schema: Entity<InputState>,
    query_string: Entity<InputState>,
    connection_url: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl ConnectionConfigForm {
    fn new(config: DataSourceConfig, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let connection_url_value = connection_url_from_config(&config);
        let name = cx.new(|cx| InputState::new(window, cx).default_value(config.name));
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
        let connection_url =
            cx.new(|cx| InputState::new(window, cx).default_value(connection_url_value));

        let mut subscriptions = Vec::new();
        for input in [
            host.clone(),
            port.clone(),
            user.clone(),
            password.clone(),
            database.clone(),
            schema.clone(),
            query_string.clone(),
        ] {
            subscriptions.push(cx.subscribe_in(&input, window, {
                move |this: &mut Self, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.sync_url_from_fields(window, cx);
                    }
                }
            }));
        }

        subscriptions.push(cx.subscribe_in(&connection_url, window, {
            move |this: &mut Self, url, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = url.read(cx).value().to_string();
                    this.apply_connection_url(&value, window, cx);
                }
            }
        }));

        Self {
            selected_db_type: config.db_type,
            advanced_open: false,
            testing_connection: false,
            test_status: None,
            name,
            host,
            port,
            user,
            password,
            database,
            schema,
            query_string,
            connection_url,
            _subscriptions: subscriptions,
        }
    }

    fn config(&self, cx: &App) -> DataSourceConfig {
        let db_type = self.selected_db_type;
        DataSourceConfig {
            name: self.name.read(cx).value().trim().to_string(),
            db_type,
            host: self.host.read(cx).value().trim().to_string(),
            port: self
                .port
                .read(cx)
                .value()
                .trim()
                .parse::<u16>()
                .unwrap_or_else(|_| db_type.default_port()),
            user: self.user.read(cx).value().trim().to_string(),
            password: self.password.read(cx).value().to_string(),
            database: self.database.read(cx).value().trim().to_string(),
            schema: self.schema.read(cx).value().trim().to_string(),
            query_string: self.query_string.read(cx).value().trim().to_string(),
        }
    }

    fn select_database(&mut self, database: Database, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self.selected_db_type;
        if database == previous {
            return;
        }

        let previous_port = previous.default_port().to_string();
        let current_port = self.port.read(cx).value().trim().to_string();
        if current_port == previous_port {
            self.port.update(cx, |input, cx| {
                input.set_value(database.default_port().to_string(), window, cx);
            });
        }

        let previous_schema = previous.default_schema();
        let current_schema = self.schema.read(cx).value().trim().to_string();
        if !current_schema.is_empty() && current_schema == previous_schema {
            self.schema.update(cx, |input, cx| {
                input.set_value(database.default_schema().to_string(), window, cx);
            });
        }

        self.selected_db_type = database;
        self.sync_url_from_fields(window, cx);
        cx.notify();
    }

    fn sync_url_from_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let config = self.config(cx);
        let url = connection_url_from_config(&config);
        self.connection_url.update(cx, |input, cx| {
            if input.value().as_ref() != url {
                input.set_value(url, window, cx);
            }
        });
    }

    fn apply_connection_url(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parsed) = parse_connection_url(value) else {
            return;
        };

        self.selected_db_type = parsed.db_type;
        set_input_value(&self.host, parsed.host, window, cx);
        set_input_value(&self.port, parsed.port.to_string(), window, cx);
        set_input_value(&self.user, parsed.user, window, cx);
        if parsed.password_present {
            set_input_value(&self.password, parsed.password, window, cx);
        }
        set_input_value(&self.database, parsed.database, window, cx);
        set_input_value(&self.schema, parsed.schema, window, cx);
        set_input_value(&self.query_string, parsed.query_string, window, cx);

        let config = self.config(cx);
        let sanitized_url = connection_url_from_config(&config);
        self.connection_url.update(cx, |input, cx| {
            if input.value().as_ref() != sanitized_url {
                input.set_value(sanitized_url, window, cx);
            }
        });

        cx.notify();
    }
}

impl Render for ConnectionConfigForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected_db_type;
        let form = cx.entity();
        let advanced_open = self.advanced_open;
        let testing_connection = self.testing_connection;

        let file_database = matches!(selected, Database::SQLite | Database::DuckDB);

        v_flex()
            .gap_2()
            .child(form_field("Name", Input::new(&self.name)))
            .child(
                div()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .mt_1()
                    .pt_2(),
            )
            .child(form_field(
                "Connection URL",
                Input::new(&self.connection_url),
            ))
            .child(form_field(
                "Type",
                Button::new("database-type-picker")
                    .icon(
                        Icon::new(IconName::File)
                            .path(ConnectionPanel::database_icon_path(selected)),
                    )
                    .label(database_label(selected))
                    .dropdown_caret(true)
                    .w_full()
                    .dropdown_menu(move |menu, window, _cx| {
                        let mut menu = menu;
                        for database in DATABASE_OPTIONS {
                            let form_for_item = form.clone();
                            menu = menu.item(
                                PopupMenuItem::new(database_label(database))
                                    .icon(
                                        Icon::new(IconName::File)
                                            .path(ConnectionPanel::database_icon_path(database)),
                                    )
                                    .checked(database == selected)
                                    .on_click(window.listener_for(
                                        &form_for_item,
                                        move |this, _, window, cx| {
                                            this.select_database(database, window, cx);
                                        },
                                    )),
                            );
                        }
                        menu
                    }),
            ))
            .when(file_database, |this| {
                this.child(form_field("File location", Input::new(&self.database)))
            })
            .when(!file_database, |this| {
                this.child(form_field("Host", Input::new(&self.host)))
                    .child(form_field("Port", Input::new(&self.port)))
                    .child(form_field("User", Input::new(&self.user)))
                    .child(form_field("Password", Input::new(&self.password)))
                    .child(form_field("Database", Input::new(&self.database)))
            })
            .child(
                v_flex()
                    .gap_2()
                    .pt_1()
                    .child(
                        Button::new("advanced-options")
                            .icon(if advanced_open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .label("Advanced options")
                            .ghost()
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.advanced_open = !this.advanced_open;
                                cx.notify();
                            })),
                    )
                    .when(advanced_open, |this| {
                        this.child(form_field("Schema", Input::new(&self.schema)))
                            .child(form_field("Query String", Input::new(&self.query_string)))
                    }),
            )
            .when(testing_connection, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Testing connection..."),
                )
            })
            .when_some(self.test_status.as_ref(), |this, status| match status {
                ConnectionTestStatus::Succeeded => this.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x16a34a))
                        .child("Connection test succeeded."),
                ),
                ConnectionTestStatus::Failed(message) => this.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xef4444))
                        .child(format!("Connection test failed: {}", message)),
                ),
            })
    }
}

fn set_input_value(
    input: &Entity<InputState>,
    value: String,
    window: &mut Window,
    cx: &mut Context<ConnectionConfigForm>,
) {
    input.update(cx, |input, cx| {
        if input.value().as_ref() != value {
            input.set_value(value, window, cx);
        }
    });
}

fn validate_connection_config(config: &DataSourceConfig) -> Result<(), String> {
    if config.name.is_empty() {
        return Err("Connection name is required.".to_string());
    }
    validate_connection_test_config(config)
}

fn validate_connection_test_config(config: &DataSourceConfig) -> Result<(), String> {
    if config.database.is_empty() {
        return Err(
            if matches!(config.db_type, Database::SQLite | Database::DuckDB) {
                "File location is required.".to_string()
            } else {
                "Database is required.".to_string()
            },
        );
    }
    if !matches!(config.db_type, Database::SQLite | Database::DuckDB) {
        if config.host.is_empty() {
            return Err("Host is required.".to_string());
        }
        if config.user.is_empty() {
            return Err("User is required.".to_string());
        }
    }

    Ok(())
}

fn estimated_row_width(depth: usize, label: &str) -> Pixels {
    let indent = 12. * depth as f32 + 48.;
    let text = label.chars().count() as f32 * 7.5;
    px((indent + text).max(MIN_ROW_WIDTH.as_f32()))
}

struct ParsedConnectionUrl {
    db_type: Database,
    host: String,
    port: u16,
    user: String,
    password: String,
    password_present: bool,
    database: String,
    schema: String,
    query_string: String,
}

fn connection_url_from_config(config: &DataSourceConfig) -> String {
    if matches!(config.db_type, Database::SQLite | Database::DuckDB) {
        let database = if config.database.is_empty() {
            String::new()
        } else {
            percent_encode_path(&config.database)
        };
        let scheme = match config.db_type {
            Database::SQLite => "sqlite",
            Database::DuckDB => "duckdb",
            _ => unreachable!(),
        };
        return format!("{scheme}://{}", database);
    }

    let scheme = match config.db_type {
        Database::Postgres => "postgresql",
        Database::MySql => "mysql",
        Database::Databend => "databend",
        Database::SQLite | Database::DuckDB => unreachable!(),
    };
    let auth = if config.user.is_empty() {
        String::new()
    } else {
        format!("{}@", percent_encode_component(&config.user))
    };
    let mut query_parts = Vec::new();
    if !config.schema.is_empty() {
        query_parts.push(format!(
            "schema={}",
            percent_encode_component(&config.schema)
        ));
    }
    if !config.query_string.trim().is_empty() {
        query_parts.push(query_string_for_url(&config.query_string));
    }
    let query = if query_parts.is_empty() {
        String::new()
    } else {
        format!("?{}", query_parts.join("&"))
    };

    format!(
        "{}://{}{}:{}/{}{}",
        scheme,
        auth,
        config.host,
        config.port,
        percent_encode_component(&config.database),
        query
    )
}

fn parse_connection_url(value: &str) -> Option<ParsedConnectionUrl> {
    let value = value.trim();
    let (scheme, rest) = value.split_once("://")?;
    let db_type = match scheme.to_ascii_lowercase().as_str() {
        "postgres" | "postgresql" => Database::Postgres,
        "mysql" => Database::MySql,
        "sqlite" | "sqlite3" => Database::SQLite,
        "duckdb" => Database::DuckDB,
        "databend" | "databend+http" | "databend+https" => Database::Databend,
        _ => return None,
    };

    if matches!(db_type, Database::SQLite | Database::DuckDB) {
        let (path, query) = split_query(rest);
        let (schema, query_string) = parse_url_query(db_type, query);
        return Some(ParsedConnectionUrl {
            db_type,
            host: String::new(),
            port: db_type.default_port(),
            user: String::new(),
            password: String::new(),
            password_present: false,
            database: percent_decode(path),
            schema,
            query_string,
        });
    }

    let (without_query, query) = split_query(rest);
    let (authority, path) = without_query.split_once('/').unwrap_or((without_query, ""));
    let (auth, host_port) = authority
        .rsplit_once('@')
        .map(|(auth, host_port)| (Some(auth), host_port))
        .unwrap_or((None, authority));
    let (user, password, password_present) = auth
        .map(|auth| {
            auth.split_once(':')
                .map(|(user, password)| (percent_decode(user), percent_decode(password), true))
                .unwrap_or_else(|| (percent_decode(auth), String::new(), false))
        })
        .unwrap_or_default();
    let (host, port) = parse_host_port(host_port, db_type.default_port());
    let (schema, query_string) = parse_url_query(db_type, query);

    Some(ParsedConnectionUrl {
        db_type,
        host,
        port,
        user,
        password,
        password_present,
        database: percent_decode(path),
        schema,
        query_string,
    })
}

fn split_query(value: &str) -> (&str, &str) {
    value.split_once('?').unwrap_or((value, ""))
}

fn parse_host_port(value: &str, default_port: u16) -> (String, u16) {
    if let Some(rest) = value.strip_prefix('[') {
        if let Some((host, port_part)) = rest.split_once("]:") {
            return (
                host.to_string(),
                port_part.parse::<u16>().unwrap_or(default_port),
            );
        }
    }
    value
        .rsplit_once(':')
        .filter(|(_, port)| port.chars().all(|ch| ch.is_ascii_digit()))
        .map(|(host, port)| {
            (
                host.to_string(),
                port.parse::<u16>().unwrap_or(default_port),
            )
        })
        .unwrap_or_else(|| (value.to_string(), default_port))
}

fn parse_url_query(db_type: Database, query: &str) -> (String, String) {
    let mut schema = String::new();
    let mut options = Vec::new();
    for part in query.split('&').filter(|part| !part.is_empty()) {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        let decoded_key = percent_decode(key);
        let decoded_value = percent_decode(value);
        if decoded_key.eq_ignore_ascii_case("schema") {
            schema = decoded_value;
        } else {
            options.push(format!("{}={}", decoded_key, decoded_value));
        }
    }

    let separator = if db_type == Database::Postgres {
        " "
    } else {
        "&"
    };
    (schema, options.join(separator))
}

fn query_string_for_url(query_string: &str) -> String {
    let trimmed = query_string.trim().trim_start_matches('?');
    if trimmed.contains('&') {
        trimmed.to_string()
    } else {
        trimmed.split_whitespace().collect::<Vec<_>>().join("&")
    }
}

fn percent_encode_component(value: &str) -> String {
    percent_encode(value, false)
}

fn percent_encode_path(value: &str) -> String {
    percent_encode(value, true)
}

fn percent_encode(value: &str, preserve_slash: bool) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric()
            || matches!(ch, '-' | '.' | '_' | '~')
            || (preserve_slash && ch == '/')
        {
            encoded.push(ch);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn percent_decode(value: &str) -> String {
    let mut bytes = Vec::new();
    let mut ix = 0;
    let raw = value.as_bytes();
    while let Some(byte) = raw.get(ix) {
        if *byte == b'%' {
            if let Some(encoded) = raw.get(ix + 1..ix + 3)
                && let Ok(hex) = std::str::from_utf8(encoded)
            {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    bytes.push(byte);
                    ix += 3;
                    continue;
                }
            }
        }
        bytes.push(*byte);
        ix += 1;
    }
    String::from_utf8_lossy(&bytes).replace('+', " ")
}

fn form_field(label: &'static str, input: impl IntoElement) -> impl IntoElement {
    h_flex()
        .w_full()
        .gap_2()
        .items_center()
        .child(div().w(px(96.)).flex_none().text_xs().child(label))
        .child(div().flex_1().child(input))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_parses_postgres_connection_url() {
        let config = DataSourceConfig {
            name: "local".into(),
            db_type: Database::Postgres,
            host: "localhost".into(),
            port: 5432,
            user: "app user".into(),
            password: "p@ss".into(),
            database: "app".into(),
            schema: "analytics".into(),
            query_string: "sslmode=require connect_timeout=5".into(),
        };

        let url = connection_url_from_config(&config);
        assert_eq!(
            url,
            "postgresql://app%20user@localhost:5432/app?schema=analytics&sslmode=require&connect_timeout=5"
        );

        let parsed = parse_connection_url(&url).unwrap();
        assert_eq!(parsed.db_type, Database::Postgres);
        assert_eq!(parsed.user, "app user");
        assert_eq!(parsed.password, "");
        assert!(!parsed.password_present);
        assert_eq!(parsed.schema, "analytics");
        assert_eq!(parsed.query_string, "sslmode=require connect_timeout=5");
    }

    #[test]
    fn parses_password_from_url_but_does_not_reflect_it_back() {
        let url_with_password =
            "postgresql://myuser:secret123@db.example.com:5432/mydb?schema=public";
        let parsed = parse_connection_url(url_with_password).unwrap();
        assert_eq!(parsed.db_type, Database::Postgres);
        assert_eq!(parsed.user, "myuser");
        assert_eq!(parsed.password, "secret123");
        assert!(parsed.password_present);
        assert_eq!(parsed.host, "db.example.com");
        assert_eq!(parsed.port, 5432);
        assert_eq!(parsed.database, "mydb");
        assert_eq!(parsed.schema, "public");

        let config = DataSourceConfig {
            name: "test".into(),
            db_type: Database::Postgres,
            host: parsed.host,
            port: parsed.port,
            user: parsed.user,
            password: parsed.password,
            database: parsed.database,
            schema: parsed.schema,
            query_string: parsed.query_string,
        };

        let sanitized_url = connection_url_from_config(&config);
        assert!(!sanitized_url.contains("secret123"));
        assert_eq!(
            sanitized_url,
            "postgresql://myuser@db.example.com:5432/mydb?schema=public"
        );
    }

    #[test]
    fn parses_explicit_empty_password_from_connection_url() {
        let parsed = parse_connection_url("postgresql://myuser:@db.example.com:5432/mydb").unwrap();

        assert_eq!(parsed.user, "myuser");
        assert_eq!(parsed.password, "");
        assert!(parsed.password_present);
        assert_eq!(parsed.host, "db.example.com");
    }

    #[test]
    fn preserves_sqlite_absolute_paths_in_connection_url() {
        let config = DataSourceConfig {
            db_type: Database::SQLite,
            database: "/tmp/app data.sqlite".into(),
            ..DataSourceConfig::default()
        };

        let url = connection_url_from_config(&config);
        assert_eq!(url, "sqlite:///tmp/app%20data.sqlite");
        assert_eq!(
            parse_connection_url(&url).unwrap().database,
            "/tmp/app data.sqlite"
        );
    }

    #[test]
    fn validates_required_connection_fields_for_remote_databases() {
        let config = DataSourceConfig {
            name: "remote".into(),
            db_type: Database::Postgres,
            host: String::new(),
            database: "app".into(),
            ..DataSourceConfig::default()
        };

        assert_eq!(
            validate_connection_config(&config).unwrap_err(),
            "Host is required."
        );

        let config = DataSourceConfig {
            host: "localhost".into(),
            ..config
        };
        assert_eq!(
            validate_connection_config(&config).unwrap_err(),
            "User is required."
        );
    }

    #[test]
    fn validates_file_location_for_file_databases() {
        let config = DataSourceConfig {
            name: "local".into(),
            db_type: Database::SQLite,
            database: String::new(),
            ..DataSourceConfig::default()
        };

        assert_eq!(
            validate_connection_config(&config).unwrap_err(),
            "File location is required."
        );
    }

    #[test]
    fn connection_test_validation_does_not_require_name() {
        let config = DataSourceConfig {
            name: String::new(),
            db_type: Database::Postgres,
            host: "localhost".into(),
            user: "app".into(),
            database: "app".into(),
            ..DataSourceConfig::default()
        };

        assert!(validate_connection_test_config(&config).is_ok());
        assert_eq!(
            validate_connection_config(&config).unwrap_err(),
            "Connection name is required."
        );
    }
}

fn database_label(database: Database) -> &'static str {
    match database {
        Database::Postgres => "PostgreSQL",
        Database::MySql => "MySQL",
        Database::SQLite => "SQLite",
        Database::DuckDB => "DuckDB",
        Database::Databend => "Databend",
    }
}

fn table_kind_label(kind: &TableKind) -> &'static str {
    match kind {
        TableKind::Table => "table",
        TableKind::View => "view",
        TableKind::MaterializedView => "materialized view",
        TableKind::ForeignTable => "foreign table",
    }
}
