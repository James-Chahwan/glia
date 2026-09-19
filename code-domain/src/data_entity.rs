//! ORM data entities: the table-cell writer / reader (`table_cell` /
//! `table_of`) that records which table an ORM model maps to.
//!
//! # ORM entity identity rule (A13.1; applied by A13.10–A13.17)
//!
//! An ORM-declared entity is keyed on its MODEL name,
//! `data_entity:<flavor>:<Model>` — the one token every query site in any file
//! can name (a Java repository's generic argument, an ActiveRecord / Eloquent
//! static receiver, `getRepository(User)`, `db.Model(&User{})`,
//! `Order.objects`, `DbSet<User>`). A table-keyed id would orphan every
//! repository or query site that lives in a different file from the model.
//!
//! A table name the ORM DECLARES (`@Table`, `@Document(collection)`,
//! `ToTable`, `[Table]`, `TableName()`, `self.table_name`, `$table`,
//! `@Entity('x')`, `@@map`, `db_table`), or derives by a rule that is not a
//! plain plural of the model (Django's `<app_label>_<model>`), rides a CODE
//! cell built by [`table_cell`] and emitted ONLY at the declaration site.
//! Duplicate-id nodes stack their cells when the graph builder merges file
//! parses, so query-site files never need to know the table.
//!
//! The graph crate's `DbResolver` reads the cell back through [`table_of`] and
//! joins the model to the table other services name directly. It lives here,
//! not in the graph crate or the extractors crate, because every writer (the
//! language parsers and the prisma extractor) and the one reader share only
//! this crate, and the graph crate keeps `serde_json` out of its dependencies.

use glia_core::{Cell, CellPayload};

use crate::cell_type;

/// The `orm` tags a table cell may carry, one per ORM whose declaration site
/// emits it. Writers pass these constants rather than spelling the tag inline,
/// so the reader and every writer agree on the vocabulary.
pub mod orm {
    /// JPA / Hibernate: `@Table(name = …)`, `@Document(collection = …)`.
    pub const JPA: &str = "jpa";
    /// Entity Framework Core: `ToTable(…)`, `[Table(…)]`.
    pub const EFCORE: &str = "efcore";
    /// GORM: a `TableName()` method on the model.
    pub const GORM: &str = "gorm";
    /// Rails ActiveRecord: `self.table_name = …`.
    pub const ACTIVERECORD: &str = "activerecord";
    /// Laravel Eloquent: `protected $table = …`.
    pub const ELOQUENT: &str = "eloquent";
    /// TypeORM: `@Entity('x')`.
    pub const TYPEORM: &str = "typeorm";
    /// Prisma: `@@map("x")`.
    pub const PRISMA: &str = "prisma";
    /// Django: `Meta.db_table`, or the derived `<app_label>_<model>`.
    pub const DJANGO: &str = "django";
}

/// Build the CODE cell recording that an ORM model maps to `table`.
///
/// Payload: `CellPayload::Json` holding `{"orm": <orm>, "table": <table>}`.
/// Built through `serde_json`, so a table name containing a quote or
/// backslash is escaped rather than corrupting the payload.
pub fn table_cell(table: &str, orm: &str) -> Cell {
    let payload = serde_json::json!({ "table": table, "orm": orm }).to_string();
    Cell {
        kind: cell_type::CODE,
        payload: CellPayload::Json(payload),
    }
}

/// The table recorded by the first CODE `Json` cell in `cells` that carries a
/// non-blank string `table` key, or `None` when there is none.
///
/// Other CODE `Json` cells (cron schedules, queue payloads) have no `table`
/// key and are skipped, as are `Text` / `Bytes` payloads and cells of any
/// other type.
pub fn table_of(cells: &[Cell]) -> Option<String> {
    cells.iter().find_map(|cell| {
        if cell.kind != cell_type::CODE {
            return None;
        }
        let CellPayload::Json(raw) = &cell.payload else {
            return None;
        };
        let value: serde_json::Value = serde_json::from_str(raw).ok()?;
        let table = value.get("table")?.as_str()?.trim();
        (!table.is_empty()).then(|| table.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_cell_round_trips_through_table_of() {
        let cell = table_cell("app_users", orm::JPA);
        assert_eq!(cell.kind, cell_type::CODE);
        assert_eq!(table_of(&[cell]), Some("app_users".to_string()));
    }

    #[test]
    fn table_cell_escapes_quotes_in_the_table_name() {
        let cell = table_cell(r#"we"ird\table"#, orm::DJANGO);
        assert_eq!(table_of(&[cell]), Some(r#"we"ird\table"#.to_string()));
    }

    #[test]
    fn table_of_skips_cells_without_a_table() {
        let cron = Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Json(r#"{"schedule":"* * * * *","target":"job"}"#.into()),
        };
        let text = Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(r#"{"table":"users"}"#.into()),
        };
        let doc = Cell {
            kind: cell_type::DOC,
            payload: CellPayload::Json(r#"{"table":"users"}"#.into()),
        };
        let blank = table_cell("  ", orm::GORM);
        let broken = Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Json("{not json".into()),
        };
        assert_eq!(table_of(&[cron.clone(), text, doc, blank, broken]), None);
        assert_eq!(
            table_of(&[cron, table_cell("orders", orm::GORM), table_cell("x", orm::GORM)]),
            Some("orders".to_string()),
            "the FIRST cell carrying a table wins"
        );
    }
}
