//! The statements of migrations 0011 and 0013 that read an identifier from stored JSON and
//! cast it to a UUID guard the cast: a stored value that is not a UUID does not stop the
//! upgrade, and the row gets the result that the migration states for an identifier that
//! names nothing. The first two tests run the statements on the newest schema beside valid
//! identifiers; the third runs the migrations themselves on a database of the schema before
//! 0011.

use junto_protocol::ids::WorkspaceId;
use serde_json::{Value, json};

use crate::recordings_support::Studio;
use crate::support::TestServer;

const NIL: &str = "00000000-0000-0000-0000-000000000000";

/// The plans under which the statements run: the plan that PostgreSQL chooses, and a plan
/// that reads every table through an index and joins with nested loops, in which a cast in
/// an index condition would run before a filter of the same scan.
const PLANS: [&str; 2] = [
    "",
    "set local enable_seqscan = off; set local enable_hashjoin = off; \
     set local enable_mergejoin = off;",
];

/// Values that a stored identifier can hold and that are not a UUID.
fn malformed(valid: &str) -> Vec<Value> {
    vec![
        json!("not-a-uuid"),
        json!(""),
        json!(format!("{valid} ")),
        json!(format!("x{valid}")),
        json!(17),
        json!({ "id": valid }),
    ]
}

/// The part of `migration` from the first `from` to the end of the first `to` after it.
fn part_of(migration: &str, from: &str, to: &str) -> String {
    let start = migration.find(from).expect("the start of the part");
    let end = start + migration[start..].find(to).expect("the end of the part") + to.len();
    migration[start..end].to_owned()
}

async fn stored_event(
    server: &TestServer,
    kind: &str,
    audience: Value,
    payload: Value,
) -> uuid::Uuid {
    let id = uuid::Uuid::now_v7();
    let mut payload = payload;
    payload["type"] = json!(kind);
    sqlx::query(
        "insert into events (id, recorded_at, class, kind, audience, payload)
         values ($1, now(), 'general', $2, $3, $4)",
    )
    .bind(id)
    .bind(kind)
    .bind(audience)
    .bind(payload)
    .execute(&server.db)
    .await
    .unwrap();
    id
}

async fn stored_audit_entry(server: &TestServer, recording_id: &Value) -> uuid::Uuid {
    let id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into audit_entries (id, occurred_at, action, event)
         values ($1, now(), 'recording_started', $2)",
    )
    .bind(id)
    .bind(json!({ "action": "recording_started", "recording_id": recording_id }))
    .execute(&server.db)
    .await
    .unwrap();
    id
}

async fn audience_of(server: &TestServer, id: uuid::Uuid) -> Value {
    sqlx::query_scalar("select audience from events where id = $1")
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

async fn run_in_plan(server: &TestServer, plan: &str, statements: &str) {
    let mut transaction = server.db.begin().await.unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("{plan}\n{statements}")))
        .execute(&mut *transaction)
        .await
        .unwrap_or_else(|error| panic!("plan {plan:?}: {error}"));
    transaction.commit().await.unwrap();
}

/// Migration 0011 gives a stored event about the inside of a call the call of its payload,
/// and an audit entry that names a recording the call of that recording.
#[tokio::test]
async fn migration_0011_reads_malformed_recording_identifiers_as_naming_nothing() {
    let migration = include_str!("../../migrations/0011_calls_inside.sql");
    let events = part_of(
        migration,
        "alter table events disable trigger events_no_change;\nupdate events e\nset audience = e.audience || jsonb_build_object('call_id'",
        "alter table events enable trigger events_no_change;",
    );
    let entries = part_of(
        migration,
        "alter table audit_entries disable trigger audit_entries_no_row_change;",
        "alter table audit_entries enable trigger audit_entries_no_row_change;",
    );
    for plan in PLANS {
        let studio = Studio::open().await;
        let server = &studio.scene.server;
        let recording = studio.record().await;
        let valid = recording.id.to_string();
        let call = recording.call_id.to_string();

        let named = |recording_id: Value| json!({ "recording_id": recording_id });
        let to_call = json!({ "to": "call" });
        let known = stored_event(
            server,
            "recording_probe",
            to_call.clone(),
            named(json!(valid)),
        )
        .await;
        let known_entry = stored_audit_entry(server, &json!(valid)).await;
        let mut unknown = Vec::new();
        for value in malformed(&valid) {
            let event = stored_event(
                server,
                "recording_probe",
                to_call.clone(),
                named(value.clone()),
            )
            .await;
            let entry = stored_audit_entry(server, &value).await;
            unknown.push((value, event, entry));
        }

        run_in_plan(server, plan, &format!("{events}\n{entries}")).await;

        assert_eq!(
            audience_of(server, known).await,
            json!({ "to": "call", "call_id": call })
        );
        let call_of = |entry: uuid::Uuid| async move {
            sqlx::query_scalar::<_, Option<uuid::Uuid>>(
                "select call_id from audit_entries where id = $1",
            )
            .bind(entry)
            .fetch_one(&server.db)
            .await
            .unwrap()
        };
        assert_eq!(
            call_of(known_entry).await,
            Some(recording.call_id.as_uuid())
        );
        for (value, event, entry) in unknown {
            assert_eq!(
                audience_of(server, event).await,
                json!({ "to": "call", "call_id": NIL }),
                "{plan:?} {value}"
            );
            assert_eq!(call_of(entry).await, None, "{plan:?} {value}");
        }
    }
}

