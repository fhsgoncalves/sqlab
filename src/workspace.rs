use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, Window, actions, div, prelude::FluentBuilder, px,
};
use gpui_component::ActiveTheme;
use gpui_component::{
    IconName, Root, Sizable, TitleBar, WindowExt,
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockItem},
    h_flex,
    spinner::Spinner,
    v_flex,
};

use crate::data_source::manager::DataSourceManager;
use crate::data_source::{ConnectionStatus, DataSourceError, QueryResult, create_data_source};
use crate::ui::activity::ActivityTracker;
use crate::ui::panels::bottom_panel::{BottomPanel, BottomPanelMode, ToggleBottomPanelMode};
use crate::ui::panels::connection::ConnectionPanel;
use crate::ui::panels::file_editor::{
    EditorTabs, ExecuteQuery, QueryChoice, QuerySelected, QuerySelector, SaveFile,
};
use crate::ui::panels::file_search::{FileSearch, FileSearchEvent, ToggleFileSearch};
use crate::ui::panels::file_tree::{FileTreePanel, OpenFileEvent, RootChangedEvent};
use crate::ui::panels::result::ResultPanel;
use crate::ui::panels::terminal::TerminalPanel;

actions!(workspace, [OpenFolder]);

pub struct Workspace {
    file_tree_panel: Entity<FileTreePanel>,
    file_search: Entity<FileSearch>,
    dock_area: Entity<DockArea>,
    editor_tabs: Entity<EditorTabs>,
    bottom_panel: Entity<BottomPanel>,
    data_source_manager: Entity<DataSourceManager>,
    activity_tracker: Entity<ActivityTracker>,
    focus_handle: FocusHandle,
    terminal_panel: Entity<TerminalPanel>,
}

