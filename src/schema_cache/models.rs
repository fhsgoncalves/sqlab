use crate::data_source::{
    ColumnInfo, DatabaseSchema, FunctionInfo, IndexInfo, SchemaInfo, SequenceInfo, TableInfo,
    TableKind, TriggerInfo,
};

pub fn schema_to_rows(
    connection_key: &str,
    schema: &DatabaseSchema,
) -> (
    Vec<SchemaRow>,
    Vec<TableRow>,
    Vec<ColumnRow>,
    Vec<FunctionRow>,
    Vec<SequenceRow>,
    Vec<IndexRow>,
    Vec<TriggerRow>,
) {
    let schemas: Vec<SchemaRow> = schema
        .schemas
        .iter()
        .map(|s| SchemaRow {
            connection_key: connection_key.to_string(),
            name: s.name.clone(),
            owner: s.owner.clone(),
        })
        .collect();

    let mut tables = Vec::new();
    let mut columns = Vec::new();

    for t in &schema.tables {
        tables.push(TableRow {
            connection_key: connection_key.to_string(),
            schema_name: t.schema.clone(),
            name: t.name.clone(),
            kind: match t.kind {
                TableKind::Table => "table",
                TableKind::View => "view",
                TableKind::MaterializedView => "materialized_view",
                TableKind::ForeignTable => "foreign_table",
            }
            .to_string(),
        });

        for c in &t.columns {
            columns.push(ColumnRow {
                connection_key: connection_key.to_string(),
                schema_name: t.schema.clone(),
                table_name: t.name.clone(),
                name: c.name.clone(),
                data_type: c.data_type.clone(),
                nullable: if c.nullable { 1 } else { 0 },
                ordinal: c.ordinal,
                is_pk: if c.is_pk { 1 } else { 0 },
                is_fk: if c.is_fk { 1 } else { 0 },
            });
        }
    }

    let functions: Vec<FunctionRow> = schema
        .functions
        .iter()
        .map(|f| FunctionRow {
            connection_key: connection_key.to_string(),
            schema_name: f.schema.clone(),
            name: f.name.clone(),
            arguments: Some(f.arguments.clone()),
            return_type: Some(f.return_type.clone()),
        })
        .collect();

    let sequences: Vec<SequenceRow> = schema
        .sequences
        .iter()
        .map(|s| SequenceRow {
            connection_key: connection_key.to_string(),
            schema_name: s.schema.clone(),
            name: s.name.clone(),
            data_type: Some(s.data_type.clone()),
            start_value: Some(s.start_value.clone()),
            min_value: Some(s.min_value.clone()),
            max_value: Some(s.max_value.clone()),
            increment_by: Some(s.increment_by.clone()),
        })
        .collect();

    let indexes: Vec<IndexRow> = schema
        .indexes
        .iter()
        .map(|i| IndexRow {
            connection_key: connection_key.to_string(),
            schema_name: i.schema.clone(),
            table_name: i.table_name.clone(),
            name: i.name.clone(),
            is_unique: if i.is_unique { 1 } else { 0 },
            is_primary: if i.is_primary { 1 } else { 0 },
            columns: serde_json::to_string(&i.columns).unwrap_or_default(),
        })
        .collect();

    let triggers: Vec<TriggerRow> = schema
        .triggers
        .iter()
        .map(|t| TriggerRow {
            connection_key: connection_key.to_string(),
            schema_name: t.schema.clone(),
            table_name: t.table_name.clone(),
            name: t.name.clone(),
            event: t.event.clone(),
            timing: t.timing.clone(),
            definition: t.definition.clone(),
        })
        .collect();

    (schemas, tables, columns, functions, sequences, indexes, triggers)
}

