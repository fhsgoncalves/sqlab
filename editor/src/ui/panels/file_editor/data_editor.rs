use std::collections::HashMap;

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    Modifiers, ParentElement, Render, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px, rgb,
};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Sizable,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    table::{DataTable, TableDelegate, TableEvent, TableState},
    v_flex,
};
use sqlab_drivers_core::{
    ColumnMetadata, DataSourceConfig, DataSourceError, Database, QueryResult, TableInfo,
};

use crate::drivers::create_configured_data_source;
use crate::ui::activity::ActivityTracker;
use crate::ui::panels::result::{
    EditResultCell, EditableTable, ExtendResultSelectionDown, ExtendResultSelectionLeft,
    ExtendResultSelectionRight, ExtendResultSelectionUp, ResultsTableDelegate,
    SelectResultCellDown, SelectResultCellLeft, SelectResultCellRight, SelectResultCellUp,
};

const DEFAULT_LIMIT: usize = 1000;

#[derive(Clone, Debug)]
pub struct ShowDataEditorEvent {
    pub config: DataSourceConfig,
    pub schema: String,
    pub table: String,
}

pub struct DataEditorPanel {
    focus_handle: FocusHandle,
    table_state: Entity<TableState<ResultsTableDelegate>>,
    config: DataSourceConfig,
    table: TableInfo,
    where_input: Entity<InputState>,
    limit_input: Entity<InputState>,
    activity_tracker: Entity<ActivityTracker>,
    loading: bool,
    submitting_edits: bool,
    load_version: usize,
    query_label: String,
    row_count: usize,
    error: Option<String>,
    pending_result: Option<(QueryResult, TableInfo)>,
}

