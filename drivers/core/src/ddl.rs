use crate::{
    ColumnInfo, Database, DatabaseSchema, FunctionInfo, IndexInfo, SchemaInfo, SequenceInfo,
    TableInfo, TableKind, TriggerInfo,
};

/// Trait for generating DDL statements for database objects.
pub trait DdlGenerator {
    fn generate_schema_ddl(&self, schema: &SchemaInfo) -> String;
    fn generate_table_ddl(&self, schema: &DatabaseSchema, table: &TableInfo) -> String;
    fn generate_view_ddl(&self, schema: &DatabaseSchema, table: &TableInfo) -> String;
    fn generate_function_ddl(&self, func: &FunctionInfo) -> String;
    fn generate_index_ddl(&self, idx: &IndexInfo) -> String;
    fn generate_trigger_ddl(&self, trig: &TriggerInfo) -> String;
    fn generate_sequence_ddl(&self, seq: &SequenceInfo) -> String;
    fn generate_column_ddl(&self, table: &TableInfo, column: &ColumnInfo) -> String;
}

/// PostgreSQL-specific DDL generator.
pub struct PostgresDdlGenerator;
pub struct MySqlDdlGenerator;
pub struct SQLiteDdlGenerator;
pub struct DuckDbDdlGenerator;
pub struct DatabendDdlGenerator;

impl DdlGenerator for PostgresDdlGenerator {
    fn generate_schema_ddl(&self, schema: &SchemaInfo) -> String {
        let mut ddl = String::new();
        ddl.push_str(&format!(
            "CREATE SCHEMA {};\n",
            quote_identifier(&schema.name)
        ));
        if !schema.owner.is_empty() && schema.owner != "postgres" {
            ddl.push_str(&format!(
                "ALTER SCHEMA {} OWNER TO {};\n",
                quote_identifier(&schema.name),
                quote_identifier(&schema.owner)
            ));
        }
        ddl
    }

