use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::Write as FmtWrite,
    io::{Cursor, Write},
};

use chrono::Local;
use gpui::{
    App, AppContext, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, Modifiers, ParentElement, Render, StatefulInteractiveElement,
    Styled, WeakEntity, Window, actions, div, prelude::FluentBuilder, rgb,
};
use gpui_component::scroll::ScrollableElement;
use gpui_component::{
    ActiveTheme, Disableable, IconName, Sizable, WindowExt,
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockPlacement, Panel, PanelControl, PanelEvent, PanelState},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    table::{Column, ColumnSort, DataTable, TableDelegate, TableEvent, TableState},
    v_flex,
};

use crate::drivers::create_configured_data_source;
use crate::schema_cache;
use crate::ui::activity::ActivityTracker;
use crate::ui::components::tab::{Tab, TabBar};
use sqlab_drivers_core::{
    ColumnMetadata, DataSourceConfig, DataSourceError, Database, QueryResult, TableEditBatch,
    TableEditRow, TableEditValue, TableInfo, TableKind,
};

actions!(
    results_panel,
    [
        CopyResultSelection,
        CycleTabForward,
        CycleTabBackward,
        ExtendResultSelectionUp,
        ExtendResultSelectionDown,
        ExtendResultSelectionLeft,
        ExtendResultSelectionRight,
        EditResultCell
    ]
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportFormat {
    Csv,
    Markdown,
    Json,
    Xml,
    Xlsx,
    SqlInserts,
    SqlUpdates,
    WhereClause,
}

impl ExportFormat {
    const ALL: [Self; 8] = [
        Self::Csv,
        Self::Markdown,
        Self::Json,
        Self::Xml,
        Self::Xlsx,
        Self::SqlInserts,
        Self::SqlUpdates,
        Self::WhereClause,
    ];
    const COPYABLE: [Self; 7] = [
        Self::Csv,
        Self::Markdown,
        Self::Json,
        Self::Xml,
        Self::SqlInserts,
        Self::SqlUpdates,
        Self::WhereClause,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Csv => "CSV",
            Self::Markdown => "Markdown",
            Self::Json => "JSON",
            Self::Xml => "XML",
            Self::Xlsx => "Excel (XLSX)",
            Self::SqlInserts => "SQL Inserts",
            Self::SqlUpdates => "SQL Updates",
            Self::WhereClause => "WHERE Clause",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Markdown => "md",
            Self::Json => "json",
            Self::Xml => "xml",
            Self::Xlsx => "xlsx",
            Self::SqlInserts | Self::SqlUpdates | Self::WhereClause => "sql",
        }
    }
}

pub struct ResultPanel {
    focus_handle: FocusHandle,
    table_state: gpui::Entity<TableState<ResultsTableDelegate>>,
    export_counter: usize,
    next_result_id: usize,
    executions: Vec<QueryExecution>,
    active_tab: usize,
    pending_result: Option<QueryExecution>,
    dock_area: Option<WeakEntity<DockArea>>,
    is_zoomed: bool,
    selected_export_format: ExportFormat,
    activity_tracker: gpui::Entity<ActivityTracker>,
    submitting_edits: bool,
    edit_error: Option<String>,
}

#[derive(Clone)]
pub struct ResultsTableDelegate {
    pub columns: Vec<String>,
    pub column_metadata: Vec<ColumnMetadata>,
    pub rows: Vec<Vec<String>>,
    pub nulls: Vec<Vec<bool>>,
    original_rows: Vec<Vec<String>>,
    original_nulls: Vec<Vec<bool>>,
    row_ids: Vec<usize>,
    editable_table: Option<EditableTable>,
    dirty_cells: BTreeMap<(usize, usize), Option<String>>,
    editing_cell: Option<EditingCell>,
    selected_cells: BTreeSet<(usize, usize)>,
    selection_anchor: Option<(usize, usize)>,
    selection_cursor: Option<(usize, usize)>,
}

#[derive(Clone)]
struct EditingCell {
    row_ix: usize,
    col_ix: usize,
    input: gpui::Entity<InputState>,
}

#[derive(Clone, Debug)]
struct EditableTable {
    schema: String,
    table: String,
    columns: Vec<EditableColumn>,
    pk_col_indices: Vec<usize>,
}

#[derive(Clone, Debug)]
struct EditableColumn {
    name: String,
    data_type: String,
    editable: bool,
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
        let mut order = (0..self.rows.len()).collect::<Vec<_>>();
        order.sort_by(|&a_ix, &b_ix| {
            let a = &self.rows[a_ix];
            let b = &self.rows[b_ix];
            let ord = match self.columns.get(col_ix).map(String::as_str) {
                Some("id") => {
                    let a_num: i32 = a.get(col_ix).and_then(|v| v.parse().ok()).unwrap_or(0);
                    let b_num: i32 = b.get(col_ix).and_then(|v| v.parse().ok()).unwrap_or(0);
                    a_num.cmp(&b_num)
                }
                _ => a.get(col_ix).cmp(&b.get(col_ix)),
            };
            match sort {
                ColumnSort::Descending => ord.reverse(),
                _ => ord,
            }
        });
        self.rows = order
            .iter()
            .filter_map(|&ix| self.rows.get(ix).cloned())
            .collect();
        self.nulls = order
            .iter()
            .filter_map(|&ix| self.nulls.get(ix).cloned())
            .collect();
        self.row_ids = order
            .iter()
            .filter_map(|&ix| self.row_ids.get(ix).copied())
            .collect();
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let is_selected = self.selected_cells.contains(&(row_ix, col_ix));
        let row_id = self.row_id(row_ix);
        let is_dirty = self.dirty_cells.contains_key(&(row_id, col_ix));
        let is_editable = self.is_editable_cell(row_ix, col_ix);
        let is_editing = self
            .editing_cell
            .as_ref()
            .is_some_and(|cell| cell.row_ix == row_ix && cell.col_ix == col_ix);
        let display = self.cell_text(row_ix, col_ix, _cx);

        div()
            .id(format!("result-cell-content:{row_ix}:{col_ix}"))
            .relative()
            .size_full()
            .when(is_editing, |this| {
                if let Some(input) = self.editing_cell.as_ref().map(|cell| cell.input.clone()) {
                    this.child(Input::new(&input).xsmall().bordered(false).p_0())
                } else {
                    this
                }
            })
            .when(!is_editing, |this| {
                this.child(
                    div()
                        .size_full()
                        .when(is_editable, |this| this.cursor_text())
                        .child(display),
                )
            })
            .when(is_dirty, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size(gpui::px(5.))
                        .bg(rgb(0x58a65c)),
                )
            })
            .when(is_selected, |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(_cx.theme().table_active.opacity(0.35))
                        .border_1()
                        .border_color(_cx.theme().table_active_border),
                )
            })
            .on_mouse_down(
                gpui::MouseButton::Left,
                _cx.listener(move |table, event: &gpui::MouseDownEvent, _window, cx| {
                    let modifiers = event.modifiers;
                    table.delegate_mut().select_cell(row_ix, col_ix, modifiers);
                    table.set_selected_cell(row_ix, col_ix, cx);
                }),
            )
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _cx: &App) -> String {
        let row_id = self.row_id(row_ix);
        if let Some(value) = self.dirty_cells.get(&(row_id, col_ix)) {
            return value.clone().unwrap_or_default();
        }
        self.rows
            .get(row_ix)
            .and_then(|row| row.get(col_ix))
            .cloned()
            .unwrap_or_default()
    }
}

