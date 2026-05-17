use chrono::Local;
use gpui::{
    App, AppContext, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled,
    WeakEntity, Window, actions, div, rgb,
};
use gpui_component::scroll::ScrollableElement;
use gpui_component::{
    ActiveTheme, IconName, Sizable, WindowExt,
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockPlacement, Panel, PanelControl, PanelEvent, PanelState},
    h_flex,
    input::{Input, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    table::{Column, ColumnSort, DataTable, TableDelegate, TableState},
    v_flex,
};

use crate::schema_cache;
use crate::ui::activity::ActivityTracker;
use crate::ui::components::tab::{Tab, TabBar};
use zql_drivers_core::{ColumnMetadata, DataSourceConfig, QueryResult};
use zql_drivers_postgres::PostgresDataSource;

actions!(
    results_panel,
    [CopyResultSelection, CycleTabForward, CycleTabBackward]
);

pub struct ResultPanel {
    focus_handle: FocusHandle,
    activity_tracker: gpui::Entity<ActivityTracker>,
    table_state: gpui::Entity<TableState<ResultsTableDelegate>>,
    export_counter: usize,
    next_result_id: usize,
    executions: Vec<QueryExecution>,
    active_tab: usize,
    pending_result: Option<QueryExecution>,
    dock_area: Option<WeakEntity<DockArea>>,
    is_zoomed: bool,
}

#[derive(Clone)]
pub struct ResultsTableDelegate {
    pub columns: Vec<String>,
    pub column_metadata: Vec<ColumnMetadata>,
    pub rows: Vec<Vec<String>>,
}

impl TableDelegate for ResultsTableDelegate {
    fn columns_count(&self, _cx: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _cx: &App) -> Column {
        Column::new(&self.columns[col_ix], &self.columns[col_ix])
            .width(gpui::px(140.))
            .sortable()
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let meta = self.column_metadata.get(col_ix);
        let is_pk = meta.map(|m| m.is_pk).unwrap_or(false);
        let is_fk = meta.map(|m| m.is_fk).unwrap_or(false);
        let data_type = meta.map(|m| m.data_type.clone()).unwrap_or_default();

        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;

        h_flex()
            .size_full()
            .gap_1()
            .children(if is_pk {
                Some(div().text_color(muted).child("★"))
            } else if is_fk {
                Some(div().text_color(muted).text_xs().child("→"))
            } else {
                None
            })
            .child(
                div()
                    .text_color(foreground)
                    .child(self.columns[col_ix].clone()),
            )
            .children(if !data_type.is_empty() {
                Some(div().text_color(muted).text_xs().child(data_type))
            } else {
                None
            })
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) {
        self.rows.sort_by(|a, b| {
            let ord = match self.columns[col_ix].as_str() {
                "id" => {
                    // Numeric sort for id column
                    let a_num: i32 = a[col_ix].parse().unwrap_or(0);
                    let b_num: i32 = b[col_ix].parse().unwrap_or(0);
                    a_num.cmp(&b_num)
                }
                _ => a[col_ix].cmp(&b[col_ix]),
            };
            match sort {
                ColumnSort::Descending => ord.reverse(),
                _ => ord,
            }
        });
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div().child(self.rows[row_ix][col_ix].clone())
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _cx: &App) -> String {
        self.rows[row_ix][col_ix].clone()
    }
}

#[derive(Clone)]
struct QueryExecution {
    id: usize,
    query: String,
    result: QueryResult,
    succeeded: bool,
    created_at: String,
    config: Option<DataSourceConfig>,
}

impl EventEmitter<PanelEvent> for ResultPanel {}