    fn generate_table_ddl(&self, schema: &DatabaseSchema, table: &TableInfo) -> String {
        let mut ddl = String::new();
        let tbl_qualified_name = qualified_name(&table.schema, &table.name);

        ddl.push_str(&format!("CREATE TABLE {} (\n", tbl_qualified_name));

        // Columns
        let columns_ddl: Vec<String> = table
            .columns
            .iter()
            .map(|col| generate_column_definition(col, schema, table))
            .collect();
        ddl.push_str(&columns_ddl.join(",\n"));

        // Primary key constraint (multi-column)
        let pk_columns: Vec<&ColumnInfo> = table.columns.iter().filter(|c| c.is_pk).collect();
        if pk_columns.len() > 1 {
            let pk_col_list: Vec<String> = pk_columns
                .iter()
                .map(|c| quote_identifier(&c.name))
                .collect();
            ddl.push_str(&format!(
                ",\n    CONSTRAINT {}_pkey PRIMARY KEY ({})",
                quote_identifier(&table.name),
                pk_col_list.join(", ")
            ));
        }

        // Foreign key constraints
        let fk_constraints: Vec<String> = schema
            .foreign_keys
            .iter()
            .filter(|fk| fk.source_schema == table.schema && fk.source_table == table.name)
            .filter_map(|fk| {
                if fk.source_columns.is_empty() {
                    return None;
                }
                Some(format!(
                    "    CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
                    quote_identifier(&fk.name),
                    fk.source_columns
                        .iter()
                        .map(|c| quote_identifier(c))
                        .collect::<Vec<_>>()
                        .join(", "),
                    qualified_name(&fk.target_schema, &fk.target_table),
                    fk.target_columns
                        .iter()
                        .map(|c| quote_identifier(c))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })
            .collect();

        if !fk_constraints.is_empty() {
            ddl.push_str(",\n");
            ddl.push_str(&fk_constraints.join(",\n"));
        }

        ddl.push_str("\n);\n");

        // Non-primary-key indexes
        for idx in &schema.indexes {
            if idx.schema == table.schema && idx.table_name == table.name && !idx.is_primary {
                ddl.push('\n');
                ddl.push_str(&self.generate_index_ddl(idx));
            }
        }

        // Table comment
        if let Some(comment) = &table.comment {
            ddl.push_str(&format!(
                "\nCOMMENT ON TABLE {} IS {};\n",
                tbl_qualified_name,
                quote_string(comment)
            ));
        }

        // Column comments
        for col in &table.columns {
            if let Some(comment) = &col.comment {
                ddl.push_str(&format!(
                    "COMMENT ON COLUMN {} IS {};\n",
                    qualified_name(&table.schema, &format!("{}.{}", table.name, col.name)),
                    quote_string(comment)
                ));
            }
        }

        // Add note for foreign tables
        if matches!(table.kind, TableKind::ForeignTable) {
            ddl.push_str(&format!(
                "-- Note: {} is a foreign table\n",
                tbl_qualified_name
            ));
        }

        ddl
    }

    fn generate_view_ddl(&self, _schema: &DatabaseSchema, table: &TableInfo) -> String {
        let mut ddl = String::new();
        let view_qualified_name = qualified_name(&table.schema, &table.name);

        match table.kind {
            TableKind::MaterializedView => {
                ddl.push_str(&format!("-- Materialized View: {}\n", view_qualified_name));
                ddl.push_str(&format!(
                    "-- To recreate: CREATE MATERIALIZED VIEW {} AS\n",
                    view_qualified_name
                ));
                ddl.push_str("-- SELECT ... -- (view definition not available in schema cache)\n");
            }
            TableKind::View => {
                ddl.push_str(&format!("-- View: {}\n", view_qualified_name));
                ddl.push_str(&format!(
                    "-- To recreate: CREATE VIEW {} AS\n",
                    view_qualified_name
                ));
                ddl.push_str("-- SELECT ... -- (view definition not available in schema cache)\n");
            }
            _ => {}
        }

        // Include column definitions
        if !table.columns.is_empty() {
            ddl.push_str("\n-- Columns:\n");
            for col in &table.columns {
                ddl.push_str(&format!(
                    "--   {} : {}\n",
                    col.name,
                    format_column_type(col)
                ));
            }
        }

        ddl.push('\n');
        ddl
    }

    fn generate_function_ddl(&self, func: &FunctionInfo) -> String {
        let mut ddl = String::new();
        let func_qualified_name = qualified_name(&func.schema, &func.name);

        if let Some(ref definition) = func.definition {
            // pg_get_functiondef returns the full CREATE FUNCTION statement
            let cleaned = definition.replace("$function$", "$$");
            if cleaned.ends_with(';') {
                ddl.push_str(&format!("{}\n", cleaned));
            } else {
                ddl.push_str(&format!("{};\n", cleaned));
            }
        } else if let Some(ref body) = func.body {
            // Reconstruct CREATE FUNCTION from components
            ddl.push_str(&format!(
                "CREATE OR REPLACE FUNCTION {}(",
                func_qualified_name
            ));

            // Arguments
            if !func.arguments.is_empty() {
                ddl.push_str(&func.arguments);
            }
            ddl.push_str(")\n");

            // Return type
            ddl.push_str(&format!("RETURNS {}\n", func.return_type));
            ddl.push_str(&format!("LANGUAGE {}\n", func.language));

            // Function body based on language
            match func.language.as_str() {
                "c" => {
                    // C functions need a shared library path
                    if let Some(ref library) = func.library {
                        ddl.push_str(&format!("AS '{}', '{}';\n", library, body));
                    } else {
                        ddl.push_str(&format!("AS 'object_file', '{}';\n", body));
                        ddl.push_str("-- Note: Library path not available. Update object_file.\n");
                    }
                }
                "internal" => {
                    // Internal functions use prosrc as the internal function name
                    ddl.push_str(&format!("AS '{}';\n", body));
                }
                "plpgsql" | "plperl" | "plpython3u" | "pltcl" | "sql" => {
                    ddl.push_str("AS $$\n");
                    ddl.push_str(body);
                    if !body.ends_with('\n') {
                        ddl.push('\n');
                    }
                    ddl.push_str("$$;\n");
                }
                _ => {
                    ddl.push_str("AS $$\n");
                    ddl.push_str(body);
                    if !body.ends_with('\n') {
                        ddl.push('\n');
                    }
                    ddl.push_str("$$;\n");
                }
            }
        } else {
            // Fallback: generate a skeleton
            ddl.push_str(&format!(
                "CREATE OR REPLACE FUNCTION {}(",
                func_qualified_name
            ));

            // Arguments
            if !func.arguments.is_empty() {
                ddl.push_str(&func.arguments);
            }
            ddl.push_str(")\n");

            // Return type
            ddl.push_str(&format!("RETURNS {}\n", func.return_type));
            ddl.push_str(&format!("LANGUAGE {}\n", func.language));
            ddl.push_str("AS $$\n");
            ddl.push_str("BEGIN\n");
            ddl.push_str("    -- Function body not available\n");
            ddl.push_str("END;\n");
            ddl.push_str("$$;\n");
        }

        // Add owner command if available
        if !func.owner.is_empty() {
            ddl.push_str(&format!(
                "\nALTER FUNCTION {} OWNER TO {};\n",
                func_qualified_name,
                quote_identifier(&func.owner)
            ));
        }

        ddl
    }

    fn generate_index_ddl(&self, idx: &IndexInfo) -> String {
        let mut ddl = String::new();
        let idx_qualified_name = qualified_name(&idx.schema, &idx.name);
        let table_name = qualified_name(&idx.schema, &idx.table_name);

        let unique = if idx.is_unique { "UNIQUE " } else { "" };

        ddl.push_str(&format!(
            "CREATE {}INDEX {} ON {} (",
            unique, idx_qualified_name, table_name
        ));

        let columns: Vec<String> = idx.columns.iter().map(|c| quote_identifier(c)).collect();
        ddl.push_str(&columns.join(", "));
        ddl.push_str(");\n");

        ddl
    }

    fn generate_trigger_ddl(&self, trig: &TriggerInfo) -> String {
        // TriggerInfo already contains the full definition from pg_get_triggerdef
        if !trig.definition.is_empty() {
            format!("{};\n", trig.definition)
        } else {
            // Fallback: generate from components
            let mut ddl = String::new();
            let _trig_qualified_name = qualified_name(&trig.schema, &trig.name);
            let table_name = qualified_name(&trig.schema, &trig.table_name);

            ddl.push_str(&format!(
                "CREATE TRIGGER {}\n",
                quote_identifier(&trig.name)
            ));
            ddl.push_str(&format!("    {} {}\n", trig.timing, trig.event));
            ddl.push_str(&format!("    ON {}\n", table_name));
            ddl.push_str("    FOR EACH ROW\n");
            ddl.push_str("    EXECUTE FUNCTION ...; -- function not available in schema cache\n");

            ddl
        }
    }

    fn generate_sequence_ddl(&self, seq: &SequenceInfo) -> String {
        let mut ddl = String::new();
        let seq_qualified_name = qualified_name(&seq.schema, &seq.name);

        ddl.push_str(&format!("CREATE SEQUENCE {}\n", seq_qualified_name));

        if seq.start_value != "1" && !seq.start_value.is_empty() {
            ddl.push_str(&format!("    START WITH {}\n", seq.start_value));
        }
        if seq.increment_by != "1" && !seq.increment_by.is_empty() {
            ddl.push_str(&format!("    INCREMENT BY {}\n", seq.increment_by));
        }
        if seq.min_value != "1" && !seq.min_value.is_empty() && seq.min_value != "0" {
            ddl.push_str(&format!("    MINVALUE {}\n", seq.min_value));
        }
        if !seq.max_value.is_empty() && seq.max_value != "0" {
            ddl.push_str(&format!("    MAXVALUE {}\n", seq.max_value));
        }
        if !seq.data_type.is_empty() && seq.data_type != "bigint" {
            ddl.push_str(&format!("    AS {}\n", seq.data_type));
        }

        ddl.push_str(";\n");

        ddl
    }

    fn generate_column_ddl(&self, table: &TableInfo, column: &ColumnInfo) -> String {
        let col_qualified_name = qualified_name(&table.schema, &table.name);
        format!(
            "ALTER TABLE {} ADD COLUMN {} {};\n",
            col_qualified_name,
            quote_identifier(&column.name),
            format_column_type(column)
        )
    }
}

fn generate_column_definition(
    col: &ColumnInfo,
    _schema: &DatabaseSchema,
    table: &TableInfo,
) -> String {
    let mut def = format!("    {}", quote_identifier(&col.name));
    def.push_str(&format!(" {}", format_column_type(col)));

    // NOT NULL
    if !col.nullable {
        def.push_str(" NOT NULL");
    }

    // Default value (if available)
    if let Some(default) = &col.default_value {
        def.push_str(&format!(" DEFAULT {}", default));
    }

    // Primary key (inline for single-column PKs only)
    if col.is_pk {
        let pk_columns: Vec<&ColumnInfo> = table.columns.iter().filter(|c| c.is_pk).collect();
        if pk_columns.len() == 1 {
            def.push_str(" PRIMARY KEY");
        }
    }

    def
}

fn format_column_type(col: &ColumnInfo) -> String {
    let mut type_str = col.data_type.clone();

    // Handle generated columns
    if col.is_generated
        && let Some(gen_expr) = &col.generation_expression
    {
        type_str = format!("GENERATED ALWAYS AS ({}) STORED", gen_expr);
    }

    type_str
}

fn quote_identifier(name: &str) -> String {
    // Quote if it's a reserved word or contains special characters
    if needs_quoting(name) {
        format!("\"{}\"", name.replace('"', "\"\""))
    } else {
        name.to_string()
    }
}

fn needs_quoting(name: &str) -> bool {
    // Quote if starts with digit, contains special chars, or is a reserved word
    if name.is_empty() {
        return true;
    }

    let Some(first_char) = name.chars().next() else {
        return true;
    };
    if first_char.is_ascii_digit() || !first_char.is_alphanumeric() && first_char != '_' {
        return true;
    }

    if name.contains(|c: char| !c.is_alphanumeric() && c != '_') {
        return true;
    }

    // Check for reserved words (common SQL reserved words)
    let reserved_words = [
        "select",
        "from",
        "where",
        "insert",
        "update",
        "delete",
        "create",
        "drop",
        "alter",
        "table",
        "index",
        "view",
        "schema",
        "function",
        "trigger",
        "sequence",
        "primary",
        "key",
        "foreign",
        "references",
        "constraint",
        "default",
        "null",
        "not",
        "and",
        "or",
        "in",
        "is",
        "like",
        "between",
        "exists",
        "case",
        "when",
        "then",
        "else",
        "end",
        "as",
        "on",
        "join",
        "inner",
        "outer",
        "left",
        "right",
        "full",
        "cross",
        "natural",
        "group",
        "by",
        "order",
        "having",
        "limit",
        "offset",
        "union",
        "all",
        "distinct",
        "into",
        "values",
        "set",
        "returning",
        "with",
        "recursive",
        "grant",
        "revoke",
        "public",
        "owner",
        "to",
        "user",
        "role",
        "authorization",
        "begin",
        "commit",
        "rollback",
        "transaction",
        "work",
        "language",
        "plpgsql",
        "sql",
        "returns",
        "declare",
        "execute",
        "perform",
        "raise",
        "new",
        "old",
        "for",
        "each",
        "row",
        "statement",
        "before",
        "after",
        "instead",
        "of",
    ];

    reserved_words.contains(&name.to_lowercase().as_str())
}

fn qualified_name(schema: &str, name: &str) -> String {
    format!("{}.{}", quote_identifier(schema), quote_identifier(name))
}

fn quote_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

impl DdlGenerator for MySqlDdlGenerator {
    fn generate_schema_ddl(&self, schema: &SchemaInfo) -> String {
        format!(
            "CREATE DATABASE {};\n",
            quote_mysql_identifier(&schema.name)
        )
    }

    fn generate_table_ddl(&self, schema: &DatabaseSchema, table: &TableInfo) -> String {
        let mut ddl =
            generic_table_ddl(schema, table, quote_mysql_identifier, mysql_qualified_name);

        // Non-primary-key indexes
        for idx in &schema.indexes {
            if idx.schema == table.schema && idx.table_name == table.name && !idx.is_primary {
                ddl.push('\n');
                ddl.push_str(&self.generate_index_ddl(idx));
            }
        }

        // Table comment
        if let Some(comment) = &table.comment {
            if !comment.is_empty() {
                ddl.push_str(&format!(
                    "\nALTER TABLE {} COMMENT = {};\n",
                    mysql_qualified_name(&table.schema, &table.name),
                    quote_string(comment)
                ));
            }
        }

        // Column comments
        for col in &table.columns {
            if let Some(comment) = &col.comment {
                if !comment.is_empty() {
                    ddl.push_str(&format!(
                        "ALTER TABLE {} MODIFY COLUMN {} {} COMMENT {};\n",
                        mysql_qualified_name(&table.schema, &table.name),
                        quote_mysql_identifier(&col.name),
                        format_column_type(col),
                        quote_string(comment)
                    ));
                }
            }
        }

        ddl
    }

    fn generate_view_ddl(&self, _schema: &DatabaseSchema, table: &TableInfo) -> String {
        format!(
            "-- View: {}\n-- View definition is not available in schema cache.\n",
            mysql_qualified_name(&table.schema, &table.name)
        )
    }

    fn generate_function_ddl(&self, func: &FunctionInfo) -> String {
        func.definition
            .clone()
            .unwrap_or_else(|| format!("-- Routine definition unavailable: {}\n", func.name))
    }

    fn generate_index_ddl(&self, idx: &IndexInfo) -> String {
        generic_index_ddl(idx, quote_mysql_identifier, mysql_qualified_name)
    }

    fn generate_trigger_ddl(&self, trig: &TriggerInfo) -> String {
        if trig.definition.is_empty() {
            format!("-- Trigger definition unavailable: {}\n", trig.name)
        } else {
            format!("{};\n", trig.definition.trim_end_matches(';'))
        }
    }

    fn generate_sequence_ddl(&self, seq: &SequenceInfo) -> String {
        format!("-- MySQL sequence definition unavailable: {}\n", seq.name)
    }

    fn generate_column_ddl(&self, table: &TableInfo, column: &ColumnInfo) -> String {
        format!(
            "ALTER TABLE {} ADD COLUMN {} {};\n",
            mysql_qualified_name(&table.schema, &table.name),
            quote_mysql_identifier(&column.name),
            format_column_type(column)
        )
    }
}

impl DdlGenerator for SQLiteDdlGenerator {
    fn generate_schema_ddl(&self, schema: &SchemaInfo) -> String {
        format!("-- SQLite schema: {}\n", schema.name)
    }

    fn generate_table_ddl(&self, schema: &DatabaseSchema, table: &TableInfo) -> String {
        let mut ddl = generic_table_ddl(schema, table, quote_identifier, sqlite_qualified_name);

        for idx in &schema.indexes {
            if idx.schema == table.schema && idx.table_name == table.name && !idx.is_primary {
                ddl.push('\n');
                ddl.push_str(&self.generate_index_ddl(idx));
            }
        }

        ddl
    }

    fn generate_view_ddl(&self, _schema: &DatabaseSchema, table: &TableInfo) -> String {
        format!(
            "-- View: {}\n-- View definition is not available in schema cache.\n",
            sqlite_qualified_name(&table.schema, &table.name)
        )
    }

    fn generate_function_ddl(&self, func: &FunctionInfo) -> String {
        format!("-- SQLite function definition unavailable: {}\n", func.name)
    }

    fn generate_index_ddl(&self, idx: &IndexInfo) -> String {
        generic_index_ddl(idx, quote_identifier, sqlite_qualified_name)
    }

    fn generate_trigger_ddl(&self, trig: &TriggerInfo) -> String {
        if trig.definition.is_empty() {
            format!("-- Trigger definition unavailable: {}\n", trig.name)
        } else {
            format!("{};\n", trig.definition.trim_end_matches(';'))
        }
    }

    fn generate_sequence_ddl(&self, seq: &SequenceInfo) -> String {
        format!("-- SQLite does not support sequences: {}\n", seq.name)
    }

    fn generate_column_ddl(&self, table: &TableInfo, column: &ColumnInfo) -> String {
        format!(
            "ALTER TABLE {} ADD COLUMN {} {};\n",
            sqlite_qualified_name(&table.schema, &table.name),
            quote_identifier(&column.name),
            format_column_type(column)
        )
    }
}

impl DdlGenerator for DuckDbDdlGenerator {
    fn generate_schema_ddl(&self, schema: &SchemaInfo) -> String {
        format!("CREATE SCHEMA {};\n", quote_identifier(&schema.name))
    }

    fn generate_table_ddl(&self, schema: &DatabaseSchema, table: &TableInfo) -> String {
        let mut ddl = generic_table_ddl(schema, table, quote_identifier, duckdb_qualified_name);

        for idx in &schema.indexes {
            if idx.schema == table.schema && idx.table_name == table.name && !idx.is_primary {
                ddl.push('\n');
                ddl.push_str(&self.generate_index_ddl(idx));
            }
        }

        ddl
    }

    fn generate_view_ddl(&self, _schema: &DatabaseSchema, table: &TableInfo) -> String {
        format!(
            "-- View: {}\n-- View definition is not available in schema cache.\n",
            duckdb_qualified_name(&table.schema, &table.name)
        )
    }

    fn generate_function_ddl(&self, func: &FunctionInfo) -> String {
        format!("-- DuckDB function definition unavailable: {}\n", func.name)
    }

    fn generate_index_ddl(&self, idx: &IndexInfo) -> String {
        generic_index_ddl(idx, quote_identifier, duckdb_qualified_name)
    }

    fn generate_trigger_ddl(&self, trig: &TriggerInfo) -> String {
        format!("-- DuckDB trigger definition unavailable: {}\n", trig.name)
    }

    fn generate_sequence_ddl(&self, seq: &SequenceInfo) -> String {
        format!("-- DuckDB sequence definition unavailable: {}\n", seq.name)
    }

    fn generate_column_ddl(&self, table: &TableInfo, column: &ColumnInfo) -> String {
        format!(
            "ALTER TABLE {} ADD COLUMN {} {};\n",
            duckdb_qualified_name(&table.schema, &table.name),
            quote_identifier(&column.name),
            format_column_type(column)
        )
    }
}

impl DdlGenerator for DatabendDdlGenerator {
    fn generate_schema_ddl(&self, schema: &SchemaInfo) -> String {
        format!("CREATE DATABASE {};\n", quote_identifier(&schema.name))
    }

    fn generate_table_ddl(&self, schema: &DatabaseSchema, table: &TableInfo) -> String {
        let mut ddl = generic_table_ddl(schema, table, quote_identifier, databend_qualified_name);

        for idx in &schema.indexes {
            if idx.schema == table.schema && idx.table_name == table.name && !idx.is_primary {
                ddl.push('\n');
                ddl.push_str(&self.generate_index_ddl(idx));
            }
        }

        ddl
    }

    fn generate_view_ddl(&self, _schema: &DatabaseSchema, table: &TableInfo) -> String {
        format!(
            "-- View: {}\n-- View definition is not available in schema cache.\n",
            databend_qualified_name(&table.schema, &table.name)
        )
    }

    fn generate_function_ddl(&self, func: &FunctionInfo) -> String {
        format!(
            "-- Databend function definition unavailable: {}\n",
            func.name
        )
    }

    fn generate_index_ddl(&self, idx: &IndexInfo) -> String {
        generic_index_ddl(idx, quote_identifier, databend_qualified_name)
    }

    fn generate_trigger_ddl(&self, trig: &TriggerInfo) -> String {
        format!(
            "-- Databend trigger definition unavailable: {}\n",
            trig.name
        )
    }

    fn generate_sequence_ddl(&self, seq: &SequenceInfo) -> String {
        format!(
            "-- Databend sequence definition unavailable: {}\n",
            seq.name
        )
    }

    fn generate_column_ddl(&self, table: &TableInfo, column: &ColumnInfo) -> String {
        format!(
            "ALTER TABLE {} ADD COLUMN {} {};\n",
            databend_qualified_name(&table.schema, &table.name),
            quote_identifier(&column.name),
            format_column_type(column)
        )
    }
}

fn generic_table_ddl(
    schema: &DatabaseSchema,
    table: &TableInfo,
    quote: fn(&str) -> String,
    qualified: fn(&str, &str) -> String,
) -> String {
    let mut ddl = format!("CREATE TABLE {} (\n", qualified(&table.schema, &table.name));
    let mut definitions = table
        .columns
        .iter()
        .map(|col| {
            let mut def = format!("    {} {}", quote(&col.name), format_column_type(col));
            if !col.nullable {
                def.push_str(" NOT NULL");
            }
            if let Some(default) = &col.default_value {
                def.push_str(&format!(" DEFAULT {}", default));
            }
            if col.is_pk && table.columns.iter().filter(|c| c.is_pk).count() == 1 {
                def.push_str(" PRIMARY KEY");
            }
            def
        })
        .collect::<Vec<_>>();

    // Multi-column primary key constraint
    let pk_columns: Vec<&ColumnInfo> = table.columns.iter().filter(|c| c.is_pk).collect();
    if pk_columns.len() > 1 {
        let pk_col_list: Vec<String> = pk_columns.iter().map(|c| quote(&c.name)).collect();
        definitions.push(format!(
            "    CONSTRAINT {}_pkey PRIMARY KEY ({})",
            quote(&table.name),
            pk_col_list.join(", ")
        ));
    }

    definitions.extend(
        schema
            .foreign_keys
            .iter()
            .filter(|fk| fk.source_schema == table.schema && fk.source_table == table.name)
            .map(|fk| {
                format!(
                    "    CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
                    quote(&fk.name),
                    fk.source_columns
                        .iter()
                        .map(|column| quote(column))
                        .collect::<Vec<_>>()
                        .join(", "),
                    qualified(&fk.target_schema, &fk.target_table),
                    fk.target_columns
                        .iter()
                        .map(|column| quote(column))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }),
    );

    ddl.push_str(&definitions.join(",\n"));
    ddl.push_str("\n);\n");
    ddl
}

fn generic_index_ddl(
    idx: &IndexInfo,
    quote: fn(&str) -> String,
    qualified: fn(&str, &str) -> String,
) -> String {
    let unique = if idx.is_unique { "UNIQUE " } else { "" };
    format!(
        "CREATE {}INDEX {} ON {} ({});\n",
        unique,
        quote(&idx.name),
        qualified(&idx.schema, &idx.table_name),
        idx.columns
            .iter()
            .map(|column| quote(column))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn quote_mysql_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}

fn mysql_qualified_name(schema: &str, name: &str) -> String {
    if schema.is_empty() {
        quote_mysql_identifier(name)
    } else {
        format!(
            "{}.{}",
            quote_mysql_identifier(schema),
            quote_mysql_identifier(name)
        )
    }
}

fn sqlite_qualified_name(schema: &str, name: &str) -> String {
    if schema.is_empty() || schema == "main" {
        quote_identifier(name)
    } else {
        format!("{}.{}", quote_identifier(schema), quote_identifier(name))
    }
}

fn duckdb_qualified_name(schema: &str, name: &str) -> String {
    sqlite_qualified_name(schema, name)
}

fn databend_qualified_name(schema: &str, name: &str) -> String {
    if schema.is_empty() {
        quote_identifier(name)
    } else {
        format!("{}.{}", quote_identifier(schema), quote_identifier(name))
    }
}

/// DDL generator factory based on database type.
pub fn create_ddl_generator(db_type: Database) -> Box<dyn DdlGenerator> {
    match db_type {
        Database::Postgres => Box::new(PostgresDdlGenerator),
        Database::MySql => Box::new(MySqlDdlGenerator),
        Database::SQLite => Box::new(SQLiteDdlGenerator),
        Database::DuckDB => Box::new(DuckDbDdlGenerator),
        Database::Databend => Box::new(DatabendDdlGenerator),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quote_identifier_simple() {
        assert_eq!(quote_identifier("users"), "users");
        assert_eq!(quote_identifier("id"), "id");
    }

    #[test]
    fn test_quote_identifier_needs_quoting() {
        assert_eq!(quote_identifier("user"), "\"user\"");
        assert_eq!(quote_identifier("123abc"), "\"123abc\"");
        assert_eq!(quote_identifier("my-table"), "\"my-table\"");
    }

    #[test]
    fn test_qualified_name() {
        assert_eq!(qualified_name("public", "users"), "\"public\".users");
        assert_eq!(qualified_name("public", "user"), "\"public\".\"user\"");
    }

    #[test]
    fn test_generate_schema_ddl() {
        let generator = PostgresDdlGenerator;
        let schema = SchemaInfo {
            name: "public".to_string(),
            owner: "postgres".to_string(),
        };
        let ddl = generator.generate_schema_ddl(&schema);
        assert!(ddl.contains("CREATE SCHEMA \"public\";"));
    }

    #[test]
    fn test_generate_index_ddl() {
        let generator = PostgresDdlGenerator;
        let idx = IndexInfo {
            schema: "public".to_string(),
            table_name: "users".to_string(),
            name: "idx_users_email".to_string(),
            is_unique: true,
            is_primary: false,
            columns: vec!["email".to_string()],
        };
        let ddl = generator.generate_index_ddl(&idx);
        assert!(ddl.contains("CREATE UNIQUE INDEX"));
        assert!(ddl.contains("idx_users_email"));
        assert!(ddl.contains("ON \"public\".users"));
        assert!(ddl.contains("(email)"));
    }

    #[test]
    fn test_generate_sequence_ddl() {
        let generator = PostgresDdlGenerator;
        let seq = SequenceInfo {
            schema: "public".to_string(),
            name: "users_id_seq".to_string(),
            data_type: "bigint".to_string(),
            start_value: "1".to_string(),
            min_value: "1".to_string(),
            max_value: "0".to_string(),
            increment_by: "1".to_string(),
        };
        let ddl = generator.generate_sequence_ddl(&seq);
        assert!(ddl.contains("CREATE SEQUENCE \"public\".users_id_seq"));
    }

    #[test]
    fn test_create_ddl_generator() {
        let generator = create_ddl_generator(Database::Postgres);
        // Just verify it doesn't panic
        let schema = SchemaInfo {
            name: "test".to_string(),
            owner: "".to_string(),
        };
        let _ = generator.generate_schema_ddl(&schema);
    }
}