impl ResultsTableDelegate {
    fn empty() -> Self {
        Self::from_parts(Vec::new(), Vec::new(), Vec::new())
    }

    fn from_parts(
        columns: Vec<String>,
        column_metadata: Vec<ColumnMetadata>,
        rows: Vec<Vec<String>>,
    ) -> Self {
        let nulls = rows
            .iter()
            .map(|row| vec![false; row.len()])
            .collect::<Vec<_>>();
        Self::from_query(columns, column_metadata, rows, nulls, None)
    }

    fn from_query(
        columns: Vec<String>,
        column_metadata: Vec<ColumnMetadata>,
        rows: Vec<Vec<String>>,
        nulls: Vec<Vec<bool>>,
        editable_table: Option<EditableTable>,
    ) -> Self {
        let row_ids = (0..rows.len()).collect::<Vec<_>>();
        Self::from_query_state(
            columns,
            column_metadata,
            rows.clone(),
            nulls.clone(),
            rows,
            nulls,
            row_ids,
            BTreeMap::new(),
            editable_table,
        )
    }

    fn from_query_state(
        columns: Vec<String>,
        column_metadata: Vec<ColumnMetadata>,
        rows: Vec<Vec<String>>,
        nulls: Vec<Vec<bool>>,
        original_rows: Vec<Vec<String>>,
        original_nulls: Vec<Vec<bool>>,
        row_ids: Vec<usize>,
        dirty_cells: BTreeMap<(usize, usize), Option<String>>,
        editable_table: Option<EditableTable>,
    ) -> Self {
        Self {
            columns,
            column_metadata,
            original_rows,
            original_nulls,
            rows,
            nulls,
            row_ids,
            editable_table,
            dirty_cells,
            editing_cell: None,
            selected_cells: BTreeSet::new(),
            selection_anchor: None,
            selection_cursor: None,
        }
    }

    fn row_id(&self, row_ix: usize) -> usize {
        self.row_ids.get(row_ix).copied().unwrap_or(row_ix)
    }

    fn is_editable_cell(&self, row_ix: usize, col_ix: usize) -> bool {
        row_ix < self.rows.len()
            && self
                .editable_table
                .as_ref()
                .and_then(|table| table.columns.get(col_ix))
                .map(|column| column.editable)
                .unwrap_or(false)
    }

    fn has_dirty_cells(&self) -> bool {
        !self.dirty_cells.is_empty()
    }

    fn start_editing(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        input: gpui::Entity<InputState>,
    ) -> bool {
        if !self.is_editable_cell(row_ix, col_ix) {
            return false;
        }
        self.editing_cell = Some(EditingCell {
            row_ix,
            col_ix,
            input,
        });
        true
    }

    fn commit_editing(&mut self, cx: &App) -> bool {
        let Some(editing) = self.editing_cell.take() else {
            return false;
        };
        if !self.is_editable_cell(editing.row_ix, editing.col_ix) {
            return false;
        }

        let value = editing.input.read(cx).value().to_string();
        let new_value = if value.is_empty() { None } else { Some(value) };
        self.set_cell_value(editing.row_ix, editing.col_ix, new_value);
        true
    }

    fn set_cell_value(&mut self, row_ix: usize, col_ix: usize, value: Option<String>) {
        let row_id = self.row_id(row_ix);
        let display_value = value.clone().unwrap_or_default();
        if let Some(row) = self.rows.get_mut(row_ix)
            && let Some(cell) = row.get_mut(col_ix)
        {
            *cell = display_value;
        }
        if let Some(row) = self.nulls.get_mut(row_ix)
            && let Some(cell) = row.get_mut(col_ix)
        {
            *cell = value.is_none();
        }

        let original_value = self
            .original_rows
            .get(row_id)
            .and_then(|row| row.get(col_ix))
            .cloned()
            .unwrap_or_default();
        let original_is_null = self
            .original_nulls
            .get(row_id)
            .and_then(|row| row.get(col_ix))
            .copied()
            .unwrap_or(false);
        let matches_original = match &value {
            Some(value) => !original_is_null && *value == original_value,
            None => original_is_null,
        };

        if matches_original {
            self.dirty_cells.remove(&(row_id, col_ix));
        } else {
            self.dirty_cells.insert((row_id, col_ix), value);
        }
    }

    fn edit_batch(&self) -> Option<TableEditBatch> {
        let editable_table = self.editable_table.as_ref()?;
        if self.dirty_cells.is_empty() {
            return None;
        }

        let mut rows = BTreeMap::<usize, Vec<(usize, Option<String>)>>::new();
        for (&(row_id, col_ix), value) in &self.dirty_cells {
            if editable_table
                .columns
                .get(col_ix)
                .map(|column| column.editable)
                .unwrap_or(false)
            {
                rows.entry(row_id)
                    .or_default()
                    .push((col_ix, value.clone()));
            }
        }

        let rows = rows
            .into_iter()
            .filter_map(|(row_id, assignments)| {
                let keys = editable_table
                    .pk_col_indices
                    .iter()
                    .filter_map(|&col_ix| {
                        let column = editable_table.columns.get(col_ix)?;
                        Some(TableEditValue {
                            column: column.name.clone(),
                            data_type: column.data_type.clone(),
                            value: self.original_cell_value(row_id, col_ix),
                        })
                    })
                    .collect::<Vec<_>>();
                let assignments = assignments
                    .into_iter()
                    .filter_map(|(col_ix, value)| {
                        let column = editable_table.columns.get(col_ix)?;
                        Some(TableEditValue {
                            column: column.name.clone(),
                            data_type: column.data_type.clone(),
                            value,
                        })
                    })
                    .collect::<Vec<_>>();
                (!keys.is_empty() && !assignments.is_empty())
                    .then_some(TableEditRow { keys, assignments })
            })
            .collect::<Vec<_>>();

        (!rows.is_empty()).then_some(TableEditBatch {
            schema: editable_table.schema.clone(),
            table: editable_table.table.clone(),
            rows,
        })
    }

    fn original_cell_value(&self, row_id: usize, col_ix: usize) -> Option<String> {
        let is_null = self
            .original_nulls
            .get(row_id)
            .and_then(|row| row.get(col_ix))
            .copied()
            .unwrap_or(false);
        if is_null {
            None
        } else {
            Some(
                self.original_rows
                    .get(row_id)
                    .and_then(|row| row.get(col_ix))
                    .cloned()
                    .unwrap_or_default(),
            )
        }
    }