impl ResultPanel {
    pub fn new(
        activity_tracker: gpui::Entity<ActivityTracker>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = ResultsTableDelegate {
            columns: Vec::new(),
            column_metadata: Vec::new(),
            rows: Vec::new(),
        };
        let table_state = cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .cell_selectable(true)
                .row_selectable(true)
                .col_resizable(true)
                .col_movable(true)
                .sortable(true)
        });

        Self {
            focus_handle: cx.focus_handle(),
            activity_tracker,
            table_state,
            export_counter: 0,
            next_result_id: 1,
            executions: Vec::new(),
            active_tab: 0,
            pending_result: None,
            dock_area: None,
            is_zoomed: false,
        }
    }

    pub fn set_dock_area(&mut self, dock_area: WeakEntity<DockArea>) {
        self.dock_area = Some(dock_area);
    }

    pub fn set_result(
        &mut self,
        query: String,
        result: QueryResult,
        succeeded: bool,
        config: Option<DataSourceConfig>,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_result_id;
        self.next_result_id += 1;
        self.pending_result = Some(QueryExecution {
            id,
            query,
            result,
            succeeded,
            created_at: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            config,
        });
        cx.notify();
    }

    fn apply_pending_result(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(execution) = self.pending_result.take() else {
            return;
        };
        self.executions.push(execution);
        self.active_tab = self.executions.len();
        self.rebuild_table(window, cx);
    }

    fn rebuild_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active_tab = self.active_tab.min(self.executions.len());
        let delegate = if self.active_tab == 0 {
            ResultsTableDelegate {
                columns: vec![
                    "id".into(),
                    "status".into(),
                    "data_source".into(),
                    "created_at".into(),
                    "query".into(),
                    "rows".into(),
                    "shown".into(),
                    "time_ms".into(),
                ],
                column_metadata: vec![],
                rows: self
                    .executions
                    .iter()
                    .rev()
                    .map(|execution| {
                        vec![
                            execution.id.to_string(),
                            if execution.succeeded {
                                "succeeded".into()
                            } else {
                                "failed".into()
                            },
                            execution.data_source_name(),
                            execution.created_at.clone(),
                            truncate_query(&execution.query, 120),
                            execution.result.row_count.to_string(),
                            execution.result.rows.len().to_string(),
                            execution.result.execution_time_ms.to_string(),
                        ]
                    })
                    .collect(),
            }
        } else {
            let execution = &self.executions[self.active_tab - 1];
            let enriched_metadata = enrich_column_metadata(
                execution.config.as_ref(),
                execution.result.column_metadata.clone(),
            );
            ResultsTableDelegate {
                columns: execution.result.columns.clone(),
                column_metadata: enriched_metadata,
                rows: execution.result.rows.clone(),
            }
        };

        self.table_state = cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .cell_selectable(true)
                .row_selectable(true)
                .col_resizable(true)
                .col_movable(true)
                .sortable(true)
        });
    }

    fn close_result_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.executions.len() {
            return;
        }

        self.executions.remove(ix);
        let closed_tab = ix + 1;
        if self.active_tab == closed_tab {
            self.active_tab = ix.min(self.executions.len());
        } else if self.active_tab > closed_tab {
            self.active_tab -= 1;
        }
        self.rebuild_table(window, cx);
        cx.notify();
    }

    fn export_to_csv(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(execution) = self
            .active_tab
            .checked_sub(1)
            .and_then(|ix| self.executions.get(ix))
            .cloned()
        else {
            return;
        };

        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let default_path = format!("{}/result_{}.csv", home, self.export_counter);
        self.export_counter += 1;

        let input_state = cx.new(|cx| InputState::new(window, cx).default_value(default_path));
        let input_state_for_ok = input_state.clone();
        let activity_tracker = self.activity_tracker.clone();

        window.open_alert_dialog(cx, move |alert, _window, _cx| {
            alert
                .title("Export to CSV")
                .child(Input::new(&input_state))
                .show_cancel(true)
                .on_ok({
                    let input_state_for_ok = input_state_for_ok.clone();
                    let execution = execution.clone();
                    let activity_tracker = activity_tracker.clone();
                    move |_, _window, cx| {
                        let path = input_state_for_ok.read(cx).value().to_string();
                        if let Some(config) = execution.config.clone() {
                            let query = execution.query.clone();
                            let activity_id = activity_tracker
                                .update(cx, |tracker, cx| tracker.begin("Downloading results", cx));
                            let activity_tracker = activity_tracker.clone();
                            cx.spawn(async move |cx| {
                                let result = cx
                                    .background_executor()
                                    .spawn(async move {
                                        let mut source = PostgresDataSource::new(config)?;
                                        source.connect_blocking()?;
                                        let result = source.export_query_to_csv(&query, path);
                                        let _ = source.disconnect_blocking();
                                        result
                                    })
                                    .await;

                                if let Err(e) = result {
                                    eprintln!("failed to export CSV: {}", e);
                                }

                                activity_tracker.update(cx, |tracker, cx| {
                                    tracker.finish(activity_id, cx);
                                });
                            })
                            .detach();
                        } else {
                            eprintln!("cannot export CSV without a data source config");
                        }
                        true
                    }
                })
        });
    }

    fn copy_selection(
        &mut self,
        _: &CopyResultSelection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table = self.table_state.read(cx);
        let delegate = table.delegate();

        let value = if let Some((row_ix, col_ix)) = table.selected_cell() {
            Some(delegate.cell_text(row_ix, col_ix, cx))
        } else if let Some(row_ix) = table.selected_row() {
            Some(
                (0..delegate.columns_count(cx))
                    .map(|col_ix| delegate.cell_text(row_ix, col_ix, cx))
                    .collect::<Vec<_>>()
                    .join("\t"),
            )
        } else if let Some(col_ix) = table.selected_col() {
            Some(
                (0..delegate.rows_count(cx))
                    .map(|row_ix| delegate.cell_text(row_ix, col_ix, cx))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        } else {
            None
        };

        if let Some(value) = value {
            cx.write_to_clipboard(ClipboardItem::new_string(value));
        }
    }

    fn on_cycle_tab_forward(
        &mut self,
        _: &CycleTabForward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.executions.len() + 1 > 1 {
            self.active_tab = (self.active_tab + 1) % (self.executions.len() + 1);
            self.rebuild_table(window, cx);
            cx.notify();
            window.focus(&self.focus_handle, cx);
        }
    }

    fn on_cycle_tab_backward(
        &mut self,
        _: &CycleTabBackward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.executions.len() + 1 > 1 {
            self.active_tab =
                (self.active_tab + self.executions.len()) % (self.executions.len() + 1);
            self.rebuild_table(window, cx);
            cx.notify();
            window.focus(&self.focus_handle, cx);
        }
    }

    fn reorder_tab(
        &mut self,
        from_ix: usize,
        to_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if from_ix >= self.executions.len() || to_ix >= self.executions.len() || from_ix == to_ix {
            return;
        }
        let execution = self.executions.remove(from_ix);
        self.executions.insert(to_ix, execution);
        if self.active_tab == from_ix + 1 {
            self.active_tab = to_ix + 1;
        } else if from_ix < self.active_tab - 1 && to_ix >= self.active_tab - 1 {
            self.active_tab -= 1;
        } else if from_ix > self.active_tab - 1 && to_ix <= self.active_tab - 1 {
            self.active_tab += 1;
        }
        self.rebuild_table(window, cx);
        cx.notify();
    }
}

