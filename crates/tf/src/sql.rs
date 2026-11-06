//! `tf sql`: browse a SQLite database read-only: tables with row counts and schema, or the rows of one table.
//!
//! The file is opened read-only with `query_only` on, extensions off, and nothing is ever written. Table names are checked
//! against the database's own list and quoted as identifiers, never spliced in as typed.

use std::path::Path;

use mahzenterm_viewer::{render_table, Options};
use rusqlite::{types::ValueRef, Connection, OpenFlags};

/// Most rows shown.
pub const MAX_ROWS: usize = 1000;
/// Longest cell text shown, in characters.
const MAX_CELL: usize = 120;

fn open(path: &Path) -> Result<Connection, String> {
    if !path.is_file() {
        return Err(format!("{}: no such file", path.display()));
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    conn.pragma_update(None, "query_only", true)
        .map_err(|e| e.to_string())?;
    // Reading the table list proves this really is a database.
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
        r.get::<_, i64>(0)
    })
    .map_err(|_| format!("{} is not a SQLite database", path.display()))?;
    Ok(conn)
}

/// Quote an identifier for SQL: double quotes doubled.
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn cell(v: ValueRef<'_>) -> String {
    let s = match v {
        ValueRef::Null => "NULL".to_owned(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => f.to_string(),
        ValueRef::Text(t) => String::from_utf8_lossy(t).replace(['\n', '\r', '\t'], " "),
        ValueRef::Blob(b) => format!("<blob {} bytes>", b.len()),
    };
    let clean: String = s.chars().filter(|c| !c.is_control()).collect();
    if clean.chars().count() > MAX_CELL {
        let mut t: String = clean.chars().take(MAX_CELL).collect();
        t.push('\u{2026}');
        t
    } else {
        clean
    }
}

/// Tables and views with their row counts and column lists.
pub fn overview(path: &Path, opts: &Options) -> Result<String, String> {
    let conn = open(path)?;
    let mut stmt = conn
        .prepare("SELECT name, type FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .map_err(|e| e.to_string())?;
    let items: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    let mut rows = vec![vec![
        "Name".to_owned(),
        "Kind".to_owned(),
        "Rows".to_owned(),
        "Columns".to_owned(),
    ]];
    for (name, kind) in &items {
        let count = conn
            .query_row(&format!("SELECT count(*) FROM {}", quote(name)), [], |r| {
                r.get::<_, i64>(0)
            })
            .map_or_else(|_| "?".to_owned(), |n| n.to_string());
        let cols: Vec<String> = conn
            .prepare(&format!(
                "SELECT name, type FROM pragma_table_info({})",
                quote_literal(name)
            ))
            .ok()
            .map(|mut s| {
                s.query_map([], |r| {
                    Ok(format!(
                        "{} {}",
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?.to_lowercase()
                    ))
                })
                .map(|it| it.filter_map(Result::ok).collect())
                .unwrap_or_default()
            })
            .unwrap_or_default();
        rows.push(vec![name.clone(), kind.clone(), count, cols.join(", ")]);
    }
    if items.is_empty() {
        return Ok("(no tables)\n".to_owned());
    }
    Ok(render_table(&rows, opts))
}

/// A string literal for SQL (single quotes doubled).
fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The rows of one table or view (at most `limit`, never more than [`MAX_ROWS`]).
pub fn table(path: &Path, name: &str, limit: usize, opts: &Options) -> Result<String, String> {
    let conn = open(path)?;
    let known: bool = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type IN ('table','view') AND name = ?1",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())?
        > 0;
    if !known {
        return Err(format!("no table or view named {name}"));
    }
    let limit = limit.clamp(1, MAX_ROWS);
    let mut stmt = conn
        .prepare(&format!("SELECT * FROM {} LIMIT {limit}", quote(name)))
        .map_err(|e| e.to_string())?;
    let header: Vec<String> = stmt.column_names().map(str::to_owned).collect();
    let n = header.len();
    let mut rows = vec![header];
    let mut q = stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(r) = q.next().map_err(|e| e.to_string())? {
        rows.push(
            (0..n)
                .map(|i| r.get_ref(i).map_or_else(|_| "?".to_owned(), cell))
                .collect(),
        );
    }
    let shown = rows.len() - 1;
    let total: i64 = conn
        .query_row(&format!("SELECT count(*) FROM {}", quote(name)), [], |r| {
            r.get(0)
        })
        .unwrap_or(0);
    let mut out = render_table(&rows, opts);
    if total as usize > shown {
        out.push_str(&format!("(showing {shown} of {total} rows; use --limit)\n"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Options {
        Options {
            width: 100,
            color: false,
            line_numbers: false,
        }
    }

    fn db() -> (std::path::PathBuf, Connection) {
        let dir = std::env::temp_dir().join(format!(
            "mz-sql-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let _ = std::fs::remove_file(&path);
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT, score REAL, photo BLOB);
             INSERT INTO people (name, score, photo) VALUES ('Ada', 9.5, x'0102'), ('Linus', 7, NULL), ('Grace', 8.25, x'00');
             CREATE TABLE \"we\"\"ird\" (x);
             CREATE VIEW top AS SELECT name FROM people WHERE score > 8;",
        )
        .unwrap();
        (path, c)
    }

    #[test]
    fn overview_lists_tables_views_counts_and_columns() {
        let (path, _c) = db();
        let t = overview(&path, &plain()).unwrap();
        assert!(
            t.contains("people") && t.contains("view") && t.contains("3"),
            "{t}"
        );
        assert!(t.contains("id integer") && t.contains("name text"), "{t}");
        assert!(t.contains("we\"ird"), "odd names work: {t}");
        assert!(!t.contains("sqlite_"), "internal tables are hidden");
    }

    #[test]
    fn table_rows_blobs_nulls_and_limits() {
        let (path, _c) = db();
        let t = table(&path, "people", 2, &plain()).unwrap();
        assert!(
            t.contains("Ada") && t.contains("9.5") && t.contains("<blob 2 bytes>"),
            "{t}"
        );
        assert!(
            !t.contains("Grace") && t.contains("showing 2 of 3 rows"),
            "{t}"
        );
        assert!(table(&path, "people", 99, &plain())
            .unwrap()
            .contains("NULL"));
        assert!(
            table(&path, "top", 10, &plain()).unwrap().contains("Ada"),
            "views work"
        );
    }

    #[test]
    fn names_are_checked_and_the_database_is_never_changed() {
        let (path, _c) = db();
        assert!(table(&path, "nope", 5, &plain())
            .unwrap_err()
            .contains("no table or view"));
        // An injection attempt is just an unknown table name.
        assert!(table(&path, "people; DROP TABLE people", 5, &plain()).is_err());
        assert!(table(&path, "people\" UNION SELECT 1 --", 5, &plain()).is_err());
        assert!(
            overview(&path, &plain()).unwrap().contains("people"),
            "the table is still there"
        );
        // Not a database / missing file.
        let junk = path.with_file_name("junk.db");
        std::fs::write(
            &junk,
            "this is not sqlite at all, just text that is long enough to look like a file",
        )
        .unwrap();
        assert!(overview(&junk, &plain())
            .unwrap_err()
            .contains("not a SQLite database"));
        assert!(overview(&path.with_file_name("missing.db"), &plain())
            .unwrap_err()
            .contains("no such file"));
        assert_eq!(quote("a\"b"), "\"a\"\"b\"");
    }
}
