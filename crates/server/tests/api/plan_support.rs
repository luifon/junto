//! What the tests of statements read from a plan of PostgreSQL.

use serde_json::Value;
use sqlx::PgPool;

/// The rows that the nodes of an executed plan read, the ones that a filter removed
/// included.
pub fn visited_rows(plan: &Value) -> u64 {
    let loops = plan["Actual Loops"].as_u64().unwrap_or(1);
    let rows = plan["Actual Rows"].as_u64().unwrap_or(0)
        + plan["Rows Removed by Filter"].as_u64().unwrap_or(0);
    loops * rows
        + plan["Plans"]
            .as_array()
            .map_or(0, |children| children.iter().map(visited_rows).sum())
}

/// The names of the indexes that a plan reads.
pub fn index_names(plan: &Value) -> Vec<String> {
    let mut found = Vec::new();
    collect(plan, &mut found);
    found
}

fn collect(plan: &Value, found: &mut Vec<String>) {
    match plan {
        Value::Object(node) => {
            if let Some(name) = node.get("Index Name").and_then(Value::as_str) {
                found.push(name.to_owned());
            }
            node.values().for_each(|child| collect(child, found));
        }
        Value::Array(nodes) => nodes.iter().for_each(|child| collect(child, found)),
        _ => {}
    }
}

/// Fresh statistics for `tables`, so that the plan is the one of a large table.
pub async fn analyze(db: &PgPool, tables: &[&str]) {
    for table in tables {
        sqlx::query(sqlx::AssertSqlSafe(format!("analyze {table}")))
            .execute(db)
            .await
            .unwrap();
    }
}

/// The statement that executes `statement` and returns its plan.
pub fn explained(statement: &str) -> sqlx::AssertSqlSafe<String> {
    sqlx::AssertSqlSafe(format!("explain (analyze, format json) {statement}"))
}
