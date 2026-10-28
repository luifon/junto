use junto_authz::{WorkspacePermission as W, WorkspacePermissions};
use sqlx::ConnectOptions;

#[tokio::test]
async fn permission_migration_keeps_access_and_custom_channel_rules() {
    let database = crate::support::empty_database().await;
    let mut connection = database.options.connect().await.unwrap();
    crate::migration_identifiers::migrations_up_to(34)
        .run(&mut connection)
        .await
        .unwrap();
    let workspace = uuid::Uuid::now_v7();
    let everyone = uuid::Uuid::now_v7();
    let role = uuid::Uuid::now_v7();
    let category = uuid::Uuid::now_v7();
    let custom = uuid::Uuid::now_v7();
    let plain = uuid::Uuid::now_v7();
    sqlx::query("insert into workspaces(id,name,created_at) values($1,'Project',now())")
        .bind(workspace)
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("insert into workspace_roles(id,workspace_id,name,permissions,is_everyone,position,created_at) values($1,$2,'Everyone',24577,true,0,now()),($3,$2,'Editors',4,false,1,now())")
        .bind(everyone).bind(workspace).bind(role).execute(&mut connection).await.unwrap();
    sqlx::query("insert into categories(id,workspace_id,name,position,created_at) values($1,$2,'Plans',0,now())")
        .bind(category).bind(workspace).execute(&mut connection).await.unwrap();
    sqlx::query("insert into channels(id,workspace_id,category_id,kind,name,position,created_at) values($1,$2,$3,'text','Custom',0,now()),($4,$2,$3,'voice','Plain',1,now())")
        .bind(custom).bind(workspace).bind(category).bind(plain).execute(&mut connection).await.unwrap();
    sqlx::query("insert into channel_overrides(channel_id,role_id,workspace_id,allow,deny) values($1,$2,$3,4,1)")
        .bind(custom).bind(role).bind(workspace).execute(&mut connection).await.unwrap();
    crate::migration_identifiers::migrations_up_to(i64::MAX)
        .run(&mut connection)
        .await
        .unwrap();
    let bits: i64 = sqlx::query_scalar("select permissions from workspace_roles where id=$1")
        .bind(everyone)
        .fetch_one(&mut connection)
        .await
        .unwrap();
    let permissions = WorkspacePermissions::from_bits(bits as u64).unwrap();
    for permission in [
        W::SendMessages,
        W::SendMessagesInThreads,
        W::CreateThreads,
        W::ReadMessageHistory,
        W::AttachFiles,
        W::AddReactions,
        W::MentionRoles,
        W::Connect,
        W::Speak,
        W::Video,
        W::ShareScreen,
        W::UseVoiceActivity,
        W::SetVoiceChannelStatus,
    ] {
        assert!(
            permissions.contains(permission),
            "missing {}",
            permission.name()
        );
    }
    assert!(!permissions.contains(W::SetCallUserLimit));
    let bits: i64 = sqlx::query_scalar("select permissions from workspace_roles where id=$1")
        .bind(role)
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(bits, 4 | (1 << 29));
    let rules: (i64, i64) =
        sqlx::query_as("select allow,deny from channel_overrides where channel_id=$1")
            .bind(custom)
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(rules, (4 | (1 << 29), 1));
    let synced: Vec<(uuid::Uuid, bool)> =
        sqlx::query_as("select id,permissions_synced from channels order by position")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    assert_eq!(synced, [(custom, false), (plain, true)]);
    let tables: i64 = sqlx::query_scalar("select count(*) from information_schema.tables where table_schema='public' and table_name in ('server_person_overrides','workspace_person_overrides','category_overrides','category_person_overrides','channel_person_overrides')")
        .fetch_one(&mut connection).await.unwrap();
    assert_eq!(tables, 5);
    sqlx::query("insert into category_overrides(category_id,role_id,workspace_id,allow,deny) values($1,$2,$3,64,1)")
        .bind(category).bind(role).bind(workspace).execute(&mut connection).await.unwrap();
    let effective: Vec<(uuid::Uuid, i64, i64)> = sqlx::query_as(
        "select channel_id,allow,deny from effective_channel_overrides order by allow",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(effective, [(plain, 64, 1), (custom, 4 | (1 << 29), 1)]);
    let account = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into accounts(id,name,status,created_at) values($1,'Reader','active',now())",
    )
    .bind(account)
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query(
        "insert into workspace_members(workspace_id,account_id,joined_at) values($1,$2,now())",
    )
    .bind(workspace)
    .bind(account)
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query("insert into category_person_overrides(category_id,account_id,workspace_id,allow,deny) values($1,$2,$3,1,64)")
        .bind(category).bind(account).bind(workspace).execute(&mut connection).await.unwrap();
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,64,1)")
        .bind(custom).bind(account).bind(workspace).execute(&mut connection).await.unwrap();
    let effective: Vec<(uuid::Uuid, i64, i64)> = sqlx::query_as(
        "select channel_id,allow,deny from effective_channel_person_overrides order by allow",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(effective, [(plain, 1, 64), (custom, 64, 1)]);
    assert!(
        sqlx::query("insert into server_person_overrides(account_id,allow,deny) values($1,1,1)")
            .bind(account)
            .execute(&mut connection)
            .await
            .is_err()
    );
    assert!(sqlx::query("insert into workspace_person_overrides(workspace_id,account_id,allow,deny) values($1,$2,2147483648,0)")
        .bind(workspace).bind(account).execute(&mut connection).await.is_err());
    sqlx::query("delete from workspace_members where workspace_id=$1 and account_id=$2")
        .bind(workspace)
        .bind(account)
        .execute(&mut connection)
        .await
        .unwrap();
    let left: i64 = sqlx::query_scalar("select count(*) from effective_channel_person_overrides")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(left, 2);
    sqlx::query("delete from accounts where id=$1")
        .bind(account)
        .execute(&mut connection)
        .await
        .unwrap();
    let left: i64 = sqlx::query_scalar("select count(*) from effective_channel_person_overrides")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn person_rules_copied_by_the_migration_stay_when_the_membership_ends() {
    let database = crate::support::empty_database().await;
    let mut connection = database.options.connect().await.unwrap();
    crate::migration_identifiers::migrations_up_to(42)
        .run(&mut connection)
        .await
        .unwrap();
    let workspace = uuid::Uuid::now_v7();
    let category = uuid::Uuid::now_v7();
    let channel = uuid::Uuid::now_v7();
    let account = uuid::Uuid::now_v7();
    sqlx::query("insert into workspaces(id,name,created_at) values($1,'Project',now())")
        .bind(workspace)
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("insert into categories(id,workspace_id,name,position,created_at) values($1,$2,'Plans',0,now())")
        .bind(category).bind(workspace).execute(&mut connection).await.unwrap();
    sqlx::query("insert into channels(id,workspace_id,category_id,kind,name,position,created_at) values($1,$2,$3,'text','Plans',0,now())")
        .bind(channel).bind(workspace).bind(category).execute(&mut connection).await.unwrap();
    sqlx::query(
        "insert into accounts(id,name,status,created_at) values($1,'Reader','active',now())",
    )
    .bind(account)
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query(
        "insert into workspace_members(workspace_id,account_id,joined_at) values($1,$2,now())",
    )
    .bind(workspace)
    .bind(account)
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query("insert into workspace_person_overrides(workspace_id,account_id,allow,deny) values($1,$2,0,4)")
        .bind(workspace).bind(account).execute(&mut connection).await.unwrap();
    sqlx::query("insert into category_person_overrides(category_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,4)")
        .bind(category).bind(account).bind(workspace).execute(&mut connection).await.unwrap();
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,4)")
        .bind(channel).bind(account).bind(workspace).execute(&mut connection).await.unwrap();
    crate::migration_identifiers::migrations_up_to(i64::MAX)
        .run(&mut connection)
        .await
        .unwrap();
    let count = |table: &'static str| {
        sqlx::AssertSqlSafe(format!(
            "select count(*) from {table} where account_id = $1"
        ))
    };
    let tables = [
        "workspace_person_overrides",
        "category_person_overrides",
        "channel_person_overrides",
    ];
    for table in tables {
        let kept: i64 = sqlx::query_scalar(count(table))
            .bind(account)
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(kept, 1, "{table} after the migration");
    }
    sqlx::query("delete from workspace_members where workspace_id=$1 and account_id=$2")
        .bind(workspace)
        .bind(account)
        .execute(&mut connection)
        .await
        .unwrap();
    for table in tables {
        let kept: i64 = sqlx::query_scalar(count(table))
            .bind(account)
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(kept, 1, "{table} after the membership ended");
    }
    sqlx::query("delete from accounts where id=$1")
        .bind(account)
        .execute(&mut connection)
        .await
        .unwrap();
    for table in tables {
        let kept: i64 = sqlx::query_scalar(count(table))
            .bind(account)
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(kept, 0, "{table} after the account was deleted");
    }
}
