//! The commands of the tool. Everything that the binary does is done here, so that a test
//! runs a command with its own arguments and reads what it prints.

use std::io::Write;
use std::path::Path;

use junto_protocol::ids::StructureId;
use junto_protocol::secret::AccessToken;
use junto_protocol::structure_file::{
    ApplyStructureChanges, MOST_CHANGES, STRUCTURE_FORMAT, StructureChange, StructureFile,
};
use serde::{Deserialize, Serialize};

use crate::api::{ApiFailure, Server, needs_new_plan};
use crate::plan::{Planned, plan};

/// The environment variable that holds the access token.
pub const TOKEN_VARIABLE: &str = "JUNTO_ACCESS_TOKEN";
/// The environment variable that holds the address of the server.
pub const SERVER_VARIABLE: &str = "JUNTO_SERVER";

pub const SUCCESS: u8 = 0;
pub const FAILURE: u8 = 1;
pub const USAGE_ERROR: u8 = 2;

pub const USAGE: &str = "usage: junto-setup <command> [options]

commands:
  export [--out <file>]             write the structure of the server to a file, or to
                                    standard output
  plan <file> [--out <plan file>]   show what applying the file would change; change nothing
  apply <file>                      compute the plan of the file and apply it
  apply --plan <plan file>          apply a plan that `plan --out` wrote

options:
  --server <address>      the address of the server, for example https://chat.example.test;
                          without it, the variable JUNTO_SERVER
  --token-file <file>     a file whose first line is the access token; without it, the
                          variable JUNTO_ACCESS_TOKEN

The access token is never an argument. `apply` applies all changes of a plan or none.
`plan` and `apply` do not delete what a file does not hold; a file lists the channels to
delete under \"delete\".";

/// What the binary was started with.
#[derive(Debug, Clone, Default)]
pub struct Invocation {
    pub arguments: Vec<String>,
    /// The value of [`TOKEN_VARIABLE`].
    pub token: Option<String>,
    /// The value of [`SERVER_VARIABLE`].
    pub server: Option<String>,
}

/// A plan as `plan --out` writes it and `apply --plan` reads it. It records the server and
/// the revision of the structure that it was computed from (ADR 0204).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanFile {
    pub format: u32,
    pub server: String,
    pub revision: String,
    pub changes: Vec<StructureChange>,
}

enum Failure {
    Usage(String),
    Other(String),
}

impl From<ApiFailure> for Failure {
    fn from(failure: ApiFailure) -> Self {
        Self::Other(failure.to_string())
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Self::Other(format!("cannot write the output: {error}"))
    }
}

#[derive(Default)]
struct Options {
    positional: Vec<String>,
    out: Option<String>,
    plan: Option<String>,
    server: Option<String>,
    token_file: Option<String>,
}

fn options(arguments: &[String]) -> Result<Options, Failure> {
    let mut options = Options::default();
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let Some(name) = argument.strip_prefix("--") else {
            options.positional.push(argument.clone());
            continue;
        };
        let slot = match name {
            "out" => &mut options.out,
            "plan" => &mut options.plan,
            "server" => &mut options.server,
            "token-file" => &mut options.token_file,
            "token" => {
                return Err(Failure::Usage(format!(
                    "the access token is never given on the command line; set {TOKEN_VARIABLE} \
                     or use --token-file"
                )));
            }
            _ => return Err(Failure::Usage(format!("unknown option --{name}"))),
        };
        let value = arguments
            .next()
            .ok_or_else(|| Failure::Usage(format!("--{name} needs a value")))?;
        *slot = Some(value.clone());
    }
    Ok(options)
}

fn read_file(path: &str, what: &str) -> Result<String, Failure> {
    std::fs::read_to_string(Path::new(path))
        .map_err(|error| Failure::Other(format!("cannot read the {what} {path}: {error}")))
}

fn write_file(path: &str, what: &str, content: &str) -> Result<(), Failure> {
    std::fs::write(Path::new(path), content)
        .map_err(|error| Failure::Other(format!("cannot write the {what} {path}: {error}")))
}

fn pretty<T: Serialize>(value: &T) -> Result<String, Failure> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|error| Failure::Other(format!("cannot write JSON: {error}")))?;
    text.push('\n');
    Ok(text)
}

