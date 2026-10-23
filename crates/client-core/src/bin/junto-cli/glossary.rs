use junto_client_core::api::ServerApi;
use junto_client_core::protocol::ids::WorkspaceId;
use junto_client_core::protocol::transcript::Glossary;

use crate::discovery::{identifier, word};
use crate::{Arguments, Client, Failure, end_session, failed, logged_in, shown, usage};

enum Action {
    List,
    Add(String),
    Change { term: String, text: String },
    Remove(String),
}

fn parse(arguments: &Arguments) -> Result<(WorkspaceId, Action), Failure> {
    let words = &arguments.positional;
    let action = word(words, 1, "glossary action")?;
    let workspace = identifier(words, 2, "workspace")?;
    if words.len() != 3 {
        return Err(usage(
            "glossary takes an action and a workspace; terms use --term and --text",
        ));
    }
    let required = |name| {
        arguments
            .value(name)
            .map(str::to_owned)
            .ok_or_else(|| usage(format!("--{name} is required")))
    };
    let action = match action {
        "list" => Action::List,
        "add" => Action::Add(required("text")?),
        "change" => Action::Change {
            term: required("term")?,
            text: required("text")?,
        },
        "remove" => Action::Remove(required("term")?),
        _ => return Err(usage("glossary action must be list, add, change or remove")),
    };
    Ok((workspace, action))
}

async fn perform(
    api: &ServerApi,
    workspace: WorkspaceId,
    action: Action,
) -> Result<Glossary, Failure> {
    let glossary = api.glossary(workspace).await.map_err(failed)?;
    if matches!(action, Action::List) {
        return Ok(glossary);
    }
    let mut terms = glossary.terms.clone();
    match action {
        Action::List => {}
        Action::Add(text) => terms.push(text),
        Action::Change { term, text } => {
            let index = position(&terms, &term)?;
            terms[index] = text;
        }
        Action::Remove(term) => {
            let index = position(&terms, &term)?;
            terms.remove(index);
        }
    }
    api.set_glossary(workspace, &terms, Some(&glossary.terms))
        .await
        .map_err(failed)
}

fn position(terms: &[String], term: &str) -> Result<usize, Failure> {
    terms
        .iter()
        .position(|held| held == term)
        .ok_or_else(|| usage("no glossary term has that exact text; list the glossary again"))
}

pub(crate) async fn run(client: &Client, arguments: &Arguments) -> Result<(), Failure> {
    let (workspace, action) = parse(arguments)?;
    let server = logged_in(client, arguments).await?;
    let result = perform(&server.api(), workspace, action).await;
    end_session(&server).await;
    for (index, term) in result?.terms.iter().enumerate() {
        println!("{}  {}", index + 1, shown(term));
    }
    Ok(())
}