    fn mark_submitted(&mut self) {
        self.original_rows = self.rows.clone();
        self.original_nulls = self.nulls.clone();
        self.row_ids = (0..self.rows.len()).collect();
        self.dirty_cells.clear();
        self.editing_cell = None;
    }

    fn select_cell(&mut self, row_ix: usize, col_ix: usize, modifiers: Modifiers) {
        let cell = (row_ix, col_ix);
        if modifiers.shift {
            let anchor = self.selection_anchor.unwrap_or(cell);
            self.select_range(anchor, cell);
            self.selection_anchor = Some(anchor);
            self.selection_cursor = Some(cell);
        } else if modifiers.control {
            if !self.selected_cells.remove(&cell) {
                self.selected_cells.insert(cell);
            }
            self.selection_anchor = Some(cell);
            self.selection_cursor = Some(cell);
        } else {
            self.selected_cells.clear();
            self.selected_cells.insert(cell);
            self.selection_anchor = Some(cell);
            self.selection_cursor = Some(cell);
        }
    }

    fn extend_selection(&mut self, row_delta: isize, col_delta: isize) -> Option<(usize, usize)> {
        if self.rows.is_empty() || self.columns.is_empty() {
            return None;
        }

        let current = self
            .selection_cursor
            .or(self.selection_anchor)
            .unwrap_or((0, 0));
        let next_row = current
            .0
            .saturating_add_signed(row_delta)
            .min(self.rows.len().saturating_sub(1));
        let next_col = current
            .1
            .saturating_add_signed(col_delta)
            .min(self.columns.len().saturating_sub(1));
        let next = (next_row, next_col);
        let anchor = self.selection_anchor.unwrap_or(current);

        self.select_range(anchor, next);
        self.selection_anchor = Some(anchor);
        self.selection_cursor = Some(next);
        Some(next)
    }

    fn select_row(&mut self, row_ix: usize) {
        self.selected_cells.clear();
        if row_ix < self.rows.len() {
            for col_ix in 0..self.columns.len() {
                self.selected_cells.insert((row_ix, col_ix));
            }
            self.selection_anchor = Some((row_ix, 0));
            self.selection_cursor = Some((row_ix, self.columns.len().saturating_sub(1)));
        }
    }

    fn select_col(&mut self, col_ix: usize) {
        self.selected_cells.clear();
        if col_ix < self.columns.len() {
            for row_ix in 0..self.rows.len() {
                self.selected_cells.insert((row_ix, col_ix));
            }
            self.selection_anchor = Some((0, col_ix));
            self.selection_cursor = Some((self.rows.len().saturating_sub(1), col_ix));
        }
    }

    fn select_emitted_cell(&mut self, row_ix: usize, col_ix: usize) {
        let cell = (row_ix, col_ix);
        if !self.selected_cells.contains(&cell) {
            self.selected_cells.clear();
            self.selected_cells.insert(cell);
            self.selection_anchor = Some(cell);
            self.selection_cursor = Some(cell);
        } else {
            self.selection_cursor = Some(cell);
        }
    }