impl Panel for ResultPanel {
    fn panel_name(&self) -> &'static str {
        "ResultPanel"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        "Query Results"
    }

    fn closable(&self, _cx: &App) -> bool {
        false
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState::new(self)
    }

    fn zoomable(&self, _cx: &App) -> Option<PanelControl> {
        None
    }
}

impl ResultPanel {
    fn toggle_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.is_zoomed = !self.is_zoomed;
        if let Some(dock_area) = self.dock_area.as_ref() {
            if let Some(dock_area) = dock_area.upgrade() {
                dock_area.update(cx, |dock_area, cx| {
                    if self.is_zoomed {
                        if dock_area.is_dock_open(DockPlacement::Left, cx) {
                            dock_area.toggle_dock(DockPlacement::Left, window, cx);
                        }
                        if dock_area.is_dock_open(DockPlacement::Right, cx) {
                            dock_area.toggle_dock(DockPlacement::Right, window, cx);
                        }
                        if !dock_area.is_dock_open(DockPlacement::Bottom, cx) {
                            dock_area.toggle_dock(DockPlacement::Bottom, window, cx);
                        }
                    } else {
                        if !dock_area.is_dock_open(DockPlacement::Left, cx) {
                            dock_area.toggle_dock(DockPlacement::Left, window, cx);
                        }
                        if !dock_area.is_dock_open(DockPlacement::Right, cx) {
                            dock_area.toggle_dock(DockPlacement::Right, window, cx);
                        }
                        if !dock_area.is_dock_open(DockPlacement::Bottom, cx) {
                            dock_area.toggle_dock(DockPlacement::Bottom, window, cx);
                        }
                    }
                });
            }
        }
        cx.notify();
    }
}