impl Workspace {
    pub fn new(
        root_path: PathBuf,
        initial_file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let dock_area = cx.new(|cx| DockArea::new("main-dock", None, window, cx));

        let file_tree_panel = cx.new(|cx| FileTreePanel::new(root_path.clone(), window, cx));
        let file_search = cx.new(|cx| FileSearch::new(root_path.clone(), window, cx));
        let data_source_manager = cx.new(|_cx| {
            DataSourceManager::load().unwrap_or_else(|e| {
                eprintln!("failed to load data source config: {}", e);
                DataSourceManager::empty()
            })
        });
        let activity_tracker = cx.new(|_cx| ActivityTracker::new());

        cx.observe(&activity_tracker, |_, _, cx| {
            cx.notify();
        })
        .detach();

        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.save_open_editors(cx);
            }
        })
        .detach();

        // Subscribe to file open events from the file tree
        cx.subscribe_in(
            &file_tree_panel,
            window,
            |this, _file_tree, event: &OpenFileEvent, window, cx| {
                this.open_file(event.path.clone(), window, cx);
            },
        )
        .detach();

        // Subscribe to root changed events to clear editor tabs
        let file_search_for_root = file_search.clone();
    cx.subscribe_in(
        &file_tree_panel,
        window,
        move |this, _file_tree, _event: &RootChangedEvent, _window, cx| {
            this.editor_tabs.update(cx, |tabs, cx| {
                tabs.clear_tabs(cx);
            });
            let root = this.file_tree_panel.read(cx).root().clone();
            this.terminal_panel.update(cx, |terminal, _cx| {
                terminal.set_working_directory(root.clone());
            });
            file_search_for_root.update(cx, |search, cx| {
                search.set_root(root, cx);
            });
        },
    )
        .detach();

        // Track recently opened files from file tree
        let file_search_for_recent = file_search.clone();
        cx.subscribe_in(
            &file_tree_panel,
            window,
            move |_: &mut Workspace, _file_tree, event: &OpenFileEvent, _window, cx| {
                file_search_for_recent.update(cx, |search, cx| {
                    search.add_recent(event.path.clone(), cx);
                });
            },
        )
        .detach();

        let weak_dock_area = dock_area.downgrade();

        let editor_tabs = cx.new(|cx| {
            let mut tabs = EditorTabs::new(data_source_manager.clone(), window, cx);
            tabs.set_dock_area(weak_dock_area.clone());
            tabs
        });

        // Subscribe to file search results (after editor_tabs is created)
        let editor_tabs_for_focus = editor_tabs.clone();
        cx.subscribe_in(&file_search, window, {
            move |this: &mut Workspace, _file_search, event: &FileSearchEvent, window, cx| {
                match event {
                    FileSearchEvent::OpenFile(path) => {
                        this.open_file(path.clone(), window, cx);
                        // Defer focus to active editor's inner input after render cycle
                        let editor_tabs = editor_tabs_for_focus.clone();
                        cx.defer_in(window, move |_, window, cx| {
                            if let Some(editor) = editor_tabs.read(cx).active_editor() {
                                let input_focus = editor.read(cx).editor_focus_handle(cx);
                                window.focus(&input_focus, cx);
                            }
                        });
                    }
                    FileSearchEvent::Closed => {
                        // Restore focus to active editor when search is closed
                        let editor_tabs = editor_tabs_for_focus.clone();
                        cx.defer_in(window, move |_, window, cx| {
                            if let Some(editor) = editor_tabs.read(cx).active_editor() {
                                let input_focus = editor.read(cx).editor_focus_handle(cx);
                                window.focus(&input_focus, cx);
                            }
                        });
                    }
                }
            }
        })
        .detach();

        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        // Set up left dock: file tree (panel mode to avoid title bar)
        let left_panels = DockItem::panel(Arc::new(file_tree_panel.clone()));

        // Set up center dock with our custom editor tabs
        let center_panels = DockItem::panel(Arc::new(editor_tabs.clone()));

        // Set up right dock: database connections
        let database_panel = cx.new(|cx| {
            ConnectionPanel::new(
                data_source_manager.clone(),
                activity_tracker.clone(),
                window,
                cx,
            )
        });
        let right_panels = DockItem::panel(Arc::new(database_panel));

        let results_panel = cx.new(|cx| {
            let mut panel = ResultPanel::new(activity_tracker.clone(), window, cx);
            panel.set_dock_area(weak_dock_area.clone());
            panel
        });

    let terminal_panel = cx.new(|cx| {
        let mut panel = TerminalPanel::new(root_path.clone(), window, cx);
        panel.set_dock_area(weak_dock_area.clone());
        panel
    });

    let bottom_panel = cx.new(|cx| {
        let mut panel = BottomPanel::new(results_panel, terminal_panel.clone(), cx);
            panel.set_dock_area(weak_dock_area.clone(), cx);
            panel
        });

        // Set up bottom dock: wrapper panel
        let bottom_panels = DockItem::panel(Arc::new(bottom_panel.clone()));

        dock_area.update(cx, |dock_area, cx| {
            dock_area.set_center(center_panels, window, cx);
            dock_area.set_left_dock(left_panels, Some(px(240.)), true, window, cx);
            dock_area.set_right_dock(right_panels, Some(px(260.)), true, window, cx);
            dock_area.set_bottom_dock(bottom_panels, Some(px(200.)), true, window, cx);
            dock_area.set_dock_collapsible(
                gpui::Edges {
                    left: true,
                    bottom: true,
                    right: true,
                    ..Default::default()
                },
                window,
                cx,
            );
        });

        let mut this = Self {
            file_tree_panel,
            file_search,
            dock_area,
            editor_tabs,
            bottom_panel,
            data_source_manager: data_source_manager.clone(),
            activity_tracker,
            focus_handle,
            terminal_panel,
        };

        cx.subscribe_in(
            &this.terminal_panel,
            window,
            |this, _terminal, _event: &gpui_component::dock::PanelEvent, _window, cx| {
                let sessions_count = this.terminal_panel.read(cx).sessions_count();
                if sessions_count == 0 {
                    this.bottom_panel.update(cx, |panel, cx| {
                        panel.set_mode(BottomPanelMode::Results, cx);
                    });
                }
            },
        )
        .detach();

        if let Some(file) = initial_file {
            this.open_file(file, window, cx);
        }

        this
    }

    fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_tabs.update(cx, |editor_tabs, cx| {
            editor_tabs.open_file(path, window, cx);
        });
    }

    fn on_open_folder(&mut self, _: &OpenFolder, _window: &mut Window, cx: &mut Context<Self>) {
        let options = gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Folder".into()),
        };
        let rx = cx.prompt_for_paths(options);
        let file_tree = self.file_tree_panel.clone();
        cx.spawn(async move |_this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(path) = paths.first() {
                    cx.update_entity(&file_tree, |tree, cx| {
                        tree.set_root(path.clone(), cx);
                    });
                }
            }
        })
        .detach();
    }

    fn on_save_file(&mut self, _: &SaveFile, _window: &mut Window, cx: &mut Context<Self>) {
        self.save_active_editor(cx);
    }

    fn save_active_editor(&mut self, cx: &mut Context<Self>) {
        self.editor_tabs.update(cx, |tabs, cx| {
            if let Some(editor) = tabs.active_editor() {
                editor.update(cx, |editor, cx| {
                    editor.save(cx);
                });
            }
        });
    }

    fn save_open_editors(&mut self, cx: &mut Context<Self>) {
        self.editor_tabs.update(cx, |tabs, cx| {
            tabs.save_all(cx);
        });
    }

    fn on_execute_query(&mut self, _: &ExecuteQuery, window: &mut Window, cx: &mut Context<Self>) {
        let Some((selected, active_queries)) = self
            .editor_tabs
            .read(cx)
            .active_editor()
            .map(|editor| editor.read(cx).query_context(cx))
        else {
            window.open_alert_dialog(cx, |alert, _, _| {
                alert
                    .title("No Active Editor")
                    .child("Open a SQL file before executing a query.")
            });
            return;
        };

        let queries: Vec<QueryChoice> = if !selected.trim().is_empty() {
            vec![QueryChoice {
                query: selected.clone(),
                range: None,
            }]
        } else {
            active_queries
                .into_iter()
                .map(|query| QueryChoice {
                    query: query.text.clone(),
                    range: Some(query),
                })
                .collect()
        };

        if queries.is_empty() {
            window.open_alert_dialog(cx, |alert, _, _| {
                alert
                    .title("No Query Detected")
                    .child("Place the cursor inside a SQL statement or select query text.")
            });
            return;
        }

        if queries.len() == 1 {
            self.execute_single_query(queries[0].query.clone(), window, cx);
            return;
        }

        let selector = cx.new(|cx| QuerySelector::new(queries, cx));
        cx.subscribe_in(
            &selector,
            window,
            |this, _selector, event: &QuerySelected, window, cx| {
                let active_editor = this
                    .editor_tabs
                    .update(cx, |tabs, _cx| tabs.active_editor().cloned());
                if let Some(editor) = active_editor {
                    editor.update(cx, |editor, cx| {
                        editor.override_query_decoration(event.choice.range.clone(), cx);
                    });
                }

                if event.confirmed {
                    window.close_dialog(cx);
                    this.execute_single_query(event.choice.query.clone(), window, cx);
                }
            },
        )
        .detach();

        window.open_alert_dialog(cx, {
            let selector = selector.clone();
            move |alert, _window, _cx| {
                alert
                    .title("Choose Query")
                    .child(selector.clone())
                    .footer(div())
                    .close_button(true)
            }
        });
        window.focus(&selector.read(cx).focus_handle(cx), cx);
    }

    fn execute_single_query(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(config) = self.data_source_manager.read(cx).active_config().cloned() else {
            window.open_alert_dialog(cx, |alert, _, _| {
                alert
                    .title("No Active Connection")
                    .child("Activate a database connection before running queries.")
            });
            return;
        };

        let results_panel = self.bottom_panel.read(cx).results_panel().clone();
        let bottom_panel = self.bottom_panel.clone();
        let data_source_manager = self.data_source_manager.clone();
        let activity_tracker = self.activity_tracker.clone();
        let config_for_result = config.clone();
        let config_name = config.name.clone();
        let activity_id = self
            .activity_tracker
            .update(cx, |tracker, cx| tracker.begin("Running query", cx));

        cx.spawn(async move |_this, cx| {
            let query_for_task = query.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut source = create_data_source(&config)?;
                    source.connect().await?;
                    let result = source.execute_query(&query_for_task).await;
                    source.disconnect().await?;
                    result
                })
                .await;

            let (result, succeeded, connection_failed) = match result {
                Ok(result) => (result, true, false),
                Err(error) => {
                    let is_conn_fail = matches!(error, DataSourceError::ConnectionFailed(_));
                    (error_result(error), false, is_conn_fail)
                }
            };

            cx.update_entity(&results_panel, |panel, cx| {
                panel.set_result(query, result, succeeded, Some(config_for_result), cx);
            });

            cx.update_entity(&bottom_panel, |panel, cx| {
                panel.set_mode(BottomPanelMode::Results, cx);
            });

            cx.update_entity(&activity_tracker, |tracker, cx| {
                tracker.finish(activity_id, cx);
            });

            cx.update_entity(&data_source_manager, move |manager, cx| {
                if connection_failed {
                    manager.set_status(&config_name, ConnectionStatus::Failed);
                } else {
                    manager.set_status(&config_name, ConnectionStatus::Connected);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn on_toggle_file_search(
        &mut self,
        _: &ToggleFileSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.file_search.update(cx, |search, cx| {
            search.toggle(window, cx);
        });
    }

    fn on_toggle_bottom_panel_mode(
        &mut self,
        _action: &ToggleBottomPanelMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut is_open = false;
        self.dock_area.update(cx, |dock_area, cx| {
            is_open = dock_area.is_dock_open(
                gpui_component::dock::DockPlacement::Bottom,
                cx,
            );
        });

        if is_open {
            self.bottom_panel.update(cx, |panel, cx| {
                let new_mode = match panel.mode() {
                    BottomPanelMode::Terminal => BottomPanelMode::Results,
                    BottomPanelMode::Results => BottomPanelMode::Terminal,
                };
                panel.set_mode(new_mode, cx);
                window.focus(&panel.focus_handle(cx), cx);
            });
            return;
        }

        self.bottom_panel.update(cx, |panel, cx| {
            panel.set_mode(BottomPanelMode::Terminal, cx);
        });

        self.terminal_panel.update(cx, |panel, cx| {
            panel.ensure_has_tab(window, cx);
        });

        self.bottom_panel.update(cx, |panel, cx| {
            window.focus(&panel.focus_handle(cx), cx);
        });

        self.dock_area.update(cx, |dock_area, cx| {
            dock_area.toggle_dock(
                gpui_component::dock::DockPlacement::Bottom,
                window,
                cx,
            );
        });
    }

    fn render_bottom_bar(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active_connection = self
            .data_source_manager
            .read(cx)
            .active_name()
            .map(|name| {
                let status = self.data_source_manager.read(cx).status(name);
                let status = match status {
                    ConnectionStatus::Idle => "idle",
                    ConnectionStatus::Connected => "connected",
                    ConnectionStatus::Failed => "failed",
                };
                format!("{} ({})", name, status)
            })
            .unwrap_or_else(|| "No active connection".into());

        let (is_busy, activity_label, activity_count) = {
            let tracker = self.activity_tracker.read(cx);
            (
                tracker.is_busy(),
                tracker.label().to_string(),
                tracker.count(),
            )
        };
        let activity_label = if activity_count > 1 {
            format!("{} (+{})", activity_label, activity_count - 1)
        } else {
            activity_label
        };

        let bottom_panel_mode = self.bottom_panel.read(cx).mode();
        let is_terminal_active = bottom_panel_mode == BottomPanelMode::Terminal;
        let is_dock_open = self.dock_area.read(cx).is_dock_open(gpui_component::dock::DockPlacement::Bottom, cx);

        h_flex()
            .id("workspace-bottom-bar")
            .h(px(24.))
            .flex_none()
            .items_center()
            .justify_between()
            .px_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().tab_bar)
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(div().truncate().child(active_connection))
            .child(div().flex_1())
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child({
                        let btn = Button::new("terminal-toggle")
                            .icon(IconName::Inbox) // Using Inbox for Terminal
                            .xsmall()
                            .ghost()
                            .tooltip("Terminal");
                        
                        let btn = if is_terminal_active && is_dock_open {
                            btn.text_color(cx.theme().accent)
                        } else {
                            btn
                        };

                        btn.on_click(cx.listener(|this, _, window, cx| {
                                let mut is_open = false;
                                this.dock_area.update(cx, |dock_area, cx| {
                                    is_open = dock_area.is_dock_open(gpui_component::dock::DockPlacement::Bottom, cx);
                                });

                                this.bottom_panel.update(cx, |panel, cx| {
                                    let mode = panel.mode();
                                    if is_open {
                                        if mode == BottomPanelMode::Terminal {
                                            // Switch to results and keep dock open
                                            panel.set_mode(BottomPanelMode::Results, cx);
                                        } else {
                                            // Switch to terminal
                                            panel.set_mode(BottomPanelMode::Terminal, cx);
                                        }
                                    } else {
                                        // Open dock and set terminal
                                        panel.set_mode(BottomPanelMode::Terminal, cx);
                                        this.dock_area.update(cx, |dock_area, cx| {
                                            dock_area.toggle_dock(gpui_component::dock::DockPlacement::Bottom, window, cx);
                                        });
                                    }
                                });

                                this.terminal_panel.update(cx, |panel, cx| {
                                    panel.ensure_has_tab(window, cx);
                                });

                                this.bottom_panel.update(cx, |panel, cx| {
                                    window.focus(&panel.focus_handle(cx), cx);
                                });
                            }))
                    })
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(div().child(activity_label))
                            .child(if is_busy {
                                Spinner::new()
                                    .xsmall()
                                    .color(cx.theme().muted_foreground)
                                    .into_any_element()
                            } else {
                                div().size(px(12.)).into_any_element()
                            }),
                    )
            )
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_dark = cx.theme().is_dark();
        let theme_icon = if is_dark {
            IconName::Sun
        } else {
            IconName::Moon
        };

        let is_file_search_visible = self.file_search.read(cx).is_visible();

        v_flex()
            .id("workspace")
            .size_full()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_open_folder))
            .on_action(cx.listener(Self::on_save_file))
            .on_action(cx.listener(Self::on_execute_query))
            .on_action(cx.listener(Self::on_toggle_file_search))
            .on_action(cx.listener(Self::on_toggle_bottom_panel_mode))
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .child(
                            div()
                                .child("zql")
                                .text_color(cx.theme().foreground)
                                .font_weight(gpui::FontWeight::MEDIUM),
                        )
                        .child(
                            Button::new("theme-toggle")
                                .icon(theme_icon)
                                .small()
                                .ghost()
                                .tooltip(if is_dark {
                                    "Switch to Light"
                                } else {
                                    "Switch to Dark"
                                })
                                .on_click(move |_event, window, cx| {
                                    let new_mode = if is_dark {
                                        gpui_component::ThemeMode::Light
                                    } else {
                                        gpui_component::ThemeMode::Dark
                                    };
                                    gpui_component::Theme::change(new_mode, Some(window), cx);
                                }),
                        ),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .relative()
                    .child(self.dock_area.clone())
                    .when(is_file_search_visible, |overlay| {
                        overlay
                            .child(
                                div()
                                    .absolute()
                                    .size_full()
                                    .inset_0()
                                    .occlude()
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            this.file_search.update(cx, |search, cx| {
                                                search.close(window, cx);
                                            });
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .size_full()
                                    .top(px(80.))
                                    .flex()
                                    .justify_center()
                                    .items_start()
                                    .child(
                                        div()
                                            .occlude()
                                            .on_mouse_down(
                                                gpui::MouseButton::Left,
                                                |_, _, cx| {
                                                    cx.stop_propagation();
                                                },
                                            )
                                            .child(self.file_search.clone()),
                                    ),
                            )
                    }),
            )
            .child(self.render_bottom_bar(window, cx))
            .children(Root::render_dialog_layer(window, cx))
    }
}

fn error_result(error: DataSourceError) -> QueryResult {
    let error_msg = match error {
        DataSourceError::QueryFailed(msg) => msg,
        _ => error.to_string(),
    };
    QueryResult {
        columns: vec!["error".into()],
        rows: vec![vec![error_msg]],
        row_count: 1,
        execution_time_ms: 0,
    }
}