    fn selected_export_data(&self, table_name: String) -> Option<ExportData> {
        if self.selected_cells.is_empty() {
            return None;
        }

        let mut col_indices = self
            .selected_cells
            .iter()
            .map(|(_, col_ix)| *col_ix)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let row_indices = self
            .selected_cells
            .iter()
            .map(|(row_ix, _)| *row_ix)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        col_indices.retain(|col_ix| *col_ix < self.columns.len());
        let rows = row_indices
            .into_iter()
            .filter(|row_ix| *row_ix < self.rows.len())
            .map(|row_ix| {
                col_indices
                    .iter()
                    .map(|&col_ix| {
                        if self.selected_cells.contains(&(row_ix, col_ix)) {
                            self.rows[row_ix][col_ix].clone()
                        } else {
                            String::new()
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        if rows.is_empty() {
            return None;
        }

        Some(ExportData {
            columns: col_indices
                .iter()
                .map(|&col_ix| self.columns[col_ix].clone())
                .collect(),
            column_metadata: col_indices
                .iter()
                .filter_map(|&col_ix| self.column_metadata.get(col_ix).cloned())
                .collect(),
            rows,
            table_name,
        })
    }

    fn select_range(&mut self, anchor: (usize, usize), cursor: (usize, usize)) {
        self.selected_cells.clear();
        let row_start = anchor.0.min(cursor.0);
        let row_end = anchor.0.max(cursor.0);
        let col_start = anchor.1.min(cursor.1);
        let col_end = anchor.1.max(cursor.1);
        for row_ix in row_start..=row_end {
            for col_ix in col_start..=col_end {
                self.selected_cells.insert((row_ix, col_ix));
            }
        }
    }
}

#[derive(Clone)]
struct QueryExecution {
    id: usize,
    query: String,
    result: QueryResult,
    original_rows: Vec<Vec<String>>,
    original_nulls: Vec<Vec<bool>>,
    row_ids: Vec<usize>,
    dirty_cells: BTreeMap<(usize, usize), Option<String>>,
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
        let delegate = ResultsTableDelegate::empty();
        let table_state = cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .cell_selectable(true)
                .row_selectable(true)
                .col_resizable(true)
                .col_movable(true)
                .sortable(true)
        });
        Self::subscribe_table_selection(&table_state, window, cx);

        Self {
            focus_handle: cx.focus_handle(),
            table_state,
            export_counter: 0,
            next_result_id: 1,
            executions: Vec::new(),
            active_tab: 0,
            pending_result: None,
            dock_area: None,
            is_zoomed: false,
            selected_export_format: ExportFormat::Csv,
            activity_tracker,
            submitting_edits: false,
            edit_error: None,
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
        let original_rows = result.rows.clone();
        let original_nulls = result.nulls.clone();
        let row_ids = (0..result.rows.len()).collect();
        self.pending_result = Some(QueryExecution {
            id,
            query,
            result,
            original_rows,
            original_nulls,
            row_ids,
            dirty_cells: BTreeMap::new(),
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
            ResultsTableDelegate::from_parts(
                vec![
                    "id".into(),
                    "status".into(),
                    "data_source".into(),
                    "created_at".into(),
                    "query".into(),
                    "rows".into(),
                    "time_ms".into(),
                ],
                vec![],
                self.executions
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
                            execution.result.execution_time_ms.to_string(),
                        ]
                    })
                    .collect(),
            )
        } else {
            let execution = &self.executions[self.active_tab - 1];
            let enriched_metadata = enrich_column_metadata(
                execution.config.as_ref(),
                execution.result.column_metadata.clone(),
            );
            let editable_table = editable_table_for_execution(
                &execution.query,
                execution.config.as_ref(),
                &execution.result.columns,
            );
            ResultsTableDelegate::from_query_state(
                execution.result.columns.clone(),
                enriched_metadata,
                execution.result.rows.clone(),
                execution.result.nulls.clone(),
                execution.original_rows.clone(),
                execution.original_nulls.clone(),
                execution.row_ids.clone(),
                execution.dirty_cells.clone(),
                editable_table,
            )
        };

        self.table_state = cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .cell_selectable(true)
                .row_selectable(true)
                .col_resizable(true)
                .col_movable(true)
                .sortable(true)
        });
        Self::subscribe_table_selection(&self.table_state, window, cx);
    }

    fn subscribe_table_selection(
        table_state: &gpui::Entity<TableState<ResultsTableDelegate>>,
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
                }
                TableEvent::DoubleClickedCell(row_ix, col_ix) => {
                    this.start_edit_cell(*row_ix, *col_ix, window, cx);
                }
                _ => {}
            },
        )
        .detach();
    }

    fn start_edit_selected_cell(
        &mut self,
        _: &EditResultCell,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected_cell = self.table_state.read(cx).selected_cell();
        if let Some((row_ix, col_ix)) = selected_cell {
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
            self.sync_active_execution_from_delegate(cx);
            self.edit_error = None;
            cx.notify();
        }
        committed
    }

    fn sync_active_execution_from_delegate(&mut self, cx: &App) {
        let Some(active_ix) = self.active_tab.checked_sub(1) else {
            return;
        };
        let table = self.table_state.read(cx);
        let delegate = table.delegate();
        if let Some(execution) = self.executions.get_mut(active_ix) {
            execution.result.rows = delegate.rows.clone();
            execution.result.nulls = delegate.nulls.clone();
            execution.row_ids = delegate.row_ids.clone();
            execution.dirty_cells = delegate.dirty_cells.clone();
        }
    }

    fn has_dirty_edits(&self, cx: &App) -> bool {
        self.table_state.read(cx).delegate().has_dirty_cells()
    }

    fn submit_edits(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.submitting_edits {
            return;
        }
        self.commit_current_edit(cx);

        let Some(active_ix) = self.active_tab.checked_sub(1) else {
            return;
        };
        let Some(config) = self
            .executions
            .get(active_ix)
            .and_then(|execution| execution.config.clone())
        else {
            return;
        };
        let Some(batch) = self.table_state.read(cx).delegate().edit_batch() else {
            return;
        };

        self.submitting_edits = true;
        self.edit_error = None;
        cx.notify();

        let panel = cx.entity();
        let activity_tracker = self.activity_tracker.clone();
        let activity_id = cx.update_entity(&activity_tracker, |tracker, cx| {
            tracker.begin("Submitting result edits", cx)
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
                    Ok(()) => this.mark_active_edits_submitted(cx),
                    Err(error) => {
                        this.edit_error = Some(match error {
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

    fn mark_active_edits_submitted(&mut self, cx: &mut Context<Self>) {
        let Some(active_ix) = self.active_tab.checked_sub(1) else {
            return;
        };

        let (rows, nulls, row_ids, dirty_cells, original_rows, original_nulls) =
            self.table_state.update(cx, |table, cx| {
                let delegate = table.delegate_mut();
                delegate.mark_submitted();
                cx.notify();
                (
                    delegate.rows.clone(),
                    delegate.nulls.clone(),
                    delegate.row_ids.clone(),
                    delegate.dirty_cells.clone(),
                    delegate.original_rows.clone(),
                    delegate.original_nulls.clone(),
                )
            });
        if let Some(execution) = self.executions.get_mut(active_ix) {
            execution.result.rows = rows;
            execution.result.nulls = nulls;
            execution.row_ids = row_ids;
            execution.dirty_cells = dirty_cells;
            execution.original_rows = original_rows;
            execution.original_nulls = original_nulls;
        }
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

    fn export_result(&mut self, format: ExportFormat, window: &mut Window, cx: &mut Context<Self>) {
        let Some(data) = self.current_export_data(cx) else {
            return;
        };

        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let default_path = format!(
            "{}/result_{}.{}",
            home,
            self.export_counter,
            format.extension()
        );
        self.export_counter += 1;

        let input_state = cx.new(|cx| InputState::new(window, cx).default_value(default_path));
        let input_state_for_ok = input_state.clone();
        let activity_tracker = self.activity_tracker.clone();

        window.open_alert_dialog(cx, move |alert, _window, _cx| {
            alert
                .title(format!("Export to {}", format.label()))
                .child(Input::new(&input_state))
                .show_cancel(true)
                .on_ok({
                    let input_state_for_ok = input_state_for_ok.clone();
                    let data = data.clone();
                    let activity_tracker = activity_tracker.clone();
                    move |_this, _window, cx| {
                        let path = input_state_for_ok.read(cx).value().to_string();
                        let activity_id = cx.update_entity(&activity_tracker, |tracker, cx| {
                            tracker.begin(format!("Exporting to {}", format.label()), cx)
                        });

                        let data_for_spawn = data.clone();
                        let activity_tracker_for_spawn = activity_tracker.clone();
                        cx.spawn(async move |cx| {
                            let result =
                                cx.background_executor()
                                    .spawn(async move {
                                        write_export_file(&data_for_spawn, format, &path)
                                    })
                                    .await;

                            if let Err(error) = result {
                                eprintln!("failed to export {}: {}", format.label(), error);
                            }

                            cx.update_entity(&activity_tracker_for_spawn, |tracker, cx| {
                                tracker.finish(activity_id, cx);
                            });
                        })
                        .detach();

                        true
                    }
                })
        });
    }

    fn current_export_data(&self, cx: &App) -> Option<ExportData> {
        let execution = self
            .active_tab
            .checked_sub(1)
            .and_then(|ix| self.executions.get(ix))?;
        let table = self.table_state.read(cx);
        let delegate = table.delegate();
        Some(ExportData {
            columns: delegate.columns.clone(),
            column_metadata: delegate.column_metadata.clone(),
            rows: delegate.rows.clone(),
            table_name: infer_table_name(&execution.query),
        })
    }

    fn selected_export_data(&self, cx: &App) -> Option<ExportData> {
        let table = self.table_state.read(cx);
        let delegate = table.delegate();
        let active_query = self
            .active_tab
            .checked_sub(1)
            .and_then(|ix| self.executions.get(ix))
            .map(|execution| execution.query.as_str())
            .unwrap_or("results");
        let table_name = infer_table_name(active_query);

        if let Some(data) = delegate.selected_export_data(table_name.clone()) {
            return Some(data);
        }

        let (col_indices, row_indices) = if let Some((row_ix, col_ix)) = table.selected_cell() {
            (vec![col_ix], vec![row_ix])
        } else if let Some(row_ix) = table.selected_row() {
            ((0..delegate.columns_count(cx)).collect(), vec![row_ix])
        } else if let Some(col_ix) = table.selected_col() {
            (vec![col_ix], (0..delegate.rows_count(cx)).collect())
        } else {
            return None;
        };

        Some(ExportData {
            columns: col_indices
                .iter()
                .map(|&col_ix| delegate.columns[col_ix].clone())
                .collect(),
            column_metadata: col_indices
                .iter()
                .filter_map(|&col_ix| delegate.column_metadata.get(col_ix).cloned())
                .collect(),
            rows: row_indices
                .into_iter()
                .map(|row_ix| {
                    col_indices
                        .iter()
                        .map(|&col_ix| delegate.cell_text(row_ix, col_ix, cx))
                        .collect()
                })
                .collect(),
            table_name,
        })
    }

    fn copy_selection_as(&mut self, format: ExportFormat, cx: &mut Context<Self>) {
        self.selected_export_format = format;
        if let Some(data) = self.selected_export_data(cx) {
            match render_export_text(&data, format) {
                Ok(value) => cx.write_to_clipboard(ClipboardItem::new_string(value)),
                Err(error) => eprintln!("failed to copy {}: {}", format.label(), error),
            }
        }
    }

    fn copy_selection(
        &mut self,
        _: &CopyResultSelection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_selection_as(self.selected_export_format, cx);
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
            .map(|execution| format!("{} rows", execution.result.row_count))
            .unwrap_or_else(|| format!("{} queries", self.executions.len()));
        let has_dirty_edits = self.has_dirty_edits(cx);
        let submit_disabled = !has_dirty_edits || self.submitting_edits;

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
        let view_for_copy = view.clone();
        let view_for_export = view.clone();
        let selected_export_format = self.selected_export_format;

        v_flex()
            .id("results-panel")
            .key_context("results_panel")
            .size_full()
            .bg(cx.theme().background)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::on_extend_selection_up))
            .on_action(cx.listener(Self::on_extend_selection_down))
            .on_action(cx.listener(Self::on_extend_selection_left))
            .on_action(cx.listener(Self::on_extend_selection_right))
            .on_action(cx.listener(Self::on_cycle_tab_forward))
            .on_action(cx.listener(Self::on_cycle_tab_backward))
            .on_action(cx.listener(Self::start_edit_selected_cell))
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.focus_handle, cx);
            }))
            .child(
                h_flex()
                    .items_center()
                    .bg(cx.theme().tab_bar)
                    .border_b_1()
                    .border_color(cx.theme().border)
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
                        Button::new("results-submit-edits")
                            .icon(IconName::ArrowUp)
                            .xsmall()
                            .ghost()
                            .when(!submit_disabled, |button| button.text_color(rgb(0x58a65c)))
                            .tooltip(if has_dirty_edits {
                                "Submit result edits"
                            } else {
                                "No result edits to submit"
                            })
                            .disabled(submit_disabled)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_edits(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .overflow_x_hidden()
                            .truncate()
                            .child(query_label),
                    )
                    .children(self.edit_error.as_ref().map(|error| {
                        div()
                            .text_xs()
                            .text_color(rgb(0xef4444))
                            .truncate()
                            .child(error.clone())
                    }))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(row_label),
                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap_1()
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
                                Button::new("results-copy")
                                    .icon(IconName::Copy)
                                    .label(self.selected_export_format.label())
                                    .dropdown_caret(true)
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Copy selection")
                                    .dropdown_menu(move |menu, window, _cx| {
                                        let mut menu = menu;
                                        for format in ExportFormat::COPYABLE {
                                            menu = menu.item(
                                                PopupMenuItem::new(format!(
                                                    "Copy as {}",
                                                    format.label()
                                                ))
                                                .icon(IconName::Copy)
                                                .checked(format == selected_export_format)
                                                .on_click(window.listener_for(
                                                    &view_for_copy,
                                                    move |this, _, _window, cx| {
                                                        this.copy_selection_as(format, cx);
                                                    },
                                                )),
                                            );
                                        }
                                        menu
                                    }),
                            )
                            .child(
                                Button::new("results-download")
                                    .icon(IconName::ArrowDown)
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Download")
                                    .dropdown_menu(move |menu, window, _cx| {
                                        let mut menu = menu;
                                        for format in ExportFormat::ALL {
                                            menu = menu.item(
                                                PopupMenuItem::new(format!(
                                                    "Export {}",
                                                    format.label()
                                                ))
                                                .icon(IconName::File)
                                                .on_click(window.listener_for(
                                                    &view_for_export,
                                                    move |this, _, window, cx| {
                                                        this.export_result(format, window, cx);
                                                    },
                                                )),
                                            );
                                        }
                                        menu
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .id("result-table-container")
                    .flex_1()
                    .key_context("DataTable")
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.focus(&this.focus_handle, cx);
                    }))
                    .child({
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
                    }),
            )
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

fn editable_table_for_execution(
    query: &str,
    config: Option<&DataSourceConfig>,
    result_columns: &[String],
) -> Option<EditableTable> {
    let config = config?;
    let table_ref = single_table_select(query)?;
    let cache_key = schema_cache::cache_key(config);
    let schema = schema_cache::load(&cache_key).ok().flatten()?;
    let table = find_schema_table(&schema.tables, config, &table_ref)?;
    if !matches!(table.kind, TableKind::Table) {
        return None;
    }

    let column_counts =
        result_columns
            .iter()
            .fold(HashMap::<String, usize>::new(), |mut counts, column| {
                *counts.entry(column.clone()).or_default() += 1;
                counts
            });
    let table_columns = table
        .columns
        .iter()
        .map(|column| (column.name.clone(), column))
        .collect::<HashMap<_, _>>();

    let mut columns = Vec::with_capacity(result_columns.len());
    for column_name in result_columns {
        let unique = column_counts.get(column_name).copied().unwrap_or(0) == 1;
        let Some(column) = table_columns.get(column_name) else {
            columns.push(EditableColumn {
                name: column_name.clone(),
                data_type: String::new(),
                editable: false,
            });
            continue;
        };
        columns.push(EditableColumn {
            name: column.name.clone(),
            data_type: column.data_type.clone(),
            editable: unique && !column.is_pk && !column.is_generated,
        });
    }

    let pk_names = table
        .columns
        .iter()
        .filter(|column| column.is_pk)
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();
    if pk_names.is_empty() {
        return None;
    }
    let pk_col_indices = pk_names
        .iter()
        .map(|pk_name| {
            result_columns.iter().enumerate().find_map(|(ix, column)| {
                (column == pk_name && column_counts.get(column).copied() == Some(1)).then_some(ix)
            })
        })
        .collect::<Option<Vec<_>>>()?;

    Some(EditableTable {
        schema: table.schema.clone(),
        table: table.name.clone(),
        columns,
        pk_col_indices,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedTableRef {
    schema: Option<String>,
    table: String,
}

fn find_schema_table<'a>(
    tables: &'a [TableInfo],
    config: &DataSourceConfig,
    table_ref: &ParsedTableRef,
) -> Option<&'a TableInfo> {
    if let Some(schema) = &table_ref.schema {
        return tables
            .iter()
            .find(|table| table.name == table_ref.table && table.schema == *schema);
    }

    if !config.schema.is_empty() {
        return tables
            .iter()
            .find(|table| table.name == table_ref.table && table.schema == config.schema);
    }

    let matches = tables
        .iter()
        .filter(|table| table.name == table_ref.table)
        .collect::<Vec<_>>();
    if config.db_type == Database::Postgres {
        if let Some(table) = matches.iter().find(|table| table.schema == "public") {
            return Some(table);
        }
    }

    if matches.len() == 1 {
        Some(matches[0])
    } else {
        None
    }
}

fn single_table_select(query: &str) -> Option<ParsedTableRef> {
    let tokens = sql_tokens(query);
    if tokens.is_empty() || !tokens[0].eq_ignore_ascii_case("select") {
        return None;
    }
    if tokens
        .get(1)
        .is_some_and(|token| token.eq_ignore_ascii_case("distinct"))
    {
        return None;
    }

    let mut depth = 0usize;
    let mut from_ix = None;
    for (ix, token) in tokens.iter().enumerate() {
        match token.as_str() {
            "(" => depth += 1,
            ")" => depth = depth.saturating_sub(1),
            _ if depth == 0 && token.eq_ignore_ascii_case("from") => {
                from_ix = Some(ix);
                break;
            }
            _ => {}
        }
    }
    let from_ix = from_ix?;
    let table_token = tokens.get(from_ix + 1)?;
    if table_token == "(" {
        return None;
    }

    let stop_words = [
        "where",
        "order",
        "limit",
        "offset",
        "fetch",
        "for",
        "group",
        "having",
        "union",
        "intersect",
        "except",
    ];
    let mut ix = from_ix + 2;
    while ix < tokens.len() {
        let token = &tokens[ix];
        if token == "(" {
            depth += 1;
        } else if token == ")" {
            depth = depth.saturating_sub(1);
        } else if depth == 0 {
            if token == "," || token.eq_ignore_ascii_case("join") {
                return None;
            }
            if matches!(
                token.to_ascii_lowercase().as_str(),
                "group" | "having" | "union" | "intersect" | "except"
            ) {
                return None;
            }
            if stop_words
                .iter()
                .any(|stop_word| token.eq_ignore_ascii_case(stop_word))
            {
                break;
            }
        }
        ix += 1;
    }

    split_qualified_identifier(table_token)
}

fn sql_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = query.trim().trim_end_matches(';').chars().peekable();

    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            push_token(&mut tokens, &mut current);
            continue;
        }
        if matches!(ch, '(' | ')' | ',') {
            push_token(&mut tokens, &mut current);
            tokens.push(ch.to_string());
            continue;
        }
        if ch == '\'' {
            push_token(&mut tokens, &mut current);
            while let Some(next) = chars.next() {
                if next == '\'' {
                    if chars.peek() == Some(&'\'') {
                        let _ = chars.next();
                        continue;
                    }
                    break;
                }
            }
            continue;
        }
        current.push(ch);
    }
    push_token(&mut tokens, &mut current);
    tokens
}