impl Render for ResultPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.apply_pending_result(window, cx);
        let active_execution = self
            .active_tab
            .checked_sub(1)
            .and_then(|ix| self.executions.get(ix));
        let query_label = active_execution
            .map(|execution| truncate_query(&execution.query, 80))
            .unwrap_or_default();
        let data_source_label = active_execution.map(QueryExecution::data_source_name);
        let row_label = active_execution
            .map(|execution| {
                format!(
                    "{} rows (showing {})",
                    execution.result.row_count,
                    execution.result.rows.len()
                )
            })
            .unwrap_or_else(|| format!("{} queries", self.executions.len()));

        let bottom_btn = self.dock_area.as_ref().and_then(|dock_area| {
            let dock_area = dock_area.upgrade()?;
            let is_open = dock_area.read(cx).is_dock_open(DockPlacement::Bottom, cx);
            let icon = if is_open {
                IconName::PanelBottom
            } else {
                IconName::PanelBottomOpen
            };
            Some(
                Button::new("toggle-bottom")
                    .icon(icon)
                    .xsmall()
                    .ghost()
                    .tooltip(if is_open { "Collapse" } else { "Expand" })
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(dock_area) = this.dock_area.as_ref() {
                            if let Some(dock_area) = dock_area.upgrade() {
                                dock_area.update(cx, |dock_area, cx| {
                                    dock_area.toggle_dock(DockPlacement::Bottom, window, cx);
                                });
                            }
                        }
                        cx.notify();
                    })),
            )
        });

        let entity = cx.entity();
        let tab_bar = TabBar::new("results-tab-bar")
            .selected_index(self.active_tab)
            .on_click(cx.listener(|this, ix: &usize, window, cx| {
                this.active_tab = *ix;
                this.rebuild_table(window, cx);
                cx.notify();
            }))
            .on_reorder(cx.listener(|this, (from_ix, to_ix), window, cx| {
                this.reorder_tab(*from_ix, *to_ix, window, cx);
            }))
            .child(Tab::new().label("History").selected(self.active_tab == 0));

        let tab_bar =
            self.executions
                .iter()
                .enumerate()
                .fold(tab_bar, |tab_bar, (ix, execution)| {
                    let entity = entity.clone();
                    tab_bar.child(
                        Tab::new()
                            .label(format!("Result {}", execution.id))
                            .selected(self.active_tab == ix + 1)
                            .closable(true)
                            .on_close(move |window, cx| {
                                entity.update(cx, |this, cx| {
                                    this.close_result_tab(ix, window, cx);
                                });
                            }),
                    )
                });
        let view = cx.entity();

        v_flex()
            .id("results-panel")
            .key_context("results_panel")
            .size_full()
            .bg(cx.theme().background)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::on_cycle_tab_forward))
            .on_action(cx.listener(Self::on_cycle_tab_backward))
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.focus_handle, cx);
            }))
            .child(
                h_flex()
                    .items_center()
                    .bg(cx.theme().tab_bar)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().p_1().children(bottom_btn))
                    .child(tab_bar),
            )
            .child(
                // Toolbar
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().accent)
                            .text_color(cx.theme().accent_foreground)
                            .text_sm()
                            .child("Output"),
                    )
                    .children(data_source_label.map(|name| {
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(cx.theme().border)
                            .text_xs()
                            .text_color(cx.theme().foreground)
                            .child(name)
                    }))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(query_label),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(row_label),
                    )
                    .child(
                        Button::new("results-zoom")
                            .icon(if self.is_zoomed {
                                IconName::Minimize
                            } else {
                                IconName::Maximize
                            })
                            .xsmall()
                            .ghost()
                            .tooltip(if self.is_zoomed {
                                "Restore"
                            } else {
                                "Maximize"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_zoom(window, cx);
                            })),
                    )
                    .child(
                        Button::new("results-download")
                            .icon(IconName::ArrowDown)
                            .xsmall()
                            .ghost()
                            .tooltip("Download")
                            .dropdown_menu(move |menu, window, _cx| {
                                menu.item(
                                    PopupMenuItem::new("Export CSV")
                                        .icon(IconName::File)
                                        .on_click(window.listener_for(
                                            &view,
                                            |this, _, window, cx| {
                                                this.export_to_csv(window, cx);
                                            },
                                        )),
                                )
                            }),
                    ),
            )
            .child(div().flex_1().child({
                if self.active_tab == 0 {
                    DataTable::new(&self.table_state)
                        .xsmall()
                        .stripe(true)
                        .bordered(true)
                        .scrollbar_visible(true, true)
                        .into_any_element()
                } else {
                    let execution = &self.executions[self.active_tab - 1];
                    if execution.succeeded {
                        DataTable::new(&self.table_state)
                            .xsmall()
                            .stripe(true)
                            .bordered(true)
                            .scrollbar_visible(true, true)
                            .into_any_element()
                    } else {
                        let error_msg = execution
                            .result
                            .rows
                            .first()
                            .and_then(|r| r.first())
                            .cloned()
                            .unwrap_or_default();
                        div()
                            .size_full()
                            .p_4()
                            .overflow_y_scrollbar()
                            .text_color(rgb(0xef4444))
                            .font_family("Monospace")
                            .child(error_msg)
                            .into_any_element()
                    }
                }
            }))
    }
}

impl Focusable for ResultPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl QueryExecution {
    fn data_source_name(&self) -> String {
        self.config
            .as_ref()
            .map(|config| config.name.clone())
            .unwrap_or_else(|| "unknown".into())
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

fn enrich_column_metadata(
    config: Option<&DataSourceConfig>,
    metadata: Vec<ColumnMetadata>,
) -> Vec<ColumnMetadata> {
    let Some(config) = config else {
        return metadata;
    };

    let cache_key = schema_cache::cache_key(config);
    let Ok(Some(schema)) = schema_cache::load(&cache_key) else {
        return metadata;
    };

    let mut pk_columns = std::collections::HashSet::new();
    let mut fk_columns = std::collections::HashSet::new();

    for table in &schema.tables {
        for col in &table.columns {
            if col.is_pk {
                pk_columns.insert(col.name.clone());
            }
            if col.is_fk {
                fk_columns.insert(col.name.clone());
            }
        }
    }

    metadata
        .into_iter()
        .map(|mut m| {
            if pk_columns.contains(&m.name) {
                m.is_pk = true;
            }
            if fk_columns.contains(&m.name) {
                m.is_fk = true;
            }
            m
        })
        .collect()
}