/// Migration 0013 gives a stored event of an invite the audience of its invite, and an
/// event whose invite does not exist the audience of the administrators.
#[tokio::test]
async fn migration_0013_reads_malformed_invite_identifiers_as_naming_nothing() {
    let migration = include_str!("../../migrations/0013_limits_after_audit.sql");
    let start = migration
        .find("alter table events disable trigger")
        .unwrap();
    let statements = &migration[start..];
    for plan in PLANS {
        let server = TestServer::start().await;
        let owner = server.owner().await;
        let workspace: WorkspaceId = server.default_workspace(&owner).await.id;
        let invite = server
            .invite(&owner, json!({ "workspace_ids": [workspace] }))
            .await;
        let valid = invite.invite.id.to_string();
        let earlier = json!({ "to": "server_permission", "permission": "create_invites" });

        let known = stored_event(
            &server,
            "invite_revoked",
            earlier.clone(),
            json!({ "invite_id": valid }),
        )
        .await;
        let mut unknown = Vec::new();
        for value in malformed(&valid) {
            for payload in [
                json!({ "invite": { "id": value } }),
                json!({ "invite_id": value }),
            ] {
                let event =
                    stored_event(&server, "invite_created", earlier.clone(), payload.clone()).await;
                unknown.push((payload, event));
            }
        }

        run_in_plan(&server, plan, statements).await;

        assert_eq!(
            audience_of(&server, known).await,
            json!({
                "to": "invite",
                "created_by": owner.account_id,
                "workspace_ids": [workspace],
            })
        );
        for (payload, event) in unknown {
            assert_eq!(
                audience_of(&server, event).await,
                json!({ "to": "server_permission", "permission": "administrator" }),
                "{plan:?} {payload}"
            );
        }
    }
}

/// The migrations of `MIGRATIONS` up to `version`.
fn migrations_up_to(version: i64) -> sqlx::migrate::Migrator {
    let migrations = junto_server::bootstrap::MIGRATIONS
        .iter()
        .filter(|migration| migration.version <= version)
        .cloned()
        .collect();
    sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(migrations),
        ..sqlx::migrate::Migrator::DEFAULT
    }
}

/// An upgrade from the schema of migration 0010 runs 0011 to 0013 on stored values that are
/// not UUIDs, with the indexes of that time and under each plan, and then the rest of the
/// migrations.
#[tokio::test]
async fn an_upgrade_from_before_0011_reads_malformed_identifiers_as_naming_nothing() {
    use sqlx::{ConnectOptions, Connection};

    let unknown = uuid::Uuid::new_v4().to_string();
    for plan in PLANS {
        let database = crate::support::empty_database().await;
        let mut connection = database.options.connect().await.unwrap();
        migrations_up_to(10).run(&mut connection).await.unwrap();

        let mut values = malformed(&unknown);
        values.push(json!(unknown));
        let earlier = json!({ "to": "server_permission", "permission": "create_invites" });
        let mut stored = Vec::new();
        for value in values {
            let events: [(&str, Value, Value); 3] = [
                (
                    "recording_probe",
                    json!({ "to": "call" }),
                    json!({ "recording_id": value }),
                ),
                (
                    "invite_created",
                    earlier.clone(),
                    json!({ "invite": { "id": value } }),
                ),
                (
                    "invite_revoked",
                    earlier.clone(),
                    json!({ "invite_id": value }),
                ),
            ];
            for (kind, audience, mut payload) in events {
                payload["type"] = json!(kind);
                let id = uuid::Uuid::now_v7();
                sqlx::query(
                    "insert into events (id, recorded_at, class, kind, audience, payload)
                     values ($1, now(), 'general', $2, $3, $4)",
                )
                .bind(id)
                .bind(kind)
                .bind(audience)
                .bind(payload)
                .execute(&mut connection)
                .await
                .unwrap();
                stored.push((kind, value.clone(), id));
            }
            let entry = uuid::Uuid::now_v7();
            sqlx::query(
                "insert into audit_entries (id, occurred_at, action, event)
                 values ($1, now(), 'recording_started', $2)",
            )
            .bind(entry)
            .bind(json!({ "action": "recording_started", "recording_id": value }))
            .execute(&mut connection)
            .await
            .unwrap();
            stored.push(("audit_entry", value, entry));
        }

        sqlx::raw_sql(sqlx::AssertSqlSafe(plan.replace("set local", "set")))
            .execute(&mut connection)
            .await
            .unwrap();
        migrations_up_to(13)
            .run(&mut connection)
            .await
            .unwrap_or_else(|error| panic!("plan {plan:?}: {error}"));

        for (kind, value, id) in stored {
            if kind == "audit_entry" {
                let call: Option<uuid::Uuid> =
                    sqlx::query_scalar("select call_id from audit_entries where id = $1")
                        .bind(id)
                        .fetch_one(&mut connection)
                        .await
                        .unwrap();
                assert_eq!(call, None, "{plan:?} {value}");
                continue;
            }
            let audience: Value = sqlx::query_scalar("select audience from events where id = $1")
                .bind(id)
                .fetch_one(&mut connection)
                .await
                .unwrap();
            let expected = if kind == "recording_probe" {
                json!({ "to": "call", "call_id": NIL })
            } else {
                json!({ "to": "server_permission", "permission": "administrator" })
            };
            assert_eq!(audience, expected, "{plan:?} {kind} {value}");
        }
        junto_server::bootstrap::MIGRATIONS
            .run(&mut connection)
            .await
            .unwrap_or_else(|error| panic!("plan {plan:?}: {error}"));
        connection.close().await.unwrap();
    }
}