fn push_token(tokens: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        tokens.push(std::mem::take(current));
    }
}

fn split_qualified_identifier(token: &str) -> Option<ParsedTableRef> {
    let parts = token
        .split('.')
        .map(clean_identifier_token)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [table] => Some(ParsedTableRef {
            schema: None,
            table: table.clone(),
        }),
        [schema, table] => Some(ParsedTableRef {
            schema: Some(schema.clone()),
            table: table.clone(),
        }),
        _ => None,
    }
}

#[derive(Clone, Debug)]
struct ExportData {
    columns: Vec<String>,
    column_metadata: Vec<ColumnMetadata>,
    rows: Vec<Vec<String>>,
    table_name: String,
}

fn write_export_file(data: &ExportData, format: ExportFormat, path: &str) -> anyhow::Result<()> {
    match format {
        ExportFormat::Xlsx => std::fs::write(path, render_xlsx(data)?)?,
        _ => std::fs::write(path, render_export_text(data, format)?)?,
    }
    Ok(())
}

fn render_export_text(data: &ExportData, format: ExportFormat) -> anyhow::Result<String> {
    let output = match format {
        ExportFormat::Csv => render_csv(data),
        ExportFormat::Markdown => render_markdown(data),
        ExportFormat::Json => render_json(data)?,
        ExportFormat::Xml => render_xml(data),
        ExportFormat::Xlsx => anyhow::bail!("XLSX is a binary export format"),
        ExportFormat::SqlInserts => render_sql_inserts(data),
        ExportFormat::SqlUpdates => render_sql_updates(data),
        ExportFormat::WhereClause => render_where_clause(data),
    };
    Ok(output)
}

