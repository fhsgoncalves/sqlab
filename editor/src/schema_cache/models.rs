use sqlab_drivers_core::{
    ColumnInfo, Database, DatabaseSchema, ForeignKeyInfo, FunctionInfo, IndexInfo, SchemaInfo,
    SequenceInfo, TableInfo, TableKind, TriggerInfo, display_data_type,
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
    Vec<ForeignKeyRow>,
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
                enum_values: serde_json::to_string(&c.enum_values).unwrap_or_default(),
                nullable: if c.nullable { 1 } else { 0 },
                ordinal: c.ordinal,
                is_pk: if c.is_pk { 1 } else { 0 },
                is_fk: if c.is_fk { 1 } else { 0 },
                default_value: c.default_value.clone(),
                is_generated: if c.is_generated { 1 } else { 0 },
                generation_expression: c.generation_expression.clone(),
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
            arguments: f.arguments.clone(),
            return_type: Some(f.return_type.clone()),
            definition: f.definition.clone(),
            language: Some(f.language.clone()),
            body: f.body.clone(),
            library: f.library.clone(),
            owner: Some(f.owner.clone()),
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

    let foreign_keys: Vec<ForeignKeyRow> = schema
        .foreign_keys
        .iter()
        .map(|fk| ForeignKeyRow {
            connection_key: connection_key.to_string(),
            name: fk.name.clone(),
            source_schema: fk.source_schema.clone(),
            source_table: fk.source_table.clone(),
            source_columns: serde_json::to_string(&fk.source_columns).unwrap_or_default(),
            target_schema: fk.target_schema.clone(),
            target_table: fk.target_table.clone(),
            target_columns: serde_json::to_string(&fk.target_columns).unwrap_or_default(),
        })
        .collect();

    (
        schemas,
        tables,
        columns,
        functions,
        sequences,
        indexes,
        triggers,
        foreign_keys,
    )
}

pub fn rows_to_schema(
    db_type: Database,
    schemas: Vec<SchemaRow>,
    tables: Vec<TableRow>,
    columns: Vec<ColumnRow>,
    functions: Vec<FunctionRow>,
    sequences: Vec<SequenceRow>,
    indexes: Vec<IndexRow>,
    triggers: Vec<TriggerRow>,
    foreign_keys: Vec<ForeignKeyRow>,
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
        if let Some(table) = table_infos
            .iter_mut()
            .find(|t| t.schema == c.schema_name && t.name == c.table_name)
        {
            table.columns.push(ColumnInfo {
                name: c.name,
                data_type: display_data_type(db_type, c.data_type),
                enum_values: serde_json::from_str(&c.enum_values).unwrap_or_default(),
                nullable: c.nullable != 0,
                ordinal: c.ordinal,
                is_pk: c.is_pk != 0,
                is_fk: c.is_fk != 0,
                default_value: c.default_value,
                is_generated: c.is_generated != 0,
                generation_expression: c.generation_expression,
            });
        }
    }

    let function_infos: Vec<FunctionInfo> = functions
        .into_iter()
        .map(|f| FunctionInfo {
            schema: f.schema_name,
            name: f.name,
            arguments: f.arguments,
            return_type: display_data_type(db_type, f.return_type.unwrap_or_default()),
            definition: f.definition,
            language: f.language.unwrap_or_else(|| "unknown".to_string()),
            body: f.body,
            library: f.library,
            owner: f.owner.unwrap_or_default(),
        })
        .collect();

    let sequence_infos: Vec<SequenceInfo> = sequences
        .into_iter()
        .map(|s| SequenceInfo {
            schema: s.schema_name,
            name: s.name,
            data_type: display_data_type(db_type, s.data_type.unwrap_or_default()),
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

    let foreign_key_infos: Vec<ForeignKeyInfo> = foreign_keys
        .into_iter()
        .map(|fk| ForeignKeyInfo {
            name: fk.name,
            source_schema: fk.source_schema,
            source_table: fk.source_table,
            source_columns: serde_json::from_str(&fk.source_columns).unwrap_or_default(),
            target_schema: fk.target_schema,
            target_table: fk.target_table,
            target_columns: serde_json::from_str(&fk.target_columns).unwrap_or_default(),
        })
        .collect();

    DatabaseSchema {
        db_type,
        schemas: schema_infos,
        tables: table_infos,
        functions: function_infos,
        sequences: sequence_infos,
        indexes: index_infos,
        triggers: trigger_infos,
        foreign_keys: foreign_key_infos,
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
    pub enum_values: String,
    pub nullable: i32,
    pub ordinal: i32,
    pub is_pk: i32,
    pub is_fk: i32,
    pub default_value: Option<String>,
    pub is_generated: i32,
    pub generation_expression: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FunctionRow {
    pub connection_key: String,
    pub schema_name: String,
    pub name: String,
    pub arguments: String,
    pub return_type: Option<String>,
    pub definition: Option<String>,
    pub language: Option<String>,
    pub body: Option<String>,
    pub library: Option<String>,
    pub owner: Option<String>,
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

#[derive(Debug, Clone)]
pub struct ForeignKeyRow {
    pub connection_key: String,
    pub name: String,
    pub source_schema: String,
    pub source_table: String,
    pub source_columns: String,
    pub target_schema: String,
    pub target_table: String,
    pub target_columns: String,
}