fn server(invocation: &Invocation, options: &Options) -> Result<Server, Failure> {
    let address = options
        .server
        .clone()
        .or_else(|| invocation.server.clone())
        .filter(|address| !address.trim().is_empty())
        .ok_or_else(|| {
            Failure::Usage(format!(
                "no server address: give --server or set {SERVER_VARIABLE}"
            ))
        })?;
    let token = match &options.token_file {
        Some(path) => read_file(path, "token file")?
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned(),
        None => invocation
            .token
            .clone()
            .unwrap_or_default()
            .trim()
            .to_owned(),
    };
    if token.is_empty() {
        return Err(Failure::Usage(format!(
            "no access token: set {TOKEN_VARIABLE} or give --token-file"
        )));
    }
    Ok(Server::new(&address, AccessToken::new(token))?)
}

fn structure_file(path: &str) -> Result<StructureFile, Failure> {
    let text = read_file(path, "structure file")?;
    serde_json::from_str(&text).map_err(|error| {
        Failure::Other(format!(
            "the structure file {path} is not a structure of format {STRUCTURE_FORMAT}: {error}"
        ))
    })
}

struct Computed {
    revision: String,
    planned: Vec<Planned>,
}

async fn compute(server: &Server, path: &str) -> Result<Computed, Failure> {
    let file = structure_file(path)?;
    let export = server.structure().await?;
    let planned =
        plan(&file, &export.structure, &mut StructureId::generate).map_err(|problems| {
            Failure::Other(format!(
                "the structure file {path} cannot be applied; nothing was changed:\n{problems}"
            ))
        })?;
    Ok(Computed {
        revision: export.revision,
        planned,
    })
}

fn count(changes: usize) -> String {
    if changes == 1 {
        "1 change".to_owned()
    } else {
        format!("{changes} changes")
    }
}

fn check_size(changes: usize) -> Result<(), Failure> {
    if changes > MOST_CHANGES {
        return Err(Failure::Other(format!(
            "the plan has {changes} changes and one request applies at most {MOST_CHANGES}; \
             nothing was changed. Apply a file with a part of the entries first"
        )));
    }
    Ok(())
}

async fn send(
    server: &Server,
    revision: String,
    changes: Vec<StructureChange>,
    out: &mut (dyn Write + Send),
) -> Result<(), Failure> {
    let total = changes.len();
    check_size(total)?;
    let request = ApplyStructureChanges { revision, changes };
    match server.apply(&request).await {
        Ok(applied) if total == 0 => {
            writeln!(
                out,
                "The plan has no changes, and its revision {} is the current one of {}.",
                applied.revision,
                server.origin()
            )?;
            Ok(())
        }
        Ok(applied) => {
            writeln!(
                out,
                "Applied {}. The structure of {} has the revision {}.",
                count(total),
                server.origin(),
                applied.revision
            )?;
            Ok(())
        }
        // Without an answer of the API, the outcome is not known: the request can have been
        // applied before the answer was lost or replaced.
        Err(failure @ (ApiFailure::NoAnswer { .. } | ApiFailure::Unreadable(_))) => {
            Err(Failure::Other(format!(
                "{failure}\nThe tool cannot tell whether the server applied the {}. The \
                 server applies all changes of a request or none: run `plan` again to see \
                 which",
                count(total)
            )))
        }
        Err(failure) => {
            let advice = if needs_new_plan(&failure) {
                "\nThe structure of the server is not what the plan was computed from. Run \
                 `plan` again"
            } else {
                ""
            };
            Err(Failure::Other(format!(
                "{failure}\nThe server applied none of the {}.{advice}",
                count(total)
            )))
        }
    }
}

async fn export(
    invocation: &Invocation,
    options: &Options,
    out: &mut (dyn Write + Send),
    err: &mut (dyn Write + Send),
) -> Result<(), Failure> {
    if !options.positional.is_empty() || options.plan.is_some() {
        return Err(Failure::Usage("export takes no file".to_owned()));
    }
    let server = server(invocation, options)?;
    let export = server.structure().await?;
    let text = pretty(&export.structure)?;
    match &options.out {
        Some(path) => write_file(path, "structure file", &text)?,
        None => out.write_all(text.as_bytes())?,
    }
    writeln!(
        err,
        "Exported the structure of {} at the revision {}.",
        server.origin(),
        export.revision
    )?;
    Ok(())
}