fn render_csv(data: &ExportData) -> String {
    let mut lines = Vec::with_capacity(data.rows.len() + 1);
    lines.push(
        data.columns
            .iter()
            .map(|column| escape_csv_field(column))
            .collect::<Vec<_>>()
            .join(","),
    );
    lines.extend(data.rows.iter().map(|row| {
        row.iter()
            .map(|cell| escape_csv_field(cell))
            .collect::<Vec<_>>()
            .join(",")
    }));
    lines.join("\n")
}

fn render_markdown(data: &ExportData) -> String {
    let mut output = String::new();
    output.push('|');
    for column in &data.columns {
        output.push(' ');
        output.push_str(&escape_markdown_cell(column));
        output.push_str(" |");
    }
    output.push('\n');
    output.push('|');
    for _ in &data.columns {
        output.push_str(" --- |");
    }
    output.push('\n');
    for row in &data.rows {
        output.push('|');
        for cell in row {
            output.push(' ');
            output.push_str(&escape_markdown_cell(cell));
            output.push_str(" |");
        }
        output.push('\n');
    }
    output
}

fn render_json(data: &ExportData) -> anyhow::Result<String> {
    let columns = unique_column_names(&data.columns);
    let rows = data
        .rows
        .iter()
        .map(|row| {
            let mut object = serde_json::Map::new();
            for (column, cell) in columns.iter().zip(row) {
                object.insert(column.clone(), serde_json::Value::String(cell.clone()));
            }
            serde_json::Value::Object(object)
        })
        .collect::<Vec<_>>();
    Ok(serde_json::to_string_pretty(&rows)?)
}

fn render_xml(data: &ExportData) -> String {
    let columns = unique_column_names(&data.columns);
    let mut output = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<results>\n");
    for row in &data.rows {
        output.push_str("  <row>\n");
        for (column, cell) in columns.iter().zip(row) {
            let _ = writeln!(
                output,
                "    <field name=\"{}\">{}</field>",
                escape_xml(column),
                escape_xml(cell)
            );
        }
        output.push_str("  </row>\n");
    }
    output.push_str("</results>\n");
    output
}