pub fn rows_to_schema(
    schemas: Vec<SchemaRow>,
    tables: Vec<TableRow>,
    columns: Vec<ColumnRow>,
    functions: Vec<FunctionRow>,
    sequences: Vec<SequenceRow>,
    indexes: Vec<IndexRow>,
    triggers: Vec<TriggerRow>,
) -> DatabaseSchema {
    let schema_infos: Vec<SchemaInfo> = schemas
        .into_iter()
        .map(|s| SchemaInfo {
            name: s.name,
            owner: s.owner,
        })
        .collect();

    let mut table_infos: Vec<TableInfo> = Vec::new();
    for t in tables {
        let kind = match t.kind.as_str() {
            "view" => TableKind::View,
            "materialized_view" => TableKind::MaterializedView,
            "foreign_table" => TableKind::ForeignTable,
            _ => TableKind::Table,
        };
        table_infos.push(TableInfo {
            schema: t.schema_name,
            name: t.name,
            kind,
            columns: Vec::new(),
        });
    }

    for c in columns {
        if let Some(table) = table_infos.iter_mut().find(|t| {
            t.schema == c.schema_name && t.name == c.table_name
        }) {
            table.columns.push(ColumnInfo {
                name: c.name,
                data_type: c.data_type,
                nullable: c.nullable != 0,
                ordinal: c.ordinal,
                is_pk: c.is_pk != 0,
                is_fk: c.is_fk != 0,
            });
        }
    }

    let function_infos: Vec<FunctionInfo> = functions
        .into_iter()
        .map(|f| FunctionInfo {
            schema: f.schema_name,
            name: f.name,
            arguments: f.arguments.unwrap_or_default(),
            return_type: f.return_type.unwrap_or_default(),
        })
        .collect();

    let sequence_infos: Vec<SequenceInfo> = sequences
        .into_iter()
        .map(|s| SequenceInfo {
            schema: s.schema_name,
            name: s.name,
            data_type: s.data_type.unwrap_or_default(),
            start_value: s.start_value.unwrap_or_default(),
            min_value: s.min_value.unwrap_or_default(),
            max_value: s.max_value.unwrap_or_default(),
            increment_by: s.increment_by.unwrap_or_default(),
        })
        .collect();

    let index_infos: Vec<IndexInfo> = indexes
        .into_iter()
        .map(|i| IndexInfo {
            schema: i.schema_name,
            table_name: i.table_name,
            name: i.name,
            is_unique: i.is_unique != 0,
            is_primary: i.is_primary != 0,
            columns: serde_json::from_str(&i.columns).unwrap_or_default(),
        })
        .collect();

    let trigger_infos: Vec<TriggerInfo> = triggers
        .into_iter()
        .map(|t| TriggerInfo {
            schema: t.schema_name,
            table_name: t.table_name,
            name: t.name,
            event: t.event,
            timing: t.timing,
            definition: t.definition,
        })
        .collect();

    DatabaseSchema {
        schemas: schema_infos,
        tables: table_infos,
        functions: function_infos,
        sequences: sequence_infos,
        indexes: index_infos,
        triggers: trigger_infos,
    }
}

#[derive(Debug, Clone)]
pub struct SchemaRow {
    pub connection_key: String,
    pub name: String,
    pub owner: String,
}

#[derive(Debug, Clone)]
pub struct TableRow {
    pub connection_key: String,
    pub schema_name: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct ColumnRow {
    pub connection_key: String,
    pub schema_name: String,
    pub table_name: String,
    pub name: String,
    pub data_type: String,
    pub nullable: i32,
    pub ordinal: i32,
    pub is_pk: i32,
    pub is_fk: i32,
}

#[derive(Debug, Clone)]
pub struct FunctionRow {
    pub connection_key: String,
    pub schema_name: String,
    pub name: String,
    pub arguments: Option<String>,
    pub return_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SequenceRow {
    pub connection_key: String,
    pub schema_name: String,
    pub name: String,
    pub data_type: Option<String>,
    pub start_value: Option<String>,
    pub min_value: Option<String>,
    pub max_value: Option<String>,
    pub increment_by: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IndexRow {
    pub connection_key: String,
    pub schema_name: String,
    pub table_name: String,
    pub name: String,
    pub is_unique: i32,
    pub is_primary: i32,
    pub columns: String,
}

#[derive(Debug, Clone)]
pub struct TriggerRow {
    pub connection_key: String,
    pub schema_name: String,
    pub table_name: String,
    pub name: String,
    pub event: String,
    pub timing: String,
    pub definition: String,
}