impl DataEditorPanel {
    pub fn new(
        config: DataSourceConfig,
        table: TableInfo,
        activity_tracker: Entity<ActivityTracker>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let where_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("e.g. id > 10 AND name LIKE '%test%'")
        });
        let limit_input =
            cx.new(|cx| InputState::new(window, cx).default_value(DEFAULT_LIMIT.to_string()));
        let table_state = Self::new_table_state(ResultsTableDelegate::empty(), window, cx);
        Self::subscribe_table_selection(&table_state, window, cx);

        cx.subscribe_in(&where_input, window, |this, _input, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.load_data(window, cx);
            }
        })
        .detach();
        cx.subscribe_in(&limit_input, window, |this, _input, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.load_data(window, cx);
            }
        })
        .detach();

        let mut panel = Self {
            focus_handle: cx.focus_handle(),
            table_state,
            config,
            table,
            where_input,
            limit_input,
            activity_tracker,
            loading: false,
            submitting_edits: false,
            load_version: 0,
            query_label: String::new(),
            row_count: 0,
            error: None,
            pending_result: None,
        };
        panel.load_data(window, cx);
        panel
    }

    pub fn title(&self) -> String {
        format!("{}.{}", self.table.schema, self.table.name)
    }

    pub fn matches_table(&self, config: &DataSourceConfig, table: &TableInfo) -> bool {
        self.config.name == config.name
            && self.table.schema == table.schema
            && self.table.name == table.name
    }

    fn new_table_state(
        delegate: ResultsTableDelegate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TableState<ResultsTableDelegate>> {
        cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .cell_selectable(true)
                .row_selectable(true)
                .col_resizable(true)
                .col_movable(true)
                .sortable(true)
        })
    }

    fn subscribe_table_selection(
        table_state: &Entity<TableState<ResultsTableDelegate>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe_in(
            table_state,
            window,
            |this: &mut Self, table, event: &TableEvent, window, cx| match event {
                TableEvent::SelectRow(row_ix) => {
                    table.update(cx, |table, cx| {
                        table.delegate_mut().select_row(*row_ix);
                        cx.notify();
                    });
                }
                TableEvent::SelectColumn(col_ix) => {
                    table.update(cx, |table, cx| {
                        table.delegate_mut().select_col(*col_ix);
                        cx.notify();
                    });
                }
                TableEvent::SelectCell(row_ix, col_ix) => {
                    table.update(cx, |table, cx| {
                        table.delegate_mut().select_emitted_cell(*row_ix, *col_ix);
                        cx.notify();
                    });
                    cx.notify();
                }
                TableEvent::DoubleClickedCell(row_ix, col_ix) => {
                    this.start_edit_cell(*row_ix, *col_ix, window, cx);
                }
                _ => {}
            },
        )
        .detach();
    }

    fn load_data(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.submitting_edits {
            return;
        }

        self.commit_current_edit(cx);
        self.load_version += 1;
        let load_version = self.load_version;
        let limit = self.limit(cx);
        let where_clause = self.where_input.read(cx).value().trim().to_string();
        let query = data_editor_query(&self.config, &self.table, &where_clause, limit);

        self.loading = true;
        self.error = None;
        self.query_label = query.clone();
        cx.notify();

        let config = self.config.clone();
        let table = self.table.clone();
        let panel = cx.entity();
        let activity_tracker = self.activity_tracker.clone();
        let activity_label = format!("Loading data: {}.{}", table.schema, table.name);
        let activity_id = cx.update_entity(&activity_tracker, |tracker, cx| {
            tracker.begin(activity_label, cx)
        });

        cx.spawn(async move |_this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut source = create_configured_data_source(&config)?;
                    source.connect().await?;
                    let result = source.execute_query(&query).await;
                    source.disconnect().await?;
                    result
                })
                .await;

            cx.update_entity(&activity_tracker, |tracker, cx| {
                tracker.finish(activity_id, cx);
            });

            cx.update_entity(&panel, move |this, cx| {
                if this.load_version != load_version {
                    return;
                }

                this.loading = false;
                match result {
                    Ok(result) => {
                        this.pending_result = Some((result, table));
                    }
                    Err(error) => {
                        this.error = Some(match error {
                            DataSourceError::QueryFailed(message) => message,
                            other => other.to_string(),
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_pending_result(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((result, table)) = self.pending_result.take() else {
            return;
        };
        self.apply_result(result, table, window, cx);
    }

    fn apply_result(
        &mut self,
        result: QueryResult,
        table: TableInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let metadata = column_metadata_for_table(&table, &result);
        let editable_table = EditableTable::from_table_result_columns(&table, &result.columns);
        let delegate = ResultsTableDelegate::from_query(
            result.columns,
            metadata,
            result.rows,
            result.nulls,
            editable_table,
        );
        self.row_count = result.row_count;
        self.table_state = Self::new_table_state(delegate, window, cx);
        Self::subscribe_table_selection(&self.table_state, window, cx);
        self.error = None;
    }

    fn limit(&self, cx: &App) -> usize {
        self.limit_input
            .read(cx)
            .value()
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|limit| *limit > 0)
            .unwrap_or(DEFAULT_LIMIT)
    }

    fn start_edit_selected_cell(
        &mut self,
        _: &EditResultCell,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((row_ix, col_ix)) = self.table_state.read(cx).selected_cell() {
            self.start_edit_cell(row_ix, col_ix, window, cx);
        }
    }

    fn start_edit_cell(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_current_edit(cx);
        let value = self
            .table_state
            .read(cx)
            .delegate()
            .cell_text(row_ix, col_ix, cx);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(value));

        let input_for_subscription = input.clone();
        cx.subscribe_in(
            &input_for_subscription,
            window,
            move |this, _input, event: &InputEvent, _window, cx| match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    this.commit_current_edit(cx);
                }
                _ => {}
            },
        )
        .detach();

        let started = self.table_state.update(cx, |table, cx| {
            let started = table
                .delegate_mut()
                .start_editing(row_ix, col_ix, input.clone());
            if started {
                table.set_selected_cell(row_ix, col_ix, cx);
            }
            started
        });

        if started {
            window.focus(&input.read(cx).focus_handle(cx), cx);
        }
    }

    fn commit_current_edit(&mut self, cx: &mut Context<Self>) -> bool {
        let committed = self.table_state.update(cx, |table, cx| {
            let committed = table.delegate_mut().commit_editing(cx);
            if committed {
                cx.notify();
            }
            committed
        });
        if committed {
            self.error = None;
            cx.notify();
        }
        committed
    }

    fn submit_edits(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.submitting_edits {
            return;
        }
        self.commit_current_edit(cx);

        let Some(batch) = self.table_state.read(cx).delegate().edit_batch() else {
            return;
        };

        self.submitting_edits = true;
        self.error = None;
        cx.notify();

        let config = self.config.clone();
        let table_label = self.title();
        let panel = cx.entity();
        let activity_tracker = self.activity_tracker.clone();
        let activity_id = cx.update_entity(&activity_tracker, |tracker, cx| {
            tracker.begin(format!("Submitting data edits: {table_label}"), cx)
        });

        cx.spawn(async move |_this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut source = create_configured_data_source(&config)?;
                    source.connect().await?;
                    let result = source.apply_table_edits(batch).await;
                    source.disconnect().await?;
                    result
                })
                .await;

            cx.update_entity(&activity_tracker, |tracker, cx| {
                tracker.finish(activity_id, cx);
            });

            cx.update_entity(&panel, move |this, cx| {
                this.submitting_edits = false;
                match result {
                    Ok(()) => {
                        this.table_state.update(cx, |table, cx| {
                            table.delegate_mut().mark_submitted();
                            cx.notify();
                        });
                    }
                    Err(error) => {
                        this.error = Some(match error {
                            DataSourceError::QueryFailed(message) => message,
                            other => other.to_string(),
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn has_dirty_edits(&self, cx: &App) -> bool {
        self.table_state.read(cx).delegate().has_dirty_cells()
    }

    fn extend_selection_by(
        &mut self,
        row_delta: isize,
        col_delta: isize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.table_state.update(cx, |table, cx| {
            let next = { table.delegate_mut().extend_selection(row_delta, col_delta) };
            if let Some((row_ix, col_ix)) = next {
                table.set_selected_cell(row_ix, col_ix, cx);
            }
        });
    }

    fn select_cell_horizontally(
        &mut self,
        col_delta: isize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.table_state.update(cx, |table, cx| {
            let columns_count = table.delegate().columns_count(cx);
            let rows_count = table.delegate().rows_count(cx);
            if columns_count == 0 || rows_count == 0 {
                return;
            }

            if let Some((row_ix, col_ix)) = table.selected_cell() {
                let Some(next_col) = bounded_column_move(col_ix, col_delta, columns_count) else {
                    return;
                };
                let row_ix = row_ix.min(rows_count.saturating_sub(1));
                table
                    .delegate_mut()
                    .select_cell(row_ix, next_col, Modifiers::none());
                table.set_selected_cell(row_ix, next_col, cx);
            } else {
                table.delegate_mut().select_cell(0, 0, Modifiers::none());
                table.set_selected_cell(0, 0, cx);
            }
        });
    }

    fn select_cell_vertically(
        &mut self,
        row_delta: isize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.table_state.update(cx, |table, cx| {
            let columns_count = table.delegate().columns_count(cx);
            let rows_count = table.delegate().rows_count(cx);
            if columns_count == 0 || rows_count == 0 {
                return;
            }

            if let Some((row_ix, col_ix)) = table.selected_cell() {
                let Some(next_row) = bounded_row_move(row_ix, row_delta, rows_count) else {
                    return;
                };
                let col_ix = col_ix.min(columns_count.saturating_sub(1));
                table
                    .delegate_mut()
                    .select_cell(next_row, col_ix, Modifiers::none());
                table.set_selected_cell(next_row, col_ix, cx);
            } else {
                table.delegate_mut().select_cell(0, 0, Modifiers::none());
                table.set_selected_cell(0, 0, cx);
            }
        });
    }

    fn on_select_cell_left(
        &mut self,
        _: &SelectResultCellLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_cell_horizontally(-1, window, cx);
    }

    fn on_select_cell_right(
        &mut self,
        _: &SelectResultCellRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_cell_horizontally(1, window, cx);
    }

    fn on_select_cell_up(
        &mut self,
        _: &SelectResultCellUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_cell_vertically(-1, window, cx);
    }

    fn on_select_cell_down(
        &mut self,
        _: &SelectResultCellDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_cell_vertically(1, window, cx);
    }

    fn on_extend_selection_up(
        &mut self,
        _: &ExtendResultSelectionUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extend_selection_by(-1, 0, window, cx);
    }

    fn on_extend_selection_down(
        &mut self,
        _: &ExtendResultSelectionDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extend_selection_by(1, 0, window, cx);
    }

    fn on_extend_selection_left(
        &mut self,
        _: &ExtendResultSelectionLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extend_selection_by(0, -1, window, cx);
    }

    fn on_extend_selection_right(
        &mut self,
        _: &ExtendResultSelectionRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extend_selection_by(0, 1, window, cx);
    }
}

impl Render for DataEditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.apply_pending_result(window, cx);

        let has_dirty_edits = self.has_dirty_edits(cx);
        let submit_disabled = !has_dirty_edits || self.submitting_edits || self.loading;
        let row_label = if self.loading {
            "Loading...".to_string()
        } else {
            format!("{} rows", self.row_count)
        };

        v_flex()
            .id("data-editor")
            .key_context("DataTable")
            .size_full()
            .bg(cx.theme().background)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_extend_selection_up))
            .on_action(cx.listener(Self::on_extend_selection_down))
            .on_action(cx.listener(Self::on_extend_selection_left))
            .on_action(cx.listener(Self::on_extend_selection_right))
            .on_action(cx.listener(Self::on_select_cell_left))
            .on_action(cx.listener(Self::on_select_cell_right))
            .on_action(cx.listener(Self::on_select_cell_up))
            .on_action(cx.listener(Self::on_select_cell_down))
            .on_action(cx.listener(Self::start_edit_selected_cell))
            .child(
                h_flex()
                    .h(px(38.))
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().tab_bar)
                    .child(
                        div()
                            .font_family("Monospace")
                            .text_sm()
                            .text_color(rgb(0xd89532))
                            .child("SELECT * FROM"),
                    )
                    .child(
                        div()
                            .font_family("Monospace")
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(self.title()),
                    )
                    .child(
                        div()
                            .font_family("Monospace")
                            .text_sm()
                            .text_color(rgb(0xd89532))
                            .child("WHERE"),
                    )
                    .child(
                        div()
                            .w(px(420.))
                            .child(Input::new(&self.where_input).xsmall()),
                    )
                    .child(
                        div()
                            .font_family("Monospace")
                            .text_sm()
                            .text_color(rgb(0xd89532))
                            .child("LIMIT"),
                    )
                    .child(
                        div()
                            .w(px(88.))
                            .child(Input::new(&self.limit_input).xsmall()),
                    )
                    .child(
                        Button::new("data-editor-refresh")
                            .icon(IconName::Redo)
                            .label("Refresh")
                            .xsmall()
                            .ghost()
                            .disabled(self.loading)
                            .tooltip("Refresh data")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.load_data(window, cx);
                            })),
                    )
                    .child(
                        Button::new("data-editor-submit-edits")
                            .icon(IconName::ArrowUp)
                            .xsmall()
                            .ghost()
                            .when(!submit_disabled, |button| button.text_color(rgb(0x58a65c)))
                            .disabled(submit_disabled)
                            .tooltip(if has_dirty_edits {
                                "Submit data edits"
                            } else {
                                "No data edits to submit"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_edits(window, cx);
                            })),
                    )
                    .child(div().flex_1())
                    .children(self.error.as_ref().map(|error| {
                        div()
                            .max_w(px(360.))
                            .truncate()
                            .text_xs()
                            .text_color(rgb(0xef4444))
                            .child(error.clone())
                    }))
                    .child(
                        div()
                            .flex_none()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(row_label),
                    ),
            )
            .child(
                div()
                    .id("data-editor-status")
                    .h(px(30.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .truncate()
                    .child(if has_dirty_edits {
                        "Unsaved changes".to_string()
                    } else {
                        "No unsaved changes".to_string()
                    }),
            )
            .child(
                div()
                    .id("data-editor-table")
                    .flex_1()
                    .overflow_hidden()
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.focus(&this.focus_handle, cx);
                    }))
                    .child(
                        DataTable::new(&self.table_state)
                            .xsmall()
                            .stripe(false)
                            .fill_width(false)
                            .bordered(true)
                            .scrollbar_visible(true, true),
                    ),
            )
    }
}