fn render_sql_inserts(data: &ExportData) -> String {
    let columns = unique_column_names(&data.columns);
    let table = quote_sql_identifier(&data.table_name);
    let identifiers = columns
        .iter()
        .map(|column| quote_sql_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");

    data.rows
        .iter()
        .map(|row| {
            let values = row
                .iter()
                .enumerate()
                .map(|(ix, cell)| sql_literal(cell, data.column_metadata.get(ix)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("INSERT INTO {table} ({identifiers}) VALUES ({values});")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_sql_updates(data: &ExportData) -> String {
    let columns = unique_column_names(&data.columns);
    if columns.len() < 2 {
        return "-- SQL Updates need at least one key column and one value column.\n".to_string();
    }

    let key_ix = data
        .column_metadata
        .iter()
        .enumerate()
        .find_map(|(ix, metadata)| metadata.is_pk.then_some(ix))
        .unwrap_or(0);
    let table = quote_sql_identifier(&data.table_name);
    let key_column = quote_sql_identifier(&columns[key_ix]);

    data.rows
        .iter()
        .map(|row| {
            let assignments = row
                .iter()
                .enumerate()
                .filter(|(ix, _)| *ix != key_ix)
                .map(|(ix, cell)| {
                    format!(
                        "{} = {}",
                        quote_sql_identifier(&columns[ix]),
                        sql_literal(cell, data.column_metadata.get(ix))
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "UPDATE {table} SET {assignments} WHERE {key_column} = {};",
                sql_literal(&row[key_ix], data.column_metadata.get(key_ix))
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_where_clause(data: &ExportData) -> String {
    let columns = unique_column_names(&data.columns);
    if columns.is_empty() || data.rows.is_empty() {
        return "WHERE FALSE;\n".to_string();
    }

    let clauses = data
        .rows
        .iter()
        .map(|row| {
            let conditions = row
                .iter()
                .enumerate()
                .map(|(ix, cell)| {
                    format!(
                        "{} = {}",
                        quote_sql_identifier(&columns[ix]),
                        sql_literal(cell, data.column_metadata.get(ix))
                    )
                })
                .collect::<Vec<_>>()
                .join(" AND ");
            format!("({conditions})")
        })
        .collect::<Vec<_>>();
    format!("WHERE\n  {};\n", clauses.join("\n  OR "))
}

fn render_xlsx(data: &ExportData) -> anyhow::Result<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut cursor);
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);

        zip.start_file("[Content_Types].xml", options)?;
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
  <Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
  <Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>
</Types>"#,
        )?;

        zip.start_file("_rels/.rels", options)?;
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#,
        )?;

        zip.start_file("xl/workbook.xml", options)?;
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
    <sheet name="Results" sheetId="1" r:id="rId1"/>
  </sheets>
</workbook>"#,
        )?;

        zip.start_file("xl/_rels/workbook.xml.rels", options)?;
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#,
        )?;

        zip.start_file("xl/styles.xml", options)?;
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>
  <fills count="1"><fill><patternFill patternType="none"/></fill></fills>
  <borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>
  <cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
  <cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs>
</styleSheet>"#,
        )?;

        zip.start_file("xl/worksheets/sheet1.xml", options)?;
        zip.write_all(render_xlsx_sheet(data).as_bytes())?;
        zip.finish()?;
    }
    Ok(cursor.into_inner())
}

fn render_xlsx_sheet(data: &ExportData) -> String {
    let mut output = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData>
"#,
    );
    write_xlsx_row(&mut output, 1, &data.columns);
    for (row_ix, row) in data.rows.iter().enumerate() {
        write_xlsx_row(&mut output, row_ix + 2, row);
    }
    output.push_str("  </sheetData>\n</worksheet>");
    output
}

fn write_xlsx_row(output: &mut String, row_number: usize, cells: &[String]) {
    let _ = writeln!(output, "    <row r=\"{row_number}\">");
    for (col_ix, value) in cells.iter().enumerate() {
        let cell_ref = format!("{}{}", xlsx_column_name(col_ix), row_number);
        let _ = writeln!(
            output,
            "      <c r=\"{}\" t=\"inlineStr\"><is><t>{}</t></is></c>",
            cell_ref,
            escape_xml(value)
        );
    }
    output.push_str("    </row>\n");
}

fn xlsx_column_name(mut ix: usize) -> String {
    let mut name = String::new();
    loop {
        let remainder = ix % 26;
        name.insert(0, (b'A' + remainder as u8) as char);
        if ix < 26 {
            break;
        }
        ix = ix / 26 - 1;
    }
    name
}

fn infer_table_name(query: &str) -> String {
    let tokens = query
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ';' | ',' | '(' | ')'))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();

    for window in tokens.windows(2) {
        if matches_ignore_ascii_case(window[0], "from")
            || matches_ignore_ascii_case(window[0], "into")
            || matches_ignore_ascii_case(window[0], "update")
        {
            return clean_identifier_token(window[1]);
        }
    }
    "results".to_string()
}

fn clean_identifier_token(token: &str) -> String {
    token
        .trim_matches(|ch| matches!(ch, '"' | '`' | '[' | ']'))
        .to_string()
}

fn matches_ignore_ascii_case(value: &str, expected: &str) -> bool {
    value.eq_ignore_ascii_case(expected)
}

fn unique_column_names(columns: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashMap::<String, usize>::new();
    columns
        .iter()
        .map(|column| {
            let count = seen.entry(column.clone()).or_insert(0);
            *count += 1;
            if *count == 1 {
                column.clone()
            } else {
                format!("{}_{}", column, count)
            }
        })
        .collect()
}

