use crate::Cli;
use crate::support::{Device, TestServer, claim, join};

#[tokio::test(flavor = "multi_thread")]
async fn the_tool_refuses_a_glossary_change_when_another_writer_replaces_its_read() {
    let server = TestServer::start("Glossary conflict").await;
    let proxy = crate::support::FaultProxy::start(&server).await;
    let device = Device::new();
    let owner = device.add(&proxy.address()).await;
    owner.dispatch(claim(&server, "Ada")).await.unwrap();
    let workspace = owner.api().own_account().await.unwrap().workspace_ids[0];
    proxy.hold_answer_ending("GET", "/glossary");
    let tool = Cli::in_directory(device.directory.path());
    let command = tokio::task::spawn_blocking(move || {
        tool.run(&[
            "glossary",
            "add",
            &workspace.to_string(),
            "--text",
            "Earlier",
        ])
    });
    tokio::time::timeout(crate::stream::PATIENCE, async {
        while proxy.answers_waiting() == 0 && !command.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(proxy.answers_waiting(), 1, "the glossary read did not wait");
    owner
        .api()
        .set_glossary(workspace, &["Later".to_owned()], None)
        .await
        .unwrap();
    proxy.release();
    let (success, _, error) = command.await.unwrap();
    assert!(!success && error.contains("conflict"), "{error}");
    assert_eq!(
        owner.api().glossary(workspace).await.unwrap().terms,
        ["Later"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tool_lists_adds_changes_and_removes_glossary_terms_with_workspace_permissions() {
    let server = TestServer::start("Glossary").await;
    let first = Device::new();
    let owner = first.add(&server.address()).await;
    owner.dispatch(claim(&server, "Ada")).await.unwrap();
    let workspace = owner.api().own_account().await.unwrap().workspace_ids[0];
    let id = workspace.to_string();
    let tool = Cli::in_directory(first.directory.path());
    assert_eq!(tool.ok(&["glossary", "list", &id]), "");
    tool.ok(&["glossary", "add", &id, "--text", "Acme"]);
    tool.ok(&["glossary", "add", &id, "--text", "Project one"]);
    assert_eq!(
        tool.ok(&["glossary", "list", &id]),
        "1  Acme\n2  Project one\n"
    );
    tool.ok(&[
        "glossary", "change", &id, "--term", "Acme", "--text", "ACME",
    ]);
    assert_eq!(
        owner.api().glossary(workspace).await.unwrap().terms,
        ["ACME", "Project one"]
    );
    let (success, _, error) = tool.run(&["glossary", "remove", &id, "--term", "Acme"]);
    assert!(!success && error.contains("no glossary term"), "{error}");
    let (success, _, _) = tool.run(&["glossary", "add", &id, "--text", "ACME"]);
    assert!(!success);
    let (success, _, _) = tool.run(&["glossary", "add", &id, "--text", "<|invalid|>"]);
    assert!(!success);

    let code = crate::invite(&owner, 1).await;
    let second = Device::new();
    let member = second.add(&server.address()).await;
    member.dispatch(join(&code, "Grace")).await.unwrap();
    let reader = Cli::in_directory(second.directory.path());
    assert_eq!(
        reader.ok(&["glossary", "list", &id]),
        "1  ACME\n2  Project one\n"
    );
    for arguments in [
        vec!["glossary", "add", &id, "--text", "Other"],
        vec![
            "glossary", "change", &id, "--term", "ACME", "--text", "Other",
        ],
        vec!["glossary", "remove", &id, "--term", "ACME"],
    ] {
        let (success, _, error) = reader.run(&arguments);
        assert!(!success && error.contains("forbidden"), "{error}");
    }
    tool.ok(&["glossary", "remove", &id, "--term", "ACME"]);
    assert_eq!(
        owner.api().glossary(workspace).await.unwrap().terms,
        ["Project one"]
    );
    tool.ok(&["glossary", "remove", &id, "--term", "Project one"]);
    assert_eq!(tool.ok(&["glossary", "list", &id]), "");
    tool.ok(&["glossary", "add", &id, "--text", "--flag"]);
    tool.ok(&["glossary", "remove", &id, "--term", "--flag"]);
    assert_eq!(tool.ok(&["glossary", "list", &id]), "");
}
