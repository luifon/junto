//! Writes one icon file of the packages into a new file (ADR 0756).

use std::io::Write;
use std::process::ExitCode;

const USAGE: &str = "usage: junto-icon png <size> <new file>
       junto-icon icns <new file>
       junto-icon ico <new file>
       junto-icon windows-resource <new file>
       junto-icon tray <state> <mention|none> <template|colour> <size> <new file>";

fn contents(arguments: &[String]) -> Result<(Vec<u8>, &str), String> {
    let error = |error: junto_icon::Error| error.to_string();
    match arguments {
        [kind, size, file] if kind == "png" => {
            let size = size
                .parse()
                .map_err(|_| format!("the size is a number of pixels, not {size}"))?;
            Ok((junto_icon::png(size).map_err(error)?, file))
        }
        [kind, file] if kind == "icns" => Ok((junto_icon::icns().map_err(error)?, file)),
        [kind, file] if kind == "ico" => Ok((junto_icon::ico().map_err(error)?, file)),
        [kind, file] if kind == "windows-resource" => {
            Ok((junto_icon::windows_resource().map_err(error)?, file))
        }
        [kind, state, mention, style, size, file] if kind == "tray" => {
            let state = junto_icon::TrayState::ALL
                .into_iter()
                .find(|known| known.name() == state)
                .ok_or_else(|| format!("no tray state {state}"))?;
            let mention = match mention.as_str() {
                "mention" => true,
                "none" => false,
                other => return Err(format!("mention or none, not {other}")),
            };
            let style = match style.as_str() {
                "template" => junto_icon::TrayStyle::Template,
                "colour" => junto_icon::TrayStyle::Colour,
                other => return Err(format!("template or colour, not {other}")),
            };
            let size = size
                .parse()
                .map_err(|_| format!("the size is a number of pixels, not {size}"))?;
            Ok((
                junto_icon::tray_png(state, mention, style, size).map_err(error)?,
                file,
            ))
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let written = contents(&arguments).and_then(|(bytes, file)| {
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(file)
            .map_err(|error| format!("{file}: {error}"))?;
        output.write_all(&bytes).map_err(|error| {
            // A file that holds part of the icon is not left behind.
            let _ = std::fs::remove_file(file);
            format!("{file}: {error}")
        })
    });
    match written {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
