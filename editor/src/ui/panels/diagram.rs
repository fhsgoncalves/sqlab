use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt::Write as FmtWrite,
};

use fontdue::{Font, FontSettings};
use gpui::{
    App, AppContext, BorderStyle, Bounds, Context, EventEmitter, FocusHandle, Focusable, Hsla,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, PathBuilder, Render, ScrollWheelEvent, StatefulInteractiveElement, Styled,
    TextRun, TextStyle, WhiteSpace, Window, canvas, div, fill, point, prelude::FluentBuilder, px,
    rgb, size,
};
use gpui_component::{
    ActiveTheme, IconName, Sizable, WindowExt,
    button::{Button, ButtonVariants as _},
    dock::{Panel, PanelEvent, PanelState},
    h_flex,
    input::{Input, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use image::{Rgba, RgbaImage};
use sqlab_drivers_core::{DataSourceConfig, DatabaseSchema, TableKind};
use std::sync::Arc;

use crate::schema_cache;
use crate::ui::activity::ActivityTracker;

pub const MAX_DIAGRAM_TABLES: usize = 100;

const MIN_TABLE_WIDTH: f32 = 300.0;
const MAX_TABLE_WIDTH: f32 = 460.0;
const HEADER_HEIGHT: f32 = 28.0;
const ROW_HEIGHT: f32 = 24.0;
const TABLE_GAP_X: f32 = 150.0;
const TABLE_GAP_Y: f32 = 44.0;
const COMPONENT_GAP_Y: f32 = 120.0;
const MARKER_COL_WIDTH: f32 = 26.0;
const TYPE_COL_WIDTH: f32 = 132.0;
const TABLE_PADDING_X: f32 = 10.0;
const HEADER_FONT_PX: f32 = 14.0;
const COLUMN_FONT_PX: f32 = 12.5;
const TYPE_FONT_PX: f32 = 12.0;
const MARKER_FONT_PX: f32 = 13.0;
const EXPORT_MARGIN: f32 = 32.0;
const EDGE_CLEARANCE: f32 = 18.0;
const EDGE_PORT_STUB: f32 = 34.0;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TableRef {
    pub schema: String,
    pub name: String,
}

impl TableRef {
    pub fn new(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema: schema.into(),
            name: name.into(),
        }
    }

    fn label(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }

    fn display_label(&self) -> String {
        if self.schema == "public" {
            self.name.clone()
        } else {
            self.label()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagramScope {
    Database,
    Schema(String),
    Table { schema: String, table: String },
}

impl DiagramScope {
    pub fn tab_title(&self, connection_name: &str) -> String {
        match self {
            DiagramScope::Database => format!("{} diagram", connection_name),
            DiagramScope::Schema(schema) => format!("{} diagram", schema),
            DiagramScope::Table { table, .. } => format!("{} diagram", table),
        }
    }

    fn selected_table(&self) -> Option<TableRef> {
        match self {
            DiagramScope::Table { schema, table } => Some(TableRef::new(schema, table)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ShowDiagramEvent {
    pub config: DataSourceConfig,
    pub scope: DiagramScope,
}

#[derive(Clone, Debug)]
pub struct DiagramColumn {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub is_pk: bool,
    pub is_fk: bool,
}

#[derive(Clone, Debug)]
pub struct DiagramTable {
    pub id: TableRef,
    pub columns: Vec<DiagramColumn>,
}

impl DiagramTable {
    fn height(&self) -> f32 {
        HEADER_HEIGHT + self.columns.len().max(1) as f32 * ROW_HEIGHT
    }

    fn width(&self) -> f32 {
        let title_chars = self.id.display_label().chars().count() as f32;
        let widest_row = self
            .columns
            .iter()
            .map(|column| {
                MARKER_COL_WIDTH
                    + column.name.chars().count() as f32 * 6.4
                    + TYPE_COL_WIDTH.max(column.data_type.chars().count() as f32 * 6.1)
                    + TABLE_PADDING_X * 3.0
            })
            .fold(title_chars * 7.4 + TABLE_PADDING_X * 2.0, f32::max);
        widest_row.clamp(MIN_TABLE_WIDTH, MAX_TABLE_WIDTH)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiagramExportFormat {
    Png,
    Mermaid,
}

impl DiagramExportFormat {
    const ALL: [Self; 2] = [Self::Png, Self::Mermaid];

    fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Mermaid => "Mermaid",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Mermaid => "mmd",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DiagramEdge {
    pub source: TableRef,
    pub source_columns: Vec<String>,
    pub target: TableRef,
    pub target_columns: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct DiagramModel {
    pub title: String,
    pub scope: DiagramScope,
    pub tables: Vec<DiagramTable>,
    pub edges: Vec<DiagramEdge>,
    pub total_tables: usize,
    pub truncated: bool,
    pub config: DataSourceConfig,
    pub available_schemas: Vec<String>,
}

impl DiagramModel {
    pub fn build(config: &DataSourceConfig, schema: &DatabaseSchema, scope: DiagramScope) -> Self {
        let eligible_tables = schema
            .tables
            .iter()
            .filter(|table| matches!(table.kind, TableKind::Table | TableKind::ForeignTable))
            .map(|table| TableRef::new(&table.schema, &table.name))
            .collect::<BTreeSet<_>>();

        let requested_tables = requested_tables(schema, &eligible_tables, &scope);
        let total_tables = requested_tables.len();
        let (requested_tables, truncated) = cap_tables(requested_tables, &scope);

        let tables = schema
            .tables
            .iter()
            .filter(|table| requested_tables.contains(&TableRef::new(&table.schema, &table.name)))
            .map(|table| DiagramTable {
                id: TableRef::new(&table.schema, &table.name),
                columns: sorted_diagram_columns(
                    table
                        .columns
                        .iter()
                        .map(|column| DiagramColumn {
                            name: column.name.clone(),
                            data_type: column.data_type.clone(),
                            nullable: column.nullable,
                            is_pk: column.is_pk,
                            is_fk: column.is_fk,
                        })
                        .collect(),
                ),
            })
            .collect::<Vec<_>>();

        let edges = schema
            .foreign_keys
            .iter()
            .filter_map(|fk| {
                let source = TableRef::new(&fk.source_schema, &fk.source_table);
                let target = TableRef::new(&fk.target_schema, &fk.target_table);
                (requested_tables.contains(&source) && requested_tables.contains(&target)).then(
                    || DiagramEdge {
                        source,
                        source_columns: fk.source_columns.clone(),
                        target,
                        target_columns: fk.target_columns.clone(),
                    },
                )
            })
            .collect::<Vec<_>>();

        let available_schemas = schema
            .schemas
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>();

        Self {
            title: scope.tab_title(&config.name),
            scope,
            tables,
            edges,
            total_tables,
            truncated,
            config: config.clone(),
            available_schemas,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DiagramPoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug)]
pub struct DiagramNodeLayout {
    pub position: DiagramPoint,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug, Default)]
pub struct DiagramLayout {
    pub nodes: BTreeMap<TableRef, DiagramNodeLayout>,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug)]
struct DiagramRect {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl DiagramRect {
    fn center_x(&self) -> f32 {
        (self.left + self.right) / 2.0
    }

    fn expanded(&self, amount: f32) -> Self {
        Self {
            left: self.left - amount,
            top: self.top - amount,
            right: self.right + amount,
            bottom: self.bottom + amount,
        }
    }
}

pub fn layout_diagram(model: &DiagramModel) -> DiagramLayout {
    let table_ids = model
        .tables
        .iter()
        .map(|table| table.id.clone())
        .collect::<BTreeSet<_>>();
    let heights = model
        .tables
        .iter()
        .map(|table| (table.id.clone(), table.height()))
        .collect::<BTreeMap<_, _>>();
    let widths = model
        .tables
        .iter()
        .map(|table| (table.id.clone(), table.width()))
        .collect::<BTreeMap<_, _>>();
    let adjacency = adjacency_for(&table_ids, &model.edges);

    let mut remaining = table_ids.clone();
    let mut nodes = BTreeMap::new();
    let mut max_width = 0.0;
    let mut next_component_y = 32.0;

    while let Some(start) = remaining.iter().next().cloned() {
        let component = collect_component(&start, &adjacency);
        for id in &component {
            remaining.remove(id);
        }

        let root = choose_component_root(model, &component, &adjacency);
        let layers = layered_component(&root, &component, &adjacency, &model.edges);
        let mut component_bottom = next_component_y;
        let layer_widths = layers
            .iter()
            .map(|layer| {
                layer
                    .iter()
                    .filter_map(|id| widths.get(id))
                    .copied()
                    .fold(MIN_TABLE_WIDTH, f32::max)
            })
            .collect::<Vec<_>>();
        let mut layer_x = 32.0;

        for (layer_ix, layer) in layers.iter().enumerate() {
            let x = layer_x;
            let mut y = next_component_y;
            for table_id in layer {
                let height = *heights.get(table_id).unwrap_or(&HEADER_HEIGHT);
                let width = *widths.get(table_id).unwrap_or(&MIN_TABLE_WIDTH);
                nodes.insert(
                    table_id.clone(),
                    DiagramNodeLayout {
                        position: DiagramPoint { x, y },
                        width,
                        height,
                    },
                );
                y += height + TABLE_GAP_Y;
                component_bottom = component_bottom.max(y);
                max_width = f32::max(max_width, x + width + 32.0);
            }
            layer_x += layer_widths[layer_ix] + TABLE_GAP_X;
        }

        next_component_y = component_bottom + COMPONENT_GAP_Y;
    }

    DiagramLayout {
        nodes,
        width: max_width.max(720.0),
        height: next_component_y.max(480.0),
    }
}

pub struct DiagramPanel {
    model: DiagramModel,
    layout: DiagramLayout,
    positions: BTreeMap<TableRef, DiagramPoint>,
    focus_handle: FocusHandle,
    pan: DiagramPoint,
    zoom: f32,
    last_canvas_bounds: Option<Bounds<gpui::Pixels>>,
    dragging: Option<DragState>,
    panning: Option<PanState>,
    export_counter: usize,
    activity_tracker: gpui::Entity<ActivityTracker>,
}

#[derive(Clone, Debug)]
struct DragState {
    table: TableRef,
    mouse_start: DiagramPoint,
    table_start: DiagramPoint,
}

#[derive(Clone, Copy, Debug)]
struct PanState {
    mouse_start: DiagramPoint,
    pan_start: DiagramPoint,
}

#[derive(Clone)]
struct DiagramPaintState {
    model: DiagramModel,
    layout: DiagramLayout,
    positions: BTreeMap<TableRef, DiagramPoint>,
    pan: DiagramPoint,
    zoom: f32,
    text_style: TextStyle,
    background: Hsla,
    foreground: Hsla,
    muted: Hsla,
    border: Hsla,
    card: Hsla,
    header: Hsla,
    pk_marker: Hsla,
    fk_marker: Hsla,
    regular_marker: Hsla,
    grid: Hsla,
}

#[derive(Clone)]
struct DiagramExport {
    model: DiagramModel,
    layout: DiagramLayout,
    positions: BTreeMap<TableRef, DiagramPoint>,
    style: DiagramExportStyle,
}

#[derive(Clone)]
struct DiagramExportStyle {
    background: Rgba<u8>,
    card: Rgba<u8>,
    header: Rgba<u8>,
    border: Rgba<u8>,
    foreground: Rgba<u8>,
    muted: Rgba<u8>,
    edge: Rgba<u8>,
    grid: Rgba<u8>,
    pk_marker: Rgba<u8>,
    fk_marker: Rgba<u8>,
    regular_marker: Rgba<u8>,
}

impl DiagramExportStyle {
    fn from_app(cx: &App) -> Self {
        Self {
            background: rgba_from_hsla(cx.theme().background),
            card: rgba_from_hsla(cx.theme().popover),
            header: rgba_from_hsla(cx.theme().tab_bar),
            border: rgba_from_hsla(cx.theme().border),
            foreground: rgba_from_hsla(cx.theme().foreground),
            muted: rgba_from_hsla(cx.theme().muted_foreground),
            edge: rgba_from_hsla(cx.theme().muted_foreground.opacity(0.82)),
            grid: rgba_from_hsla(cx.theme().border.opacity(0.25)),
            pk_marker: rgba(0xf0c674),
            fk_marker: rgba(0x6cb6ff),
            regular_marker: rgba_from_hsla(cx.theme().muted_foreground.opacity(0.9)),
        }
    }
}

impl Default for DiagramExportStyle {
    fn default() -> Self {
        Self {
            background: rgba(0x111322),
            card: rgba(0x27293b),
            header: rgba(0x25273a),
            border: rgba(0x3d4156),
            foreground: rgba(0xdfe3f3),
            muted: rgba(0xa7adbf),
            edge: rgba_alpha(0xa7adbf, 210),
            grid: rgba_alpha(0x3d4156, 95),
            pk_marker: rgba(0xf0c674),
            fk_marker: rgba(0x6cb6ff),
            regular_marker: rgba(0xa7adbf),
        }
    }
}

impl DiagramPanel {
    pub fn new(
        model: DiagramModel,
        activity_tracker: gpui::Entity<ActivityTracker>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let layout = layout_diagram(&model);
        let positions = layout
            .nodes
            .iter()
            .map(|(id, node)| (id.clone(), node.position))
            .collect();

        Self {
            model,
            layout,
            positions,
            focus_handle: cx.focus_handle(),
            pan: DiagramPoint { x: 40.0, y: 40.0 },
            zoom: 1.0,
            last_canvas_bounds: None,
            dragging: None,
            panning: None,
            export_counter: 1,
            activity_tracker,
        }
    }

    pub fn title(&self) -> &str {
        &self.model.title
    }

    fn zoom_in(&mut self, cx: &mut Context<Self>) {
        self.zoom = (self.zoom * 1.15).min(2.5);
        cx.notify();
    }

    fn zoom_out(&mut self, cx: &mut Context<Self>) {
        self.zoom = (self.zoom / 1.15).max(0.25);
        cx.notify();
    }

    fn reset_zoom(&mut self, cx: &mut Context<Self>) {
        self.zoom = 1.0;
        self.pan = DiagramPoint { x: 40.0, y: 40.0 };
        cx.notify();
    }

    fn select_schema(&mut self, schema_name: &str, cx: &mut Context<Self>) {
        let cache_key = schema_cache::cache_key(&self.model.config);
        let Ok(Some(schema)) = schema_cache::load(&cache_key) else {
            return;
        };

        let new_scope = DiagramScope::Schema(schema_name.to_string());
        let new_model = DiagramModel::build(&self.model.config, &schema, new_scope);

        let layout = layout_diagram(&new_model);
        let positions = layout
            .nodes
            .iter()
            .map(|(id, node)| (id.clone(), node.position))
            .collect();

        self.model = new_model;
        self.layout = layout;
        self.positions = positions;
        self.pan = DiagramPoint { x: 40.0, y: 40.0 };
        self.zoom = 1.0;
        cx.notify();
    }

    fn select_all_schemas(&mut self, cx: &mut Context<Self>) {
        let cache_key = schema_cache::cache_key(&self.model.config);
        let Ok(Some(schema)) = schema_cache::load(&cache_key) else {
            return;
        };

        let new_scope = DiagramScope::Database;
        let new_model = DiagramModel::build(&self.model.config, &schema, new_scope);

        let layout = layout_diagram(&new_model);
        let positions = layout
            .nodes
            .iter()
            .map(|(id, node)| (id.clone(), node.position))
            .collect();

        self.model = new_model;
        self.layout = layout;
        self.positions = positions;
        self.pan = DiagramPoint { x: 40.0, y: 40.0 };
        self.zoom = 1.0;
        cx.notify();
    }

    fn current_schema_label(&self) -> String {
        match &self.model.scope {
            DiagramScope::Database => "All schemas".to_string(),
            DiagramScope::Schema(name) => name.clone(),
            DiagramScope::Table { schema, table } => format!("{}.{}", schema, table),
        }
    }

    fn export_diagram(
        &mut self,
        format: DiagramExportFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let default_path = format!(
            "{}/{}_{}.{}",
            home,
            sanitize_file_name(&self.model.title),
            self.export_counter,
            format.extension()
        );
        self.export_counter += 1;

        let export = DiagramExport {
            model: self.model.clone(),
            layout: self.layout.clone(),
            positions: self.positions.clone(),
            style: DiagramExportStyle::from_app(cx),
        };
        let input_state = cx.new(|cx| InputState::new(window, cx).default_value(default_path));
        let input_state_for_ok = input_state.clone();
        let activity_tracker = self.activity_tracker.clone();

        window.open_alert_dialog(cx, move |alert, _window, _cx| {
            alert
                .title(format!("Export diagram to {}", format.label()))
                .child(Input::new(&input_state))
                .show_cancel(true)
                .on_ok({
                    let input_state_for_ok = input_state_for_ok.clone();
                    let export = export.clone();
                    let activity_tracker = activity_tracker.clone();
                    move |_, _window, cx| {
                        let path = input_state_for_ok.read(cx).value().to_string();
                        let activity_id = cx.update_entity(&activity_tracker, |tracker, cx| {
                            tracker.begin(format!("Exporting diagram to {}", format.label()), cx)
                        });

                        let export_for_spawn = export.clone();
                        let activity_tracker_for_spawn = activity_tracker.clone();
                        cx.spawn(async move |cx| {
                            let result = cx
                                .background_executor()
                                .spawn(async move {
                                    write_diagram_export_file(&export_for_spawn, format, &path)
                                })
                                .await;

                            if let Err(error) = result {
                                eprintln!(
                                    "failed to export diagram as {}: {error}",
                                    format.label()
                                );
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

    fn fit_to_screen(&mut self, bounds: Bounds<gpui::Pixels>, cx: &mut Context<Self>) {
        let available_width = (f32::from(bounds.size.width) - 80.0).max(320.0);
        let available_height = (f32::from(bounds.size.height) - 80.0).max(240.0);
        let scale_x = available_width / self.layout.width.max(1.0);
        let scale_y = available_height / self.layout.height.max(1.0);
        self.zoom = scale_x.min(scale_y).clamp(0.25, 1.5);
        self.pan = DiagramPoint { x: 40.0, y: 40.0 };
        cx.notify();
    }

    fn paint_state(
        &mut self,
        bounds: Bounds<gpui::Pixels>,
        _window: &mut Window,
        cx: &App,
    ) -> DiagramPaintState {
        self.last_canvas_bounds = Some(bounds);
        let font_size = cx.theme().font_size;
        DiagramPaintState {
            model: self.model.clone(),
            layout: self.layout.clone(),
            positions: self.positions.clone(),
            pan: self.pan,
            zoom: self.zoom,
            text_style: TextStyle {
                font_family: cx.theme().font_family.clone(),
                font_size: font_size.into(),
                line_height: font_size.into(),
                white_space: WhiteSpace::Normal,
                background_color: Some(cx.theme().background),
                color: cx.theme().foreground,
                ..Default::default()
            },
            background: cx.theme().background,
            foreground: cx.theme().foreground,
            muted: cx.theme().muted_foreground,
            border: cx.theme().border,
            card: cx.theme().popover,
            header: cx.theme().tab_bar,
            pk_marker: rgb(0xf0c674).into(),
            fk_marker: rgb(0x6cb6ff).into(),
            regular_marker: cx.theme().muted_foreground.opacity(0.9),
            grid: cx.theme().border.opacity(0.25),
        }
    }

    fn screen_to_diagram(
        &self,
        bounds: Bounds<gpui::Pixels>,
        point: gpui::Point<gpui::Pixels>,
    ) -> DiagramPoint {
        DiagramPoint {
            x: (f32::from(point.x) - f32::from(bounds.origin.x) - self.pan.x) / self.zoom,
            y: (f32::from(point.y) - f32::from(bounds.origin.y) - self.pan.y) / self.zoom,
        }
    }

    fn hit_table(&self, diagram_point: DiagramPoint) -> Option<TableRef> {
        self.model.tables.iter().rev().find_map(|table| {
            let position = self.positions.get(&table.id)?;
            let height = self
                .layout
                .nodes
                .get(&table.id)
                .map(|node| node.height)
                .unwrap_or_else(|| table.height());
            let width = self
                .layout
                .nodes
                .get(&table.id)
                .map(|node| node.width)
                .unwrap_or_else(|| table.width());
            let hit = diagram_point.x >= position.x
                && diagram_point.x <= position.x + width
                && diagram_point.y >= position.y
                && diagram_point.y <= position.y + height;
            hit.then(|| table.id.clone())
        })
    }

    fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        bounds: Bounds<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left || !bounds.contains(&event.position) {
            return;
        }

        let diagram_point = self.screen_to_diagram(bounds, event.position);
        if let Some(table) = self.hit_table(diagram_point) {
            let table_start = self.positions.get(&table).copied().unwrap_or_default();
            self.dragging = Some(DragState {
                table,
                mouse_start: diagram_point,
                table_start,
            });
        } else {
            self.panning = Some(PanState {
                mouse_start: DiagramPoint {
                    x: f32::from(event.position.x),
                    y: f32::from(event.position.y),
                },
                pan_start: self.pan,
            });
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn handle_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        bounds: Bounds<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        if !event.dragging() {
            return;
        }

        if let Some(dragging) = self.dragging.clone() {
            let diagram_point = self.screen_to_diagram(bounds, event.position);
            self.positions.insert(
                dragging.table,
                DiagramPoint {
                    x: dragging.table_start.x + diagram_point.x - dragging.mouse_start.x,
                    y: dragging.table_start.y + diagram_point.y - dragging.mouse_start.y,
                },
            );
            cx.stop_propagation();
            cx.notify();
            return;
        }

        if let Some(panning) = self.panning {
            self.pan = DiagramPoint {
                x: panning.pan_start.x + f32::from(event.position.x) - panning.mouse_start.x,
                y: panning.pan_start.y + f32::from(event.position.y) - panning.mouse_start.y,
            };
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn handle_mouse_up(&mut self, _event: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.dragging.is_some() || self.panning.is_some() {
            self.dragging = None;
            self.panning = None;
            cx.notify();
        }
    }

    fn handle_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        bounds: Bounds<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        if !bounds.contains(&event.position) {
            return;
        }
        let delta = event.delta.pixel_delta(px(18.0));
        self.pan.x += f32::from(delta.x);
        self.pan.y += f32::from(delta.y);
        cx.stop_propagation();
        cx.notify();
    }
}

impl EventEmitter<PanelEvent> for DiagramPanel {}

impl Panel for DiagramPanel {
    fn panel_name(&self) -> &'static str {
        "DiagramPanel"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.model.title.clone()
    }

    fn closable(&self, _cx: &App) -> bool {
        true
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState::new(self)
    }
}

impl Render for DiagramPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let view_for_export = entity.clone();
        let zoom_label = format!("{:.0}%", self.zoom * 100.0);
        let banner = self.model.truncated.then(|| {
            format!(
                "Showing {} of {} tables. Narrow the scope to inspect the full schema.",
                self.model.tables.len(),
                self.model.total_tables
            )
        });

        v_flex()
            .id("diagram-panel")
            .size_full()
            .track_focus(&self.focus_handle)
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .id("diagram-toolbar")
                    .h(px(32.0))
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().tab_bar)
                    .child(
                        Button::new("diagram-zoom-out")
                            .icon(IconName::Minus)
                            .xsmall()
                            .ghost()
                            .tooltip("Zoom out")
                            .on_click(cx.listener(|this, _, _, cx| this.zoom_out(cx))),
                    )
                    .child(
                        Button::new("diagram-zoom-reset")
                            .label(zoom_label)
                            .xsmall()
                            .ghost()
                            .tooltip("Reset zoom")
                            .on_click(cx.listener(|this, _, _, cx| this.reset_zoom(cx))),
                    )
                    .child(
                        Button::new("diagram-zoom-in")
                            .icon(IconName::Plus)
                            .xsmall()
                            .ghost()
                            .tooltip("Zoom in")
                            .on_click(cx.listener(|this, _, _, cx| this.zoom_in(cx))),
                    )
                    .child(
                        Button::new("diagram-fit")
                            .icon(IconName::Maximize)
                            .xsmall()
                            .ghost()
                            .tooltip("Fit to screen")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                if let Some(bounds) = this.last_canvas_bounds {
                                    this.fit_to_screen(bounds, cx);
                                } else {
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        Button::new("diagram-export")
                            .icon(IconName::ArrowDown)
                            .xsmall()
                            .ghost()
                            .tooltip("Export diagram")
                            .dropdown_menu(move |menu, window, _cx| {
                                let mut menu = menu;
                                for format in DiagramExportFormat::ALL {
                                    menu = menu.item(
                                        PopupMenuItem::new(format!("Export {}", format.label()))
                                            .icon(IconName::File)
                                            .on_click(window.listener_for(
                                                &view_for_export,
                                                move |this, _, window, cx| {
                                                    this.export_diagram(format, window, cx);
                                                },
                                            )),
                                    );
                                }
                                menu
                            }),
                    )
                    .child(div().flex_1())
                    .child({
                        let view_for_schema = entity.clone();
                        let schema_label = self.current_schema_label();
                        let available_schemas = self.model.available_schemas.clone();
                        let has_schemas = available_schemas.len() > 1;
                        if has_schemas {
                            Button::new("diagram-schema-selector")
                                .label(schema_label)
                                .xsmall()
                                .ghost()
                                .tooltip("Select schema")
                                .dropdown_menu(move |menu, window, _cx| {
                                    let mut menu = menu;
                                    menu = menu.item(PopupMenuItem::new("All schemas").on_click(
                                        window.listener_for(
                                            &view_for_schema,
                                            move |this, _, _window, cx| {
                                                this.select_all_schemas(cx);
                                            },
                                        ),
                                    ));
                                    for schema_name in &available_schemas {
                                        let view_for_item = view_for_schema.clone();
                                        let schema_name = schema_name.clone();
                                        menu = menu.item(
                                            PopupMenuItem::new(schema_name.clone()).on_click(
                                                window.listener_for(
                                                    &view_for_item,
                                                    move |this, _, _window, cx| {
                                                        this.select_schema(&schema_name, cx);
                                                    },
                                                ),
                                            ),
                                        );
                                    }
                                    menu
                                })
                                .into_any_element()
                        } else {
                            div().into_any_element()
                        }
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.model.title.clone()),
                    ),
            )
            .when_some(banner, |this, banner| {
                this.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .bg(cx.theme().warning.opacity(0.12))
                        .text_color(cx.theme().warning)
                        .child(banner),
                )
            })
            .child(
                div()
                    .id("diagram-canvas")
                    .flex_1()
                    .relative()
                    .overflow_hidden()
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.focus(&this.focus_handle, cx);
                    }))
                    .child(
                        canvas(
                            {
                                let entity = entity.clone();
                                move |bounds, window, cx| {
                                    entity
                                        .update(cx, |this, cx| this.paint_state(bounds, window, cx))
                                }
                            },
                            {
                                let entity = entity.clone();
                                move |bounds, state, window, cx| {
                                    paint_diagram(bounds, &state, window, cx);

                                    let view_id = window.current_view();
                                    let mouse_bounds = bounds;
                                    window.on_mouse_event({
                                        let entity = entity.clone();
                                        move |event: &MouseDownEvent, phase, _window, cx| {
                                            if !phase.bubble() {
                                                return;
                                            }
                                            let _ = entity.update(cx, |this, cx| {
                                                this.handle_mouse_down(event, mouse_bounds, cx);
                                            });
                                        }
                                    });
                                    window.on_mouse_event({
                                        let entity = entity.clone();
                                        move |event: &MouseMoveEvent, phase, _window, cx| {
                                            if !phase.bubble() {
                                                return;
                                            }
                                            let _ = entity.update(cx, |this, cx| {
                                                this.handle_mouse_move(event, mouse_bounds, cx);
                                            });
                                        }
                                    });
                                    window.on_mouse_event({
                                        let entity = entity.clone();
                                        move |event: &MouseUpEvent, phase, _window, cx| {
                                            if !phase.bubble() {
                                                return;
                                            }
                                            let _ = entity.update(cx, |this, cx| {
                                                this.handle_mouse_up(event, cx);
                                            });
                                        }
                                    });
                                    window.on_mouse_event(
                                        move |event: &ScrollWheelEvent, phase, _window, cx| {
                                            if !phase.bubble() {
                                                return;
                                            }
                                            let _ = entity.update(cx, |this, cx| {
                                                this.handle_scroll(event, mouse_bounds, cx);
                                            });
                                            cx.notify(view_id);
                                        },
                                    );
                                }
                            },
                        )
                        .size_full(),
                    ),
            )
    }
}

impl Focusable for DiagramPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn requested_tables(
    schema: &DatabaseSchema,
    eligible_tables: &BTreeSet<TableRef>,
    scope: &DiagramScope,
) -> Vec<TableRef> {
    match scope {
        DiagramScope::Database => eligible_tables.iter().cloned().collect(),
        DiagramScope::Schema(schema_name) => eligible_tables
            .iter()
            .filter(|table| table.schema == *schema_name)
            .cloned()
            .collect(),
        DiagramScope::Table {
            schema: schema_name,
            table,
        } => {
            let start = TableRef::new(schema_name, table);
            if !eligible_tables.contains(&start) {
                return Vec::new();
            }
            recursive_fk_tables(schema, eligible_tables, start)
        }
    }
}

fn recursive_fk_tables(
    schema: &DatabaseSchema,
    eligible_tables: &BTreeSet<TableRef>,
    start: TableRef,
) -> Vec<TableRef> {
    let mut adjacency = BTreeMap::<TableRef, BTreeSet<TableRef>>::new();
    for fk in &schema.foreign_keys {
        let source = TableRef::new(&fk.source_schema, &fk.source_table);
        let target = TableRef::new(&fk.target_schema, &fk.target_table);
        if eligible_tables.contains(&source) && eligible_tables.contains(&target) {
            adjacency
                .entry(source.clone())
                .or_default()
                .insert(target.clone());
            adjacency.entry(target).or_default().insert(source);
        }
    }

    let mut visited = BTreeSet::new();
    let mut ordered = Vec::new();
    let mut queue = VecDeque::from([start.clone()]);
    visited.insert(start);

    while let Some(table) = queue.pop_front() {
        ordered.push(table.clone());
        if let Some(neighbors) = adjacency.get(&table) {
            for neighbor in neighbors {
                if visited.insert(neighbor.clone()) {
                    queue.push_back(neighbor.clone());
                }
            }
        }
    }

    ordered
}

fn cap_tables(tables: Vec<TableRef>, scope: &DiagramScope) -> (BTreeSet<TableRef>, bool) {
    let truncated = tables.len() > MAX_DIAGRAM_TABLES;
    let mut capped = tables;
    if !matches!(scope, DiagramScope::Table { .. }) {
        capped.sort();
    }
    capped.truncate(MAX_DIAGRAM_TABLES);
    (capped.into_iter().collect(), truncated)
}

fn sorted_diagram_columns(columns: Vec<DiagramColumn>) -> Vec<DiagramColumn> {
    let mut regular = Vec::new();
    let mut primary_keys = Vec::new();
    for column in columns {
        if column.is_pk {
            primary_keys.push(column);
        } else {
            regular.push(column);
        }
    }
    regular.extend(primary_keys);
    regular
}

fn adjacency_for(
    table_ids: &BTreeSet<TableRef>,
    edges: &[DiagramEdge],
) -> BTreeMap<TableRef, BTreeSet<TableRef>> {
    let mut adjacency = table_ids
        .iter()
        .map(|id| (id.clone(), BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    for edge in edges {
        if table_ids.contains(&edge.source) && table_ids.contains(&edge.target) {
            adjacency
                .entry(edge.source.clone())
                .or_default()
                .insert(edge.target.clone());
            adjacency
                .entry(edge.target.clone())
                .or_default()
                .insert(edge.source.clone());
        }
    }
    adjacency
}

fn collect_component(
    start: &TableRef,
    adjacency: &BTreeMap<TableRef, BTreeSet<TableRef>>,
) -> BTreeSet<TableRef> {
    let mut component = BTreeSet::new();
    let mut queue = VecDeque::from([start.clone()]);
    component.insert(start.clone());
    while let Some(table) = queue.pop_front() {
        if let Some(neighbors) = adjacency.get(&table) {
            for neighbor in neighbors {
                if component.insert(neighbor.clone()) {
                    queue.push_back(neighbor.clone());
                }
            }
        }
    }
    component
}

fn choose_component_root(
    model: &DiagramModel,
    component: &BTreeSet<TableRef>,
    adjacency: &BTreeMap<TableRef, BTreeSet<TableRef>>,
) -> TableRef {
    if let Some(selected) = model.scope.selected_table() {
        if component.contains(&selected) {
            return selected;
        }
    }

    component
        .iter()
        .max_by_key(|table| adjacency.get(*table).map(|n| n.len()).unwrap_or_default())
        .cloned()
        .unwrap_or_else(|| component.iter().next().cloned().unwrap())
}

fn layered_component(
    root: &TableRef,
    component: &BTreeSet<TableRef>,
    adjacency: &BTreeMap<TableRef, BTreeSet<TableRef>>,
    edges: &[DiagramEdge],
) -> Vec<Vec<TableRef>> {
    let mut distance = BTreeMap::from([(root.clone(), 0usize)]);
    let mut queue = VecDeque::from([root.clone()]);
    while let Some(table) = queue.pop_front() {
        let next_distance = distance[&table] + 1;
        if let Some(neighbors) = adjacency.get(&table) {
            for neighbor in neighbors {
                if component.contains(neighbor) && !distance.contains_key(neighbor) {
                    distance.insert(neighbor.clone(), next_distance);
                    queue.push_back(neighbor.clone());
                }
            }
        }
    }

    let mut layers = BTreeMap::<usize, Vec<TableRef>>::new();
    for table in component {
        let layer = distance.get(table).copied().unwrap_or_default();
        layers.entry(layer).or_default().push(table.clone());
    }
    let mut layers = layers.into_values().collect::<Vec<_>>();
    reduce_layer_crossings(&mut layers, edges);
    layers
}

fn reduce_layer_crossings(layers: &mut [Vec<TableRef>], edges: &[DiagramEdge]) {
    let mut edge_pairs = Vec::new();
    for edge in edges {
        edge_pairs.push((edge.source.clone(), edge.target.clone()));
        edge_pairs.push((edge.target.clone(), edge.source.clone()));
    }

    for _ in 0..3 {
        for layer_ix in 1..layers.len() {
            sort_layer_by_neighbor_barycenter(layer_ix, layer_ix - 1, layers, &edge_pairs);
        }
        for layer_ix in (0..layers.len().saturating_sub(1)).rev() {
            sort_layer_by_neighbor_barycenter(layer_ix, layer_ix + 1, layers, &edge_pairs);
        }
    }
}

fn sort_layer_by_neighbor_barycenter(
    layer_ix: usize,
    neighbor_layer_ix: usize,
    layers: &mut [Vec<TableRef>],
    edge_pairs: &[(TableRef, TableRef)],
) {
    let neighbor_positions = layers[neighbor_layer_ix]
        .iter()
        .enumerate()
        .map(|(ix, table)| (table.clone(), ix as f32))
        .collect::<BTreeMap<_, _>>();

    layers[layer_ix].sort_by(|left, right| {
        let left_center = neighbor_barycenter(left, &neighbor_positions, edge_pairs);
        let right_center = neighbor_barycenter(right, &neighbor_positions, edge_pairs);
        left_center
            .partial_cmp(&right_center)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.cmp(right))
    });
}

fn neighbor_barycenter(
    table: &TableRef,
    neighbor_positions: &BTreeMap<TableRef, f32>,
    edge_pairs: &[(TableRef, TableRef)],
) -> f32 {
    let mut total = 0.0;
    let mut count = 0.0;
    for (from, to) in edge_pairs {
        if from == table {
            if let Some(position) = neighbor_positions.get(to) {
                total += *position;
                count += 1.0;
            }
        }
    }
    if count == 0.0 {
        f32::MAX
    } else {
        total / count
    }
}

fn paint_diagram(
    bounds: Bounds<gpui::Pixels>,
    state: &DiagramPaintState,
    window: &mut Window,
    cx: &mut App,
) {
    window.paint_quad(fill(bounds, state.background));
    paint_grid(bounds, state, window);
    paint_edges(bounds, state, window);
    for table in &state.model.tables {
        paint_table(bounds, state, table, window, cx);
    }
}

fn paint_grid(bounds: Bounds<gpui::Pixels>, state: &DiagramPaintState, window: &mut Window) {
    let spacing = 24.0 * state.zoom;
    if spacing < 6.0 {
        return;
    }
    let start_x = f32::from(bounds.origin.x) + state.pan.x.rem_euclid(spacing);
    let start_y = f32::from(bounds.origin.y) + state.pan.y.rem_euclid(spacing);
    let mut x = start_x;
    while x < f32::from(bounds.right()) {
        window.paint_quad(fill(
            Bounds::new(
                point(px(x), bounds.origin.y),
                size(px(1.0), bounds.size.height),
            ),
            state.grid,
        ));
        x += spacing;
    }
    let mut y = start_y;
    while y < f32::from(bounds.bottom()) {
        window.paint_quad(fill(
            Bounds::new(
                point(bounds.origin.x, px(y)),
                size(bounds.size.width, px(1.0)),
            ),
            state.grid,
        ));
        y += spacing;
    }
}

fn paint_edges(bounds: Bounds<gpui::Pixels>, state: &DiagramPaintState, window: &mut Window) {
    for edge in &state.model.edges {
        let route = route_edge_points(&state.model, &state.layout, &state.positions, edge);
        let Some(first) = route.first().copied() else {
            continue;
        };

        let mut path = PathBuilder::stroke(px(1.3));
        path.move_to(diagram_to_screen(bounds, state, first));
        for point in route.iter().skip(1) {
            path.line_to(diagram_to_screen(bounds, state, *point));
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, state.muted.opacity(0.82));
        }

        // Draw arrow at the end of the route
        if route.len() >= 2 {
            let last = route[route.len() - 1];
            let second_last = route[route.len() - 2];
            let end = diagram_to_screen(bounds, state, last);
            let prev = diagram_to_screen(bounds, state, second_last);
            paint_arrow(end, prev, state.muted.opacity(0.82), window);
        }
    }
}

fn paint_arrow(
    tip: gpui::Point<gpui::Pixels>,
    base: gpui::Point<gpui::Pixels>,
    color: Hsla,
    window: &mut Window,
) {
    let dx = f32::from(tip.x) - f32::from(base.x);
    let dy = f32::from(tip.y) - f32::from(base.y);
    let length = (dx * dx + dy * dy).sqrt();
    if length < 2.0 {
        return;
    }

    let dir_x = dx / length;
    let dir_y = dy / length;

    // Arrow tip slightly before actual end
    let tip_x = f32::from(tip.x) - dir_x * 2.0;
    let tip_y = f32::from(tip.y) - dir_y * 2.0;

    let arrow_size = 8.0;
    let base_x = tip_x - dir_x * arrow_size;
    let base_y = tip_y - dir_y * arrow_size;

    let perp_x = -dir_y;
    let perp_y = dir_x;
    let wing_span = arrow_size * 0.6;

    let left = point(
        px(base_x + perp_x * wing_span),
        px(base_y + perp_y * wing_span),
    );
    let right = point(
        px(base_x - perp_x * wing_span),
        px(base_y - perp_y * wing_span),
    );
    let tip_pt = point(px(tip_x), px(tip_y));

    let mut path = PathBuilder::fill();
    path.move_to(tip_pt);
    path.line_to(left);
    path.line_to(right);
    path.line_to(tip_pt);
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn paint_table(
    bounds: Bounds<gpui::Pixels>,
    state: &DiagramPaintState,
    table: &DiagramTable,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(position) = state.positions.get(&table.id) else {
        return;
    };
    let height = state
        .layout
        .nodes
        .get(&table.id)
        .map(|node| node.height)
        .unwrap_or_else(|| table.height());
    let width = state
        .layout
        .nodes
        .get(&table.id)
        .map(|node| node.width)
        .unwrap_or_else(|| table.width());
    let origin = diagram_to_screen(bounds, state, *position);
    let table_bounds = Bounds::new(
        origin,
        size(px(width * state.zoom), px(height * state.zoom)),
    );
    let header_bounds = Bounds::new(
        origin,
        size(px(width * state.zoom), px(HEADER_HEIGHT * state.zoom)),
    );

    window.paint_quad(fill(table_bounds, state.card));
    window.paint_quad(fill(header_bounds, state.header));
    window.paint_quad(gpui::outline(
        table_bounds,
        state.border,
        BorderStyle::default(),
    ));

    paint_text_aligned_px(
        &table.id.display_label(),
        f32::from(origin.x),
        f32::from(origin.y) + 6.0 * state.zoom,
        state.foreground,
        HEADER_FONT_PX * state.zoom,
        true,
        gpui::TextAlign::Center,
        Some(width * state.zoom),
        state,
        window,
        cx,
    );

    if table.columns.is_empty() {
        paint_text_px(
            "(no columns)",
            f32::from(origin.x) + 10.0 * state.zoom,
            f32::from(origin.y) + (HEADER_HEIGHT + 5.0) * state.zoom,
            state.muted,
            COLUMN_FONT_PX * state.zoom,
            false,
            state,
            window,
            cx,
        );
        return;
    }

    for (ix, column) in table.columns.iter().enumerate() {
        let y = f32::from(origin.y) + (HEADER_HEIGHT + ix as f32 * ROW_HEIGHT) * state.zoom;
        let is_first_pk_row = column.is_pk
            && ix > 0
            && table
                .columns
                .get(ix - 1)
                .map(|previous| !previous.is_pk)
                .unwrap_or(false);
        if ix > 0 || is_first_pk_row {
            window.paint_quad(fill(
                Bounds::new(
                    point(origin.x, px(y)),
                    size(
                        px(width * state.zoom),
                        px(if is_first_pk_row { 2.0 } else { 1.0 }),
                    ),
                ),
                if is_first_pk_row {
                    state.border.opacity(0.95)
                } else {
                    state.border.opacity(0.45)
                },
            ));
        }
        let marker = if column.is_pk {
            "★"
        } else if column.is_fk {
            "→"
        } else {
            "•"
        };
        paint_text_px(
            marker,
            f32::from(origin.x) + 8.0 * state.zoom,
            y + 4.0 * state.zoom,
            if column.is_pk {
                state.pk_marker
            } else if column.is_fk {
                state.fk_marker
            } else {
                state.regular_marker
            },
            MARKER_FONT_PX * state.zoom,
            column.is_pk,
            state,
            window,
            cx,
        );
        let type_width = TYPE_COL_WIDTH * state.zoom;
        let type_x = f32::from(origin.x) + (width - TABLE_PADDING_X - TYPE_COL_WIDTH) * state.zoom;
        let name_x = f32::from(origin.x) + (TABLE_PADDING_X + MARKER_COL_WIDTH) * state.zoom;
        let name_width =
            (width - TABLE_PADDING_X * 3.0 - MARKER_COL_WIDTH - TYPE_COL_WIDTH).max(64.0);
        let name_max_chars = (name_width / 6.4).floor().max(6.0) as usize;
        let type_max_chars = (TYPE_COL_WIDTH / 6.1).floor().max(6.0) as usize;
        paint_text_px(
            &ellipsize(&column.name, name_max_chars),
            name_x,
            y + 4.0 * state.zoom,
            state.foreground,
            COLUMN_FONT_PX * state.zoom,
            false,
            state,
            window,
            cx,
        );
        let type_color = if column.nullable {
            state.muted
        } else {
            state.foreground.opacity(0.78)
        };
        paint_text_aligned_px(
            &ellipsize(&column.data_type, type_max_chars),
            type_x,
            y + 4.0 * state.zoom,
            type_color,
            TYPE_FONT_PX * state.zoom,
            false,
            gpui::TextAlign::Right,
            Some(type_width),
            state,
            window,
            cx,
        );
    }
}

fn paint_text_px(
    text: &str,
    x: f32,
    y: f32,
    color: Hsla,
    font_size: f32,
    bold: bool,
    state: &DiagramPaintState,
    window: &mut Window,
    cx: &mut App,
) {
    paint_text_aligned_px(
        text,
        x,
        y,
        color,
        font_size,
        bold,
        gpui::TextAlign::Left,
        None,
        state,
        window,
        cx,
    );
}

fn paint_text_aligned_px(
    text: &str,
    x: f32,
    y: f32,
    color: Hsla,
    font_size: f32,
    bold: bool,
    align: gpui::TextAlign,
    align_width: Option<f32>,
    state: &DiagramPaintState,
    window: &mut Window,
    cx: &mut App,
) {
    let mut font = state.text_style.font();
    if bold {
        font = font.bold();
    }
    let line_height = px((font_size + 4.0).max(14.0));
    let run = TextRun {
        len: text.len(),
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped =
        window
            .text_system()
            .shape_line(text.to_string().into(), px(font_size), &[run], None);
    let _ = shaped.paint(
        point(px(x), px(y)),
        line_height,
        align,
        align_width.map(px),
        window,
        cx,
    );
}

fn diagram_to_screen(
    bounds: Bounds<gpui::Pixels>,
    state: &DiagramPaintState,
    point: DiagramPoint,
) -> gpui::Point<gpui::Pixels> {
    gpui::point(
        px(f32::from(bounds.origin.x) + state.pan.x + point.x * state.zoom),
        px(f32::from(bounds.origin.y) + state.pan.y + point.y * state.zoom),
    )
}

fn route_edge_points(
    model: &DiagramModel,
    layout: &DiagramLayout,
    positions: &BTreeMap<TableRef, DiagramPoint>,
    edge: &DiagramEdge,
) -> Vec<DiagramPoint> {
    let Some(source_rect) = table_rect(model, layout, positions, &edge.source) else {
        return Vec::new();
    };
    let Some(target_rect) = table_rect(model, layout, positions, &edge.target) else {
        return Vec::new();
    };

    let source_y =
        source_rect.top + column_center_y(model, &edge.source, edge.source_columns.first());
    let target_y =
        target_rect.top + column_center_y(model, &edge.target, edge.target_columns.first());
    let (start, start_direction, end, end_direction) = if source_rect.right <= target_rect.left {
        (
            DiagramPoint {
                x: source_rect.right,
                y: source_y,
            },
            1.0,
            DiagramPoint {
                x: target_rect.left,
                y: target_y,
            },
            -1.0,
        )
    } else if target_rect.right <= source_rect.left {
        (
            DiagramPoint {
                x: source_rect.left,
                y: source_y,
            },
            -1.0,
            DiagramPoint {
                x: target_rect.right,
                y: target_y,
            },
            1.0,
        )
    } else if source_rect.center_x() <= target_rect.center_x() {
        (
            DiagramPoint {
                x: source_rect.right,
                y: source_y,
            },
            1.0,
            DiagramPoint {
                x: target_rect.right,
                y: target_y,
            },
            1.0,
        )
    } else {
        (
            DiagramPoint {
                x: source_rect.left,
                y: source_y,
            },
            -1.0,
            DiagramPoint {
                x: target_rect.left,
                y: target_y,
            },
            -1.0,
        )
    };
    let route_start = DiagramPoint {
        x: start.x + start_direction * EDGE_PORT_STUB,
        y: start.y,
    };
    let route_end = DiagramPoint {
        x: end.x + end_direction * EDGE_PORT_STUB,
        y: end.y,
    };

    let obstacles = edge_route_obstacles(model, layout, positions, edge);
    if obstacles.is_empty() {
        return compact_route(vec![start, route_start, route_end, end]);
    }

    let (min_x, min_y, max_x, max_y) = diagram_obstacle_bounds(&obstacles, route_start, route_end);
    let mut x_lanes = vec![
        route_start.x,
        route_end.x,
        (route_start.x + route_end.x) / 2.0,
        min_x - EDGE_CLEARANCE,
        max_x + EDGE_CLEARANCE,
    ];
    let mut y_lanes = vec![
        route_start.y,
        route_end.y,
        (route_start.y + route_end.y) / 2.0,
        min_y - EDGE_CLEARANCE,
        max_y + EDGE_CLEARANCE,
    ];
    for obstacle in &obstacles {
        x_lanes.push(obstacle.left);
        x_lanes.push(obstacle.right);
        y_lanes.push(obstacle.top);
        y_lanes.push(obstacle.bottom);
    }
    sort_dedup_lanes(&mut x_lanes);
    sort_dedup_lanes(&mut y_lanes);

    let mut best_route = None::<(f32, Vec<DiagramPoint>)>;
    for lane_x in &x_lanes {
        consider_route(
            &mut best_route,
            vec![
                route_start,
                DiagramPoint {
                    x: *lane_x,
                    y: route_start.y,
                },
                DiagramPoint {
                    x: *lane_x,
                    y: route_end.y,
                },
                route_end,
            ],
            &obstacles,
        );
    }
    for lane_y in &y_lanes {
        consider_route(
            &mut best_route,
            vec![
                route_start,
                DiagramPoint {
                    x: route_start.x,
                    y: *lane_y,
                },
                DiagramPoint {
                    x: route_end.x,
                    y: *lane_y,
                },
                route_end,
            ],
            &obstacles,
        );
    }
    for lane_x in &x_lanes {
        for lane_y in &y_lanes {
            consider_route(
                &mut best_route,
                vec![
                    route_start,
                    DiagramPoint {
                        x: *lane_x,
                        y: route_start.y,
                    },
                    DiagramPoint {
                        x: *lane_x,
                        y: *lane_y,
                    },
                    DiagramPoint {
                        x: route_end.x,
                        y: *lane_y,
                    },
                    route_end,
                ],
                &obstacles,
            );
            consider_route(
                &mut best_route,
                vec![
                    route_start,
                    DiagramPoint {
                        x: route_start.x,
                        y: *lane_y,
                    },
                    DiagramPoint {
                        x: *lane_x,
                        y: *lane_y,
                    },
                    DiagramPoint {
                        x: *lane_x,
                        y: route_end.y,
                    },
                    route_end,
                ],
                &obstacles,
            );
        }
    }

    let routed = best_route
        .map(|(_, route)| route)
        .unwrap_or_else(|| compact_route(vec![route_start, route_end]));
    let mut route = Vec::with_capacity(routed.len() + 2);
    route.push(start);
    route.extend(routed);
    route.push(end);
    compact_route(route)
}

fn table_rect(
    model: &DiagramModel,
    layout: &DiagramLayout,
    positions: &BTreeMap<TableRef, DiagramPoint>,
    table_id: &TableRef,
) -> Option<DiagramRect> {
    let position = positions.get(table_id)?;
    let table = model.tables.iter().find(|table| &table.id == table_id)?;
    let node = layout.nodes.get(table_id);
    let width = node.map(|node| node.width).unwrap_or_else(|| table.width());
    let height = node
        .map(|node| node.height)
        .unwrap_or_else(|| table.height());
    Some(DiagramRect {
        left: position.x,
        top: position.y,
        right: position.x + width,
        bottom: position.y + height,
    })
}

fn edge_route_obstacles(
    model: &DiagramModel,
    layout: &DiagramLayout,
    positions: &BTreeMap<TableRef, DiagramPoint>,
    edge: &DiagramEdge,
) -> Vec<DiagramRect> {
    model
        .tables
        .iter()
        .filter(|table| table.id != edge.source && table.id != edge.target)
        .filter_map(|table| table_rect(model, layout, positions, &table.id))
        .map(|rect| rect.expanded(EDGE_CLEARANCE))
        .collect()
}

fn diagram_obstacle_bounds(
    obstacles: &[DiagramRect],
    start: DiagramPoint,
    end: DiagramPoint,
) -> (f32, f32, f32, f32) {
    let mut min_x = start.x.min(end.x);
    let mut min_y = start.y.min(end.y);
    let mut max_x = start.x.max(end.x);
    let mut max_y = start.y.max(end.y);
    for obstacle in obstacles {
        min_x = min_x.min(obstacle.left);
        min_y = min_y.min(obstacle.top);
        max_x = max_x.max(obstacle.right);
        max_y = max_y.max(obstacle.bottom);
    }
    (min_x, min_y, max_x, max_y)
}

fn sort_dedup_lanes(values: &mut Vec<f32>) {
    values.retain(|value| value.is_finite());
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    values.dedup_by(|left, right| (*left - *right).abs() < 1.0);
}

fn consider_route(
    best_route: &mut Option<(f32, Vec<DiagramPoint>)>,
    route: Vec<DiagramPoint>,
    obstacles: &[DiagramRect],
) {
    let route = compact_route(route);
    if !route_is_clear(&route, obstacles) {
        return;
    }
    let score = route_length(&route) + route.len() as f32 * 8.0;
    match best_route {
        Some((best_score, _)) if *best_score <= score => {}
        _ => *best_route = Some((score, route)),
    }
}

fn compact_route(route: Vec<DiagramPoint>) -> Vec<DiagramPoint> {
    let mut compacted = Vec::new();
    for point in route {
        if compacted
            .last()
            .map(|previous: &DiagramPoint| {
                (previous.x - point.x).abs() < 0.5 && (previous.y - point.y).abs() < 0.5
            })
            .unwrap_or(false)
        {
            continue;
        }
        compacted.push(point);
    }

    let mut ix = 1;
    while ix + 1 < compacted.len() {
        let previous = compacted[ix - 1];
        let current = compacted[ix];
        let next = compacted[ix + 1];
        let same_x = (previous.x - current.x).abs() < 0.5 && (current.x - next.x).abs() < 0.5;
        let same_y = (previous.y - current.y).abs() < 0.5 && (current.y - next.y).abs() < 0.5;
        if same_x || same_y {
            compacted.remove(ix);
        } else {
            ix += 1;
        }
    }
    compacted
}

fn route_length(route: &[DiagramPoint]) -> f32 {
    route
        .windows(2)
        .map(|points| (points[1].x - points[0].x).abs() + (points[1].y - points[0].y).abs())
        .sum()
}

fn route_is_clear(route: &[DiagramPoint], obstacles: &[DiagramRect]) -> bool {
    route.windows(2).all(|points| {
        obstacles
            .iter()
            .all(|obstacle| !segment_intersects_rect(points[0], points[1], *obstacle))
    })
}

fn segment_intersects_rect(start: DiagramPoint, end: DiagramPoint, rect: DiagramRect) -> bool {
    let min_x = start.x.min(end.x);
    let max_x = start.x.max(end.x);
    let min_y = start.y.min(end.y);
    let max_y = start.y.max(end.y);
    if (start.y - end.y).abs() < 0.5 {
        start.y > rect.top && start.y < rect.bottom && max_x > rect.left && min_x < rect.right
    } else if (start.x - end.x).abs() < 0.5 {
        start.x > rect.left && start.x < rect.right && max_y > rect.top && min_y < rect.bottom
    } else {
        max_x > rect.left && min_x < rect.right && max_y > rect.top && min_y < rect.bottom
    }
}

fn column_center_y(model: &DiagramModel, table_id: &TableRef, column: Option<&String>) -> f32 {
    let Some(table) = model.tables.iter().find(|table| &table.id == table_id) else {
        return HEADER_HEIGHT + ROW_HEIGHT / 2.0;
    };
    let Some(column) = column else {
        return HEADER_HEIGHT + table.columns.len().max(1) as f32 * ROW_HEIGHT / 2.0;
    };
    let ix = table
        .columns
        .iter()
        .position(|candidate| candidate.name == *column)
        .unwrap_or(0);
    HEADER_HEIGHT + ix as f32 * ROW_HEIGHT + ROW_HEIGHT / 2.0
}

fn ellipsize(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut value = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    value.push_str("...");
    value
}

fn write_diagram_export_file(
    export: &DiagramExport,
    format: DiagramExportFormat,
    path: &str,
) -> anyhow::Result<()> {
    match format {
        DiagramExportFormat::Png => {
            let image = render_diagram_png(export);
            image.save(path)?;
        }
        DiagramExportFormat::Mermaid => {
            std::fs::write(path, render_mermaid(export))?;
        }
    }
    Ok(())
}

fn render_mermaid(export: &DiagramExport) -> String {
    let mut output = String::from("erDiagram\n");
    let mut ids = BTreeMap::new();
    for table in &export.model.tables {
        ids.insert(table.id.clone(), mermaid_entity_id(&table.id));
    }

    for table in &export.model.tables {
        let id = &ids[&table.id];
        let _ = writeln!(output, "    {id} {{");
        for column in &table.columns {
            let mut flags = Vec::new();
            if column.is_pk {
                flags.push("PK");
            }
            if column.is_fk {
                flags.push("FK");
            }
            let flags = if flags.is_empty() {
                String::new()
            } else {
                format!(" {}", flags.join(","))
            };
            let _ = writeln!(
                output,
                "        {} {}{}",
                mermaid_type(&column.data_type),
                mermaid_field(&column.name),
                flags
            );
        }
        let _ = writeln!(output, "    }}");
    }

    for edge in &export.model.edges {
        let Some(source) = ids.get(&edge.source) else {
            continue;
        };
        let Some(target) = ids.get(&edge.target) else {
            continue;
        };
        let label = if edge.source_columns.is_empty() || edge.target_columns.is_empty() {
            "fk".to_string()
        } else {
            format!(
                "{} -> {}",
                edge.source_columns.join(", "),
                edge.target_columns.join(", ")
            )
        };
        let _ = writeln!(
            output,
            "    {source} }}o--|| {target} : \"{}\"",
            label.replace('"', "\\\"")
        );
    }

    output
}

fn mermaid_entity_id(table: &TableRef) -> String {
    sanitize_identifier(&table.label()).to_ascii_uppercase()
}

fn mermaid_type(data_type: &str) -> String {
    let normalized = sanitize_identifier(data_type);
    if normalized.is_empty() {
        "unknown".to_string()
    } else {
        normalized
    }
}

fn mermaid_field(name: &str) -> String {
    let normalized = sanitize_identifier(name);
    if normalized.is_empty() {
        "column".to_string()
    } else {
        normalized
    }
}

fn sanitize_identifier(value: &str) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch);
        } else if !output.ends_with('_') {
            output.push('_');
        }
    }
    output.trim_matches('_').to_string()
}

fn sanitize_file_name(value: &str) -> String {
    let name = sanitize_identifier(value).to_ascii_lowercase();
    if name.is_empty() {
        "diagram".to_string()
    } else {
        name
    }
}

struct DiagramFontCache {
    regular: Arc<Font>,
    bold: Arc<Font>,
}

impl DiagramFontCache {
    fn new() -> Self {
        let font_bytes = load_system_font_bytes();
        let regular = if font_bytes.is_empty() {
            Arc::new(
                Font::from_bytes(&[][..], FontSettings::default()).expect("Failed to load font"),
            )
        } else {
            Arc::new(
                Font::from_bytes(font_bytes.as_slice(), FontSettings::default())
                    .expect("Failed to load font"),
            )
        };
        let bold = Font::from_bytes(
            font_bytes.as_slice(),
            FontSettings {
                collection_index: 1,
                ..Default::default()
            },
        )
        .ok()
        .map(Arc::new)
        .unwrap_or_else(|| regular.clone());
        Self { regular, bold }
    }

    fn get(&self, bold: bool) -> &Arc<Font> {
        if bold { &self.bold } else { &self.regular }
    }
}

fn load_system_font_bytes() -> Vec<u8> {
    #[cfg(target_os = "macos")]
    {
        if let Ok(font_path) = std::process::Command::new("defaults")
            .args(["read", "-g", "AppleFont"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        {
            if !font_path.is_empty() {
                if let Ok(bytes) = std::fs::read(&font_path) {
                    return bytes;
                }
            }
        }
        let candidates = [
            "/System/Library/Fonts/Inter.ttc",
            "/System/Library/Fonts/SFNS.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
        ];
        for path in &candidates {
            if let Ok(bytes) = std::fs::read(path) {
                return bytes;
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        let candidates = [
            "/usr/share/fonts/truetype/inter/Inter-Regular.ttf",
            "/usr/share/fonts/TTF/Inter-Regular.ttf",
            "/usr/share/fonts/dejavu/DejaVuSans.ttf",
        ];
        for path in &candidates {
            if let Ok(bytes) = std::fs::read(path) {
                return bytes;
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        let candidates = [
            "C:\\Windows\\Fonts\\Inter-Regular.ttf",
            "C:\\Windows\\Fonts\\arial.ttf",
        ];
        for path in &candidates {
            if let Ok(bytes) = std::fs::read(path) {
                return bytes;
            }
        }
    }
    Vec::new()
}

fn render_diagram_png(export: &DiagramExport) -> RgbaImage {
    let (min_x, min_y, max_x, max_y) = diagram_export_bounds(export);
    let content_width = (max_x - min_x + EXPORT_MARGIN * 2.0).max(320.0);
    let content_height = (max_y - min_y + EXPORT_MARGIN * 2.0).max(240.0);
    let max_dimension = content_width.max(content_height);
    // Use 2x scale for higher quality output
    let scale = (4800.0 / max_dimension).clamp(1.0, 2.0);
    let image_width = (content_width * scale).ceil() as u32;
    let image_height = (content_height * scale).ceil() as u32;
    let mut image = RgbaImage::from_pixel(image_width, image_height, export.style.background);

    let offset_x = EXPORT_MARGIN - min_x;
    let offset_y = EXPORT_MARGIN - min_y;

    let font_cache = DiagramFontCache::new();

    draw_export_grid(&mut image, scale, export.style.grid);
    draw_export_edges(export, &mut image, scale, offset_x, offset_y);
    for table in &export.model.tables {
        draw_export_table(
            export,
            table,
            &mut image,
            scale,
            offset_x,
            offset_y,
            &font_cache,
        );
    }

    image
}

fn diagram_export_bounds(export: &DiagramExport) -> (f32, f32, f32, f32) {
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = 0.0_f32;
    let mut max_y = 0.0_f32;
    for table in &export.model.tables {
        let Some(position) = export.positions.get(&table.id) else {
            continue;
        };
        let width = export
            .layout
            .nodes
            .get(&table.id)
            .map(|node| node.width)
            .unwrap_or_else(|| table.width());
        let height = export
            .layout
            .nodes
            .get(&table.id)
            .map(|node| node.height)
            .unwrap_or_else(|| table.height());
        min_x = min_x.min(position.x);
        min_y = min_y.min(position.y);
        max_x = max_x.max(position.x + width);
        max_y = max_y.max(position.y + height);
    }
    if min_x == f32::MAX {
        (0.0, 0.0, 320.0, 240.0)
    } else {
        (min_x, min_y, max_x, max_y)
    }
}

fn draw_export_grid(image: &mut RgbaImage, scale: f32, color: Rgba<u8>) {
    let spacing = (24.0 * scale).round().max(8.0) as u32;
    for x in (0..image.width()).step_by(spacing as usize) {
        draw_aa_line(
            image,
            x as f32,
            0.0,
            x as f32,
            image.height() as f32 - 1.0,
            color,
        );
    }
    for y in (0..image.height()).step_by(spacing as usize) {
        draw_aa_line(
            image,
            0.0,
            y as f32,
            image.width() as f32 - 1.0,
            y as f32,
            color,
        );
    }
}

fn draw_export_edges(
    export: &DiagramExport,
    image: &mut RgbaImage,
    scale: f32,
    offset_x: f32,
    offset_y: f32,
) {
    for edge in &export.model.edges {
        let route = route_edge_points(&export.model, &export.layout, &export.positions, edge);
        if route.len() < 2 {
            continue;
        }
        // Use fully opaque edge color for better visibility in export
        let mut color = export.style.edge;
        color[3] = 255;
        // Match GPUI's 1.3px stroke width, scaled up
        let line_width = (1.5 * scale).max(2.0);

        // Draw the route with proper thickness
        for points in route.windows(2) {
            let start_x = (points[0].x + offset_x) * scale;
            let start_y = (points[0].y + offset_y) * scale;
            let end_x = (points[1].x + offset_x) * scale;
            let end_y = (points[1].y + offset_y) * scale;
            draw_aa_thick_line(image, start_x, start_y, end_x, end_y, color, line_width);
        }

        // Draw arrow at the end of the route (pointing to target table)
        if route.len() >= 2 {
            let last = route[route.len() - 1];
            let second_last = route[route.len() - 2];
            let end_x = (last.x + offset_x) * scale;
            let end_y = (last.y + offset_y) * scale;
            let prev_x = (second_last.x + offset_x) * scale;
            let prev_y = (second_last.y + offset_y) * scale;

            // Arrow size proportional to scale
            let arrow_size = (12.0 * scale).clamp(10.0, 24.0);
            draw_arrow(image, prev_x, prev_y, end_x, end_y, color, arrow_size);
        }
    }
}

fn draw_aa_thick_line(
    image: &mut RgbaImage,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    color: Rgba<u8>,
    width: f32,
) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let length = (dx * dx + dy * dy).sqrt();
    if length < 0.5 {
        return;
    }

    let radius = width / 2.0;
    // Use more steps for better coverage
    let steps = (length * 3.0).ceil() as i32;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let cx = x0 + dx * t;
        let cy = y0 + dy * t;

        // Draw a filled circle at this point for thickness
        let min_px = (cx - radius - 1.0).floor() as i32;
        let max_px = (cx + radius + 1.0).ceil() as i32;
        let min_py = (cy - radius - 1.0).floor() as i32;
        let max_py = (cy + radius + 1.0).ceil() as i32;

        for py in min_py..=max_py {
            for px in min_px..=max_px {
                if px < 0 || px >= image.width() as i32 || py < 0 || py >= image.height() as i32 {
                    continue;
                }
                let dist_x = px as f32 + 0.5 - cx;
                let dist_y = py as f32 + 0.5 - cy;
                let dist = (dist_x * dist_x + dist_y * dist_y).sqrt();
                if dist <= radius + 0.5 {
                    // Anti-aliasing at the edge
                    let coverage = if dist > radius - 0.5 {
                        (radius + 0.5 - dist).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    if coverage > 0.0 {
                        let blended_alpha = (color[3] as f32 * coverage) as u8;
                        if blended_alpha > 0 {
                            let blended = Rgba([color[0], color[1], color[2], blended_alpha]);
                            blend_pixel(image, px as u32, py as u32, blended);
                        }
                    }
                }
            }
        }
    }
}

fn draw_arrow(
    image: &mut RgbaImage,
    from_x: f32,
    from_y: f32,
    to_x: f32,
    to_y: f32,
    color: Rgba<u8>,
    size: f32,
) {
    let dx = to_x - from_x;
    let dy = to_y - from_y;
    let length = (dx * dx + dy * dy).sqrt();
    if length < 1.0 {
        return;
    }

    // Direction vector
    let dir_x = dx / length;
    let dir_y = dy / length;

    // Arrow tip (slightly before the actual end to not overlap with table border)
    let tip_x = to_x - dir_x * 2.0;
    let tip_y = to_y - dir_y * 2.0;

    // Arrow base points
    let base_x = tip_x - dir_x * size;
    let base_y = tip_y - dir_y * size;

    // Perpendicular for arrow wings
    let perp_x = -dir_y;
    let perp_y = dir_x;

    let wing_span = size * 0.6;
    let left_x = base_x + perp_x * wing_span;
    let left_y = base_y + perp_y * wing_span;
    let right_x = base_x - perp_x * wing_span;
    let right_y = base_y - perp_y * wing_span;

    // Fill the arrow triangle
    fill_triangle(image, tip_x, tip_y, left_x, left_y, right_x, right_y, color);

    // Draw arrow outline
    draw_aa_thick_line(image, tip_x, tip_y, left_x, left_y, color, 1.0);
    draw_aa_thick_line(image, tip_x, tip_y, right_x, right_y, color, 1.0);
    draw_aa_thick_line(image, left_x, left_y, right_x, right_y, color, 1.0);
}

fn fill_triangle(
    image: &mut RgbaImage,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    color: Rgba<u8>,
) {
    // Sort vertices by y
    let mut points = [(x0, y0), (x1, y1), (x2, y2)];
    points.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let (x0, y0) = points[0];
    let (x1, y1) = points[1];
    let (x2, y2) = points[2];

    let min_y = y0.floor() as i32;
    let max_y = y2.ceil() as i32;

    for y in min_y..=max_y {
        if y < 0 || y >= image.height() as i32 {
            continue;
        }
        let yf = y as f32 + 0.5;

        // Find intersection points with triangle edges
        let mut intersections = Vec::new();

        // Edge 0-1
        if yf >= y0 && yf < y1 {
            let t = (yf - y0) / (y1 - y0);
            intersections.push(x0 + (x1 - x0) * t);
        }
        // Edge 0-2
        if yf >= y0 && yf < y2 {
            let t = (yf - y0) / (y2 - y0);
            intersections.push(x0 + (x2 - x0) * t);
        }
        // Edge 1-2
        if yf >= y1 && yf < y2 {
            let t = (yf - y1) / (y2 - y1);
            intersections.push(x1 + (x2 - x1) * t);
        }

        if intersections.len() >= 2 {
            intersections.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let start_x = intersections[0].ceil() as i32;
            let end_x = intersections[intersections.len() - 1].floor() as i32;

            for x in start_x..=end_x {
                if x >= 0 && x < image.width() as i32 {
                    blend_pixel(image, x as u32, y as u32, color);
                }
            }
        }
    }
}

fn draw_export_table(
    export: &DiagramExport,
    table: &DiagramTable,
    image: &mut RgbaImage,
    scale: f32,
    offset_x: f32,
    offset_y: f32,
    font_cache: &DiagramFontCache,
) {
    let Some(position) = export.positions.get(&table.id) else {
        return;
    };
    let width = export
        .layout
        .nodes
        .get(&table.id)
        .map(|node| node.width)
        .unwrap_or_else(|| table.width());
    let height = export
        .layout
        .nodes
        .get(&table.id)
        .map(|node| node.height)
        .unwrap_or_else(|| table.height());
    let x = ((position.x + offset_x) * scale).round() as i32;
    let y = ((position.y + offset_y) * scale).round() as i32;
    let w = (width * scale).round() as i32;
    let h = (height * scale).round() as i32;
    let header_h = (HEADER_HEIGHT * scale).round() as i32;

    fill_rect(image, x, y, w, h, export.style.card);
    fill_rect(image, x, y, w, header_h, export.style.header);
    stroke_rect(image, x, y, w, h, export.style.border);

    let title = table.id.display_label();
    let title_size = (HEADER_FONT_PX * scale).round().max(10.0);
    let title_width = text_width(font_cache.get(true), &title, title_size);
    draw_text(
        image,
        font_cache.get(true),
        &title,
        (x as f32 + (w as f32 - title_width) / 2.0) as i32,
        y + (8.0 * scale).round() as i32,
        title_size,
        export.style.foreground,
    );

    for (ix, column) in table.columns.iter().enumerate() {
        let row_y = y + ((HEADER_HEIGHT + ix as f32 * ROW_HEIGHT) * scale).round() as i32;
        let row_h = (ROW_HEIGHT * scale).round() as i32;
        let is_first_pk_row = column.is_pk
            && (ix == 0
                || table
                    .columns
                    .get(ix - 1)
                    .map(|previous| !previous.is_pk)
                    .unwrap_or(true));
        let is_last_pk_row = column.is_pk
            && table
                .columns
                .get(ix + 1)
                .map(|next| !next.is_pk)
                .unwrap_or(true);
        if is_first_pk_row && ix > 0 {
            fill_rect(image, x, row_y, w, 2, export.style.border);
        }
        if is_last_pk_row && ix < table.columns.len() - 1 {
            fill_rect(image, x, row_y + row_h, w, 2, export.style.border);
        }

        let text_size = (COLUMN_FONT_PX * scale).round().max(10.0);
        let marker = if column.is_pk {
            "\u{2605}"
        } else if column.is_fk {
            "\u{2192}"
        } else {
            "\u{2022}"
        };
        let marker_color = if column.is_pk {
            export.style.pk_marker
        } else if column.is_fk {
            export.style.fk_marker
        } else {
            export.style.regular_marker
        };
        let text_y = row_y + (5.0 * scale).round() as i32;
        draw_text(
            image,
            font_cache.get(column.is_pk),
            marker,
            x + (8.0 * scale).round() as i32,
            text_y,
            text_size,
            marker_color,
        );

        let name_x = x + ((TABLE_PADDING_X + MARKER_COL_WIDTH) * scale).round() as i32;
        let type_width = (TYPE_COL_WIDTH * scale).round() as i32;
        let type_x = x + ((width - TABLE_PADDING_X - TYPE_COL_WIDTH) * scale).round() as i32;
        let name_width =
            (width - TABLE_PADDING_X * 3.0 - MARKER_COL_WIDTH - TYPE_COL_WIDTH).max(64.0);
        let name_max_chars = (name_width / 6.4).floor().max(6.0) as usize;
        let type_max_chars = (TYPE_COL_WIDTH / 6.1).floor().max(6.0) as usize;
        let name = ellipsize(&column.name, name_max_chars);
        let data_type = ellipsize(&column.data_type, type_max_chars);
        draw_text(
            image,
            font_cache.get(false),
            &name,
            name_x,
            text_y,
            text_size,
            export.style.foreground,
        );
        let type_text_size = (TYPE_FONT_PX * scale).round().max(10.0);
        let type_text_width = text_width(font_cache.get(false), &data_type, type_text_size);
        draw_text(
            image,
            font_cache.get(false),
            &data_type,
            type_x + type_width - type_text_width as i32,
            text_y,
            type_text_size,
            export.style.muted,
        );
    }
}

fn rgba(value: u32) -> Rgba<u8> {
    rgba_alpha(value, 255)
}

fn rgba_alpha(value: u32, alpha: u8) -> Rgba<u8> {
    Rgba([
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
        alpha,
    ])
}

fn rgba_from_hsla(color: Hsla) -> Rgba<u8> {
    let color = color.to_rgb();
    Rgba([
        (color.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.b.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.a.clamp(0.0, 1.0) * 255.0).round() as u8,
    ])
}

fn fill_rect(image: &mut RgbaImage, x: i32, y: i32, width: i32, height: i32, color: Rgba<u8>) {
    let start_x = x.max(0) as u32;
    let start_y = y.max(0) as u32;
    let end_x = (x + width).min(image.width() as i32).max(0) as u32;
    let end_y = (y + height).min(image.height() as i32).max(0) as u32;
    for py in start_y..end_y {
        for px in start_x..end_x {
            blend_pixel(image, px, py, color);
        }
    }
}

fn stroke_rect(image: &mut RgbaImage, x: i32, y: i32, width: i32, height: i32, color: Rgba<u8>) {
    draw_aa_line(
        image,
        x as f32,
        y as f32,
        (x + width - 1) as f32,
        y as f32,
        color,
    );
    draw_aa_line(
        image,
        x as f32,
        (y + height - 1) as f32,
        (x + width - 1) as f32,
        (y + height - 1) as f32,
        color,
    );
    draw_aa_line(
        image,
        x as f32,
        y as f32,
        x as f32,
        (y + height - 1) as f32,
        color,
    );
    draw_aa_line(
        image,
        (x + width - 1) as f32,
        y as f32,
        (x + width - 1) as f32,
        (y + height - 1) as f32,
        color,
    );
}

fn draw_aa_line(image: &mut RgbaImage, x0: f32, y0: f32, x1: f32, y1: f32, color: Rgba<u8>) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let length = (dx * dx + dy * dy).sqrt();
    if length < 0.5 {
        return;
    }
    let steps = (length * 2.0).ceil() as i32;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = x0 + dx * t;
        let y = y0 + dy * t;
        let px = x.floor() as i32;
        let py = y.floor() as i32;
        let fx = x - px as f32;
        let fy = y - py as f32;
        let coverage =
            (1.0 - (fx - 0.5).abs() * 2.0).max(0.0) * (1.0 - (fy - 0.5).abs() * 2.0).max(0.0);
        if coverage > 0.0 {
            let blended_alpha = (color[3] as f32 * coverage) as u8;
            if blended_alpha > 0 {
                let blended = Rgba([color[0], color[1], color[2], blended_alpha]);
                blend_pixel(image, px.max(0) as u32, py.max(0) as u32, blended);
                if px + 1 < image.width() as i32 {
                    blend_pixel(image, (px + 1) as u32, py.max(0) as u32, blended);
                }
                if py + 1 < image.height() as i32 {
                    blend_pixel(image, px.max(0) as u32, (py + 1) as u32, blended);
                }
                if px + 1 < image.width() as i32 && py + 1 < image.height() as i32 {
                    blend_pixel(image, (px + 1) as u32, (py + 1) as u32, blended);
                }
            }
        }
    }
}

fn blend_pixel(image: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
    if color[3] == 255 {
        image.put_pixel(x, y, color);
        return;
    }
    let existing = image.get_pixel_mut(x, y);
    let alpha = color[3] as f32 / 255.0;
    for ix in 0..3 {
        existing[ix] = (color[ix] as f32 * alpha + existing[ix] as f32 * (1.0 - alpha)) as u8;
    }
    existing[3] = 255;
}

fn draw_text(
    image: &mut RgbaImage,
    font: &Font,
    text: &str,
    x: i32,
    y: i32,
    size: f32,
    color: Rgba<u8>,
) {
    let mut cursor_x = x as f32;
    let baseline_y = y as f32 + size;

    for ch in text.chars() {
        let (metrics, bitmap) = font.rasterize(ch, size);
        if bitmap.is_empty() {
            cursor_x += metrics.advance_width;
            continue;
        }

        let glyph_x = cursor_x + metrics.xmin as f32;
        let glyph_y = baseline_y - metrics.height as f32 - metrics.ymin as f32;

        for sy in 0..metrics.height {
            for sx in 0..metrics.width {
                let alpha = bitmap[sy * metrics.width + sx];
                if alpha > 0 {
                    let px = (glyph_x + sx as f32).floor() as i32;
                    let py = (glyph_y + sy as f32).floor() as i32;
                    if px >= 0 && px < image.width() as i32 && py >= 0 && py < image.height() as i32
                    {
                        let blended_alpha = (color[3] as f32 * alpha as f32 / 255.0) as u8;
                        if blended_alpha > 0 {
                            let blended = Rgba([color[0], color[1], color[2], blended_alpha]);
                            blend_pixel(image, px as u32, py as u32, blended);
                        }
                    }
                }
            }
        }

        cursor_x += metrics.advance_width;
    }
}

fn text_width(font: &Font, text: &str, size: f32) -> f32 {
    let mut width = 0.0;
    for ch in text.chars() {
        let (metrics, _) = font.rasterize(ch, size);
        width += metrics.advance_width;
    }
    width
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlab_drivers_core::{
        ColumnInfo, Database, ForeignKeyInfo, SchemaInfo, TableInfo, TableKind,
    };

    fn config() -> DataSourceConfig {
        DataSourceConfig {
            name: "local".into(),
            database: "app".into(),
            ..DataSourceConfig::default()
        }
    }

    fn column(name: &str, is_pk: bool, is_fk: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: "integer".into(),
            nullable: false,
            ordinal: 1,
            is_pk,
            is_fk,
            default_value: None,
            is_generated: false,
            generation_expression: None,
        }
    }

    fn table(schema: &str, name: &str) -> TableInfo {
        TableInfo {
            schema: schema.into(),
            name: name.into(),
            kind: TableKind::Table,
            columns: vec![column("id", true, false), column("parent_id", false, true)],
        }
    }

    fn fk(source_schema: &str, source: &str, target_schema: &str, target: &str) -> ForeignKeyInfo {
        ForeignKeyInfo {
            name: format!("{source}_{target}_fk"),
            source_schema: source_schema.into(),
            source_table: source.into(),
            source_columns: vec!["parent_id".into()],
            target_schema: target_schema.into(),
            target_table: target.into(),
            target_columns: vec!["id".into()],
        }
    }

    fn schema() -> DatabaseSchema {
        DatabaseSchema {
            db_type: Database::Postgres,
            schemas: vec![
                SchemaInfo {
                    name: "public".into(),
                    owner: "postgres".into(),
                },
                SchemaInfo {
                    name: "billing".into(),
                    owner: "postgres".into(),
                },
            ],
            tables: vec![
                table("public", "users"),
                table("public", "orders"),
                table("public", "items"),
                table("billing", "invoices"),
                table("billing", "payments"),
            ],
            foreign_keys: vec![
                fk("public", "orders", "public", "users"),
                fk("public", "items", "public", "orders"),
                fk("billing", "payments", "billing", "invoices"),
            ],
            ..DatabaseSchema::default()
        }
    }

    fn export_for(model: DiagramModel) -> DiagramExport {
        let layout = layout_diagram(&model);
        let positions = layout
            .nodes
            .iter()
            .map(|(id, node)| (id.clone(), node.position))
            .collect();
        DiagramExport {
            model,
            layout,
            positions,
            style: DiagramExportStyle::default(),
        }
    }

    #[test]
    fn schema_scope_limits_tables_to_schema() {
        let model =
            DiagramModel::build(&config(), &schema(), DiagramScope::Schema("public".into()));
        let names = model
            .tables
            .iter()
            .map(|table| table.id.label())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["public.users", "public.orders", "public.items"]);
        assert_eq!(model.edges.len(), 2);
    }

    #[test]
    fn table_scope_recursively_includes_fk_component() {
        let model = DiagramModel::build(
            &config(),
            &schema(),
            DiagramScope::Table {
                schema: "public".into(),
                table: "users".into(),
            },
        );
        let names = model
            .tables
            .iter()
            .map(|table| table.id.label())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            names,
            BTreeSet::from([
                "public.users".to_string(),
                "public.orders".to_string(),
                "public.items".to_string()
            ])
        );
        assert_eq!(model.edges.len(), 2);
    }

    #[test]
    fn large_scopes_are_capped_deterministically() {
        let mut schema = DatabaseSchema::default();
        schema.db_type = Database::Postgres;
        for ix in 0..120 {
            schema.tables.push(table("public", &format!("t_{ix:03}")));
        }
        let model = DiagramModel::build(&config(), &schema, DiagramScope::Database);
        assert!(model.truncated);
        assert_eq!(model.total_tables, 120);
        assert_eq!(model.tables.len(), MAX_DIAGRAM_TABLES);
        assert_eq!(model.tables[0].id.name, "t_000");
        assert_eq!(model.tables[99].id.name, "t_099");
    }

    #[test]
    fn layout_is_stable_and_non_overlapping() {
        let model = DiagramModel::build(&config(), &schema(), DiagramScope::Database);
        let layout_a = layout_diagram(&model);
        let layout_b = layout_diagram(&model);
        assert_eq!(layout_a.nodes.len(), layout_b.nodes.len());
        for table in model.tables {
            let a = layout_a.nodes.get(&table.id).unwrap();
            let b = layout_b.nodes.get(&table.id).unwrap();
            assert_eq!(a.position.x, b.position.x);
            assert_eq!(a.position.y, b.position.y);
        }

        let nodes = layout_a.nodes.iter().collect::<Vec<_>>();
        for (ix, (left_id, left)) in nodes.iter().enumerate() {
            for (right_id, right) in nodes.iter().skip(ix + 1) {
                let overlap = left.position.x < right.position.x + right.width
                    && left.position.x + left.width > right.position.x
                    && left.position.y < right.position.y + right.height
                    && left.position.y + left.height > right.position.y;
                assert!(
                    !overlap,
                    "{} overlaps {}",
                    left_id.label(),
                    right_id.label()
                );
            }
        }
    }

    #[test]
    fn primary_key_columns_are_moved_to_the_bottom() {
        let mut schema = schema();
        schema.tables[0].columns = vec![
            column("id", true, false),
            column("name", false, false),
            column("account_id", false, true),
        ];

        let model = DiagramModel::build(&config(), &schema, DiagramScope::Database);
        let users = model
            .tables
            .iter()
            .find(|table| table.id == TableRef::new("public", "users"))
            .unwrap();
        assert_eq!(
            users
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            vec!["name", "account_id", "id"]
        );
    }

    #[test]
    fn mermaid_export_includes_tables_columns_and_relationships() {
        let model =
            DiagramModel::build(&config(), &schema(), DiagramScope::Schema("public".into()));
        let mermaid = render_mermaid(&export_for(model));

        assert!(mermaid.starts_with("erDiagram\n"));
        assert!(mermaid.contains("PUBLIC_USERS {"));
        assert!(mermaid.contains("integer id PK"));
        assert!(mermaid.contains("integer parent_id FK"));
        assert!(mermaid.contains("PUBLIC_ORDERS }o--|| PUBLIC_USERS"));
        assert!(mermaid.contains("\"parent_id -> id\""));
    }

    #[test]
    fn png_export_renders_non_empty_image() {
        let model =
            DiagramModel::build(&config(), &schema(), DiagramScope::Schema("public".into()));
        let image = render_diagram_png(&export_for(model));

        assert!(image.width() > 320);
        assert!(image.height() > 200);
        assert!(
            image
                .pixels()
                .any(|pixel| *pixel != DiagramExportStyle::default().background)
        );
    }

    #[test]
    fn routed_edges_do_not_cross_intermediate_tables() {
        let model = DiagramModel {
            title: "test diagram".into(),
            scope: DiagramScope::Database,
            tables: vec![
                DiagramTable {
                    id: TableRef::new("public", "orders"),
                    columns: vec![DiagramColumn {
                        name: "customer_id".into(),
                        data_type: "uuid".into(),
                        nullable: false,
                        is_pk: false,
                        is_fk: true,
                    }],
                },
                DiagramTable {
                    id: TableRef::new("public", "customers"),
                    columns: vec![DiagramColumn {
                        name: "id".into(),
                        data_type: "uuid".into(),
                        nullable: false,
                        is_pk: true,
                        is_fk: false,
                    }],
                },
                DiagramTable {
                    id: TableRef::new("public", "audit_log"),
                    columns: vec![DiagramColumn {
                        name: "id".into(),
                        data_type: "uuid".into(),
                        nullable: false,
                        is_pk: true,
                        is_fk: false,
                    }],
                },
            ],
            edges: vec![DiagramEdge {
                source: TableRef::new("public", "orders"),
                source_columns: vec!["customer_id".into()],
                target: TableRef::new("public", "customers"),
                target_columns: vec!["id".into()],
            }],
            total_tables: 3,
            truncated: false,
            config: DataSourceConfig::default(),
            available_schemas: vec!["public".to_string()],
        };
        let orders = TableRef::new("public", "orders");
        let customers = TableRef::new("public", "customers");
        let audit_log = TableRef::new("public", "audit_log");
        let mut layout = DiagramLayout::default();
        layout.nodes.insert(
            orders.clone(),
            DiagramNodeLayout {
                position: DiagramPoint { x: 0.0, y: 0.0 },
                width: 300.0,
                height: 52.0,
            },
        );
        layout.nodes.insert(
            audit_log.clone(),
            DiagramNodeLayout {
                position: DiagramPoint { x: 360.0, y: 0.0 },
                width: 300.0,
                height: 52.0,
            },
        );
        layout.nodes.insert(
            customers.clone(),
            DiagramNodeLayout {
                position: DiagramPoint { x: 720.0, y: 0.0 },
                width: 300.0,
                height: 52.0,
            },
        );
        let positions = layout
            .nodes
            .iter()
            .map(|(id, node)| (id.clone(), node.position))
            .collect::<BTreeMap<_, _>>();
        let route = route_edge_points(&model, &layout, &positions, &model.edges[0]);
        let obstacles = edge_route_obstacles(&model, &layout, &positions, &model.edges[0]);

        assert!(route.len() > 2);
        assert!(route_is_clear(&route, &obstacles));
    }
}