impl Focusable for DataEditorPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn data_editor_query(
    config: &DataSourceConfig,
    table: &TableInfo,
    where_clause: &str,
    limit: usize,
) -> String {
    let mut query = format!(
        "SELECT * FROM {}",
        qualified_table_name(config.db_type, table)
    );
    let where_clause = normalized_where_clause(where_clause);
    if !where_clause.is_empty() {
        query.push_str(" WHERE ");
        query.push_str(&where_clause);
    }
    query.push_str(" LIMIT ");
    query.push_str(&limit.to_string());
    query
}

fn normalized_where_clause(where_clause: &str) -> String {
    where_clause
        .trim()
        .strip_prefix("WHERE ")
        .or_else(|| where_clause.trim().strip_prefix("where "))
        .unwrap_or_else(|| where_clause.trim())
        .trim()
        .trim_end_matches(';')
        .trim()
        .to_string()
}

fn qualified_table_name(database: Database, table: &TableInfo) -> String {
    let quote = match database {
        Database::MySql => quote_mysql_identifier,
        Database::Postgres | Database::SQLite => quote_standard_identifier,
    };

    if table.schema.is_empty() {
        quote(&table.name)
    } else {
        format!("{}.{}", quote(&table.schema), quote(&table.name))
    }
}

fn quote_standard_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn quote_mysql_identifier(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

fn column_metadata_for_table(table: &TableInfo, result: &QueryResult) -> Vec<ColumnMetadata> {
    let table_columns = table
        .columns
        .iter()
        .map(|column| (column.name.clone(), column))
        .collect::<HashMap<_, _>>();

    result
        .columns
        .iter()
        .enumerate()
        .map(|(ix, name)| {
            if let Some(column) = table_columns.get(name) {
                ColumnMetadata {
                    name: column.name.clone(),
                    data_type: column.data_type.clone(),
                    is_pk: column.is_pk,
                    is_fk: column.is_fk,
                }
            } else {
                result
                    .column_metadata
                    .get(ix)
                    .cloned()
                    .unwrap_or_else(|| ColumnMetadata {
                        name: name.clone(),
                        data_type: String::new(),
                        is_pk: false,
                        is_fk: false,
                    })
            }
        })
        .collect()
}

fn bounded_column_move(col_ix: usize, col_delta: isize, columns_count: usize) -> Option<usize> {
    let next_col = col_ix.checked_add_signed(col_delta)?;
    (next_col < columns_count).then_some(next_col)
}

fn bounded_row_move(row_ix: usize, row_delta: isize, rows_count: usize) -> Option<usize> {
    let next_row = row_ix.checked_add_signed(row_delta)?;
    (next_row < rows_count).then_some(next_row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlab_drivers_core::TableKind;

    fn table() -> TableInfo {
        TableInfo {
            schema: "public".into(),
            name: "transactions".into(),
            kind: TableKind::Table,
            columns: vec![],
        }
    }

    #[test]
    fn builds_default_postgres_query() {
        let config = DataSourceConfig {
            db_type: Database::Postgres,
            ..DataSourceConfig::default()
        };

        assert_eq!(
            data_editor_query(&config, &table(), "", 1000),
            "SELECT * FROM \"public\".\"transactions\" LIMIT 1000"
        );
    }

    #[test]
    fn normalizes_where_prefix() {
        let config = DataSourceConfig {
            db_type: Database::Postgres,
            ..DataSourceConfig::default()
        };

        assert_eq!(
            data_editor_query(&config, &table(), "WHERE id > 10;", 25),
            "SELECT * FROM \"public\".\"transactions\" WHERE id > 10 LIMIT 25"
        );
    }

    #[test]
    fn quotes_mysql_identifiers() {
        let config = DataSourceConfig {
            db_type: Database::MySql,
            ..DataSourceConfig::default()
        };
        let table = TableInfo {
            schema: "app".into(),
            name: "order`items".into(),
            ..table()
        };

        assert_eq!(
            data_editor_query(&config, &table, "status = 'open'", 50),
            "SELECT * FROM `app`.`order``items` WHERE status = 'open' LIMIT 50"
        );
    }
}