fn escape_csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn escape_markdown_cell(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\n', "<br>")
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn quote_sql_identifier(identifier: &str) -> String {
    identifier
        .split('.')
        .map(|part| format!("\"{}\"", part.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(".")
}

fn sql_literal(value: &str, metadata: Option<&ColumnMetadata>) -> String {
    if value.is_empty() {
        return "NULL".to_string();
    }

    let data_type = metadata
        .map(|metadata| metadata.data_type.to_ascii_lowercase())
        .unwrap_or_default();
    let numeric = [
        "int",
        "int2",
        "int4",
        "int8",
        "integer",
        "bigint",
        "smallint",
        "numeric",
        "decimal",
        "real",
        "double",
        "float",
        "serial",
        "bigserial",
    ];
    if numeric.iter().any(|prefix| data_type.starts_with(prefix)) && value.parse::<f64>().is_ok() {
        return value.to_string();
    }
    if matches!(data_type.as_str(), "bool" | "boolean")
        && matches!(value.to_ascii_lowercase().as_str(), "true" | "false")
    {
        return value.to_ascii_uppercase();
    }

    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_data() -> ExportData {
        ExportData {
            columns: vec!["id".into(), "name".into()],
            column_metadata: vec![
                ColumnMetadata {
                    name: "id".into(),
                    data_type: "int4".into(),
                    is_pk: true,
                    is_fk: false,
                },
                ColumnMetadata {
                    name: "name".into(),
                    data_type: "text".into(),
                    is_pk: false,
                    is_fk: false,
                },
            ],
            rows: vec![
                vec!["1".into(), "Ada".into()],
                vec!["2".into(), "Bob".into()],
            ],
            table_name: "users".into(),
        }
    }

    #[test]
    fn renders_sql_inserts_with_identifiers_and_literals() {
        assert_eq!(
            render_sql_inserts(&sample_data()),
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES (1, 'Ada');\nINSERT INTO \"users\" (\"id\", \"name\") VALUES (2, 'Bob');"
        );
    }

    #[test]
    fn renders_sql_updates_using_primary_key() {
        assert_eq!(
            render_sql_updates(&sample_data()),
            "UPDATE \"users\" SET \"name\" = 'Ada' WHERE \"id\" = 1;\nUPDATE \"users\" SET \"name\" = 'Bob' WHERE \"id\" = 2;"
        );
    }

    #[test]
    fn renders_where_clause_for_selected_rows() {
        assert_eq!(
            render_where_clause(&sample_data()),
            "WHERE\n  (\"id\" = 1 AND \"name\" = 'Ada')\n  OR (\"id\" = 2 AND \"name\" = 'Bob');\n"
        );
    }

    #[test]
    fn writes_xlsx_zip_payload() {
        let bytes = render_xlsx(&sample_data()).expect("xlsx should render");
        assert!(bytes.starts_with(b"PK"));
    }

    #[test]
    fn shift_selects_rectangular_cell_range_for_export() {
        let mut delegate = ResultsTableDelegate::from_parts(
            vec!["id".into(), "name".into(), "city".into()],
            Vec::new(),
            vec![
                vec!["1".into(), "Ada".into(), "London".into()],
                vec!["2".into(), "Bob".into(), "Paris".into()],
            ],
        );

        delegate.select_cell(0, 0, Modifiers::none());
        delegate.select_cell(1, 1, Modifiers::shift());

        let data = delegate
            .selected_export_data("users".into())
            .expect("selection should export");
        assert_eq!(data.columns, vec!["id", "name"]);
        assert_eq!(
            data.rows,
            vec![
                vec!["1".to_string(), "Ada".to_string()],
                vec!["2".to_string(), "Bob".to_string()]
            ]
        );
    }

    #[test]
    fn control_click_toggles_sparse_cells_for_export_shape() {
        let mut delegate = ResultsTableDelegate::from_parts(
            vec!["id".into(), "name".into(), "city".into()],
            Vec::new(),
            vec![
                vec!["1".into(), "Ada".into(), "London".into()],
                vec!["2".into(), "Bob".into(), "Paris".into()],
            ],
        );

        delegate.select_cell(0, 0, Modifiers::none());
        delegate.select_cell(1, 2, Modifiers::control());

        let data = delegate
            .selected_export_data("users".into())
            .expect("selection should export");
        assert_eq!(data.columns, vec!["id", "city"]);
        assert_eq!(
            data.rows,
            vec![
                vec!["1".to_string(), String::new()],
                vec![String::new(), "Paris".to_string()]
            ]
        );
    }

    #[test]
    fn detects_strict_single_table_selects() {
        assert_eq!(
            single_table_select("select id, name from public.users where id = 1"),
            Some(ParsedTableRef {
                schema: Some("public".into()),
                table: "users".into(),
            })
        );
        assert_eq!(
            single_table_select("select * from users order by id limit 10"),
            Some(ParsedTableRef {
                schema: None,
                table: "users".into(),
            })
        );
        assert_eq!(
            single_table_select("with u as (select * from users) select * from u"),
            None
        );
        assert_eq!(
            single_table_select("select * from users join posts on true"),
            None
        );
        assert_eq!(
            single_table_select("select count(*) from users group by name"),
            None
        );
        assert_eq!(
            single_table_select("select * from (select * from users) u"),
            None
        );
    }

    #[test]
    fn resolves_unqualified_tables_with_blank_schema() {
        let tables = vec![
            table_info("analytics", "users"),
            table_info("public", "users"),
            table_info("analytics", "events"),
        ];
        let config = DataSourceConfig {
            db_type: Database::Postgres,
            schema: String::new(),
            ..DataSourceConfig::default()
        };

        let users = find_schema_table(
            &tables,
            &config,
            &ParsedTableRef {
                schema: None,
                table: "users".into(),
            },
        )
        .unwrap();
        assert_eq!(users.schema, "public");

        let events = find_schema_table(
            &tables,
            &config,
            &ParsedTableRef {
                schema: None,
                table: "events".into(),
            },
        )
        .unwrap();
        assert_eq!(events.schema, "analytics");
    }

    fn table_info(schema: &str, name: &str) -> TableInfo {
        TableInfo {
            schema: schema.into(),
            name: name.into(),
            kind: TableKind::Table,
            columns: Vec::new(),
        }
    }

    #[test]
    fn dirty_cells_are_removed_when_value_matches_original() {
        let editable_table = EditableTable {
            schema: "public".into(),
            table: "users".into(),
            columns: vec![
                EditableColumn {
                    name: "id".into(),
                    data_type: "int4".into(),
                    editable: false,
                },
                EditableColumn {
                    name: "name".into(),
                    data_type: "text".into(),
                    editable: true,
                },
            ],
            pk_col_indices: vec![0],
        };
        let mut delegate = ResultsTableDelegate::from_query(
            vec!["id".into(), "name".into()],
            sample_data().column_metadata,
            vec![vec!["1".into(), "Ada".into()]],
            vec![vec![false, false]],
            Some(editable_table),
        );

        delegate.set_cell_value(0, 1, Some("Grace".into()));
        assert!(delegate.has_dirty_cells());

        delegate.set_cell_value(0, 1, Some("Ada".into()));
        assert!(!delegate.has_dirty_cells());
    }

    #[test]
    fn builds_edit_batch_from_dirty_cells_with_original_primary_key() {
        let editable_table = EditableTable {
            schema: "public".into(),
            table: "users".into(),
            columns: vec![
                EditableColumn {
                    name: "id".into(),
                    data_type: "int4".into(),
                    editable: false,
                },
                EditableColumn {
                    name: "name".into(),
                    data_type: "text".into(),
                    editable: true,
                },
            ],
            pk_col_indices: vec![0],
        };
        let mut delegate = ResultsTableDelegate::from_query(
            vec!["id".into(), "name".into()],
            sample_data().column_metadata,
            vec![vec!["1".into(), "Ada".into()]],
            vec![vec![false, false]],
            Some(editable_table),
        );

        delegate.set_cell_value(0, 1, Some("Grace".into()));
        let batch = delegate
            .edit_batch()
            .expect("dirty cell should build batch");

        assert_eq!(batch.schema, "public");
        assert_eq!(batch.table, "users");
        assert_eq!(batch.rows.len(), 1);
        assert_eq!(batch.rows[0].keys[0].column, "id");
        assert_eq!(batch.rows[0].keys[0].value.as_deref(), Some("1"));
        assert_eq!(batch.rows[0].assignments[0].column, "name");
        assert_eq!(batch.rows[0].assignments[0].value.as_deref(), Some("Grace"));
    }
}