async fn show_plan(
    invocation: &Invocation,
    options: &Options,
    out: &mut (dyn Write + Send),
) -> Result<(), Failure> {
    let [path] = options.positional.as_slice() else {
        return Err(Failure::Usage("plan takes one structure file".to_owned()));
    };
    if options.plan.is_some() {
        return Err(Failure::Usage("plan takes no --plan".to_owned()));
    }
    let server = server(invocation, options)?;
    let computed = compute(&server, path).await?;
    describe(&server, &computed, out)?;
    if let Some(plan_path) = &options.out {
        let plan = PlanFile {
            format: STRUCTURE_FORMAT,
            server: server.origin().to_string(),
            revision: computed.revision.clone(),
            changes: computed
                .planned
                .into_iter()
                .map(|planned| planned.change)
                .collect(),
        };
        write_file(plan_path, "plan file", &pretty(&plan)?)?;
    }
    Ok(())
}

fn describe(
    server: &Server,
    computed: &Computed,
    out: &mut (dyn Write + Send),
) -> Result<(), Failure> {
    if computed.planned.is_empty() {
        writeln!(
            out,
            "No changes: the structure of {} matches the file (revision {}).",
            server.origin(),
            computed.revision
        )?;
        return Ok(());
    }
    writeln!(
        out,
        "Plan for {} at the revision {}: {}",
        server.origin(),
        computed.revision,
        count(computed.planned.len())
    )?;
    for planned in &computed.planned {
        writeln!(out, "  {}", planned.line)?;
    }
    Ok(())
}

async fn apply(
    invocation: &Invocation,
    options: &Options,
    out: &mut (dyn Write + Send),
) -> Result<(), Failure> {
    if options.out.is_some() {
        return Err(Failure::Usage("apply takes no --out".to_owned()));
    }
    match (options.positional.as_slice(), &options.plan) {
        ([path], None) => {
            let server = server(invocation, options)?;
            let computed = compute(&server, path).await?;
            describe(&server, &computed, out)?;
            if computed.planned.is_empty() {
                return Ok(());
            }
            let changes = computed
                .planned
                .into_iter()
                .map(|planned| planned.change)
                .collect();
            send(&server, computed.revision, changes, out).await
        }
        ([], Some(path)) => {
            let server = server(invocation, options)?;
            let text = read_file(path, "plan file")?;
            let plan: PlanFile = serde_json::from_str(&text).map_err(|error| {
                Failure::Other(format!("the plan file {path} is not a plan: {error}"))
            })?;
            if plan.format != STRUCTURE_FORMAT {
                return Err(Failure::Other(format!(
                    "the plan file {path} has the format {}; this tool reads the format \
                     {STRUCTURE_FORMAT}",
                    plan.format
                )));
            }
            if plan.server != server.origin().as_str() {
                return Err(Failure::Other(format!(
                    "the plan file {path} was computed for {} and not for {}; nothing was \
                     changed",
                    plan.server,
                    server.origin()
                )));
            }
            // A plan without changes is sent as well: the server checks its revision.
            send(&server, plan.revision, plan.changes, out).await
        }
        _ => Err(Failure::Usage(
            "apply takes one structure file, or --plan with one plan file".to_owned(),
        )),
    }
}

/// Runs one command and returns the exit code of the process.
pub async fn run(
    invocation: Invocation,
    out: &mut (dyn Write + Send),
    err: &mut (dyn Write + Send),
) -> u8 {
    let Some((command, rest)) = invocation.arguments.split_first() else {
        let _ = writeln!(err, "{USAGE}");
        return USAGE_ERROR;
    };
    if command == "--help" || command == "help" {
        let _ = writeln!(out, "{USAGE}");
        return SUCCESS;
    }
    let outcome = match options(rest) {
        Err(failure) => Err(failure),
        Ok(options) => match command.as_str() {
            "export" => export(&invocation, &options, out, err).await,
            "plan" => show_plan(&invocation, &options, out).await,
            "apply" => apply(&invocation, &options, out).await,
            other => Err(Failure::Usage(format!("unknown command {other}"))),
        },
    };
    match outcome {
        Ok(()) => SUCCESS,
        Err(Failure::Usage(problem)) => {
            let _ = writeln!(err, "{problem}\n\n{USAGE}");
            USAGE_ERROR
        }
        Err(Failure::Other(problem)) => {
            let _ = writeln!(err, "{problem}");
            FAILURE
        }
    }
}
