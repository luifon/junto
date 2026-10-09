use std::env;
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};
use objc2_foundation::NSBundle;
use security_framework::base::Error as SecError;
use security_framework::os::macos::keychain::SecKeychain;
use security_framework::passwords::{
    PasswordOptions, delete_generic_password, delete_generic_password_options, generic_password,
    get_generic_password, set_generic_password, set_generic_password_options,
};

const VERSION: &str = env!("SPIKE_VERSION");
const ACCOUNT: &str = "device-key";
const KEY_FILE_NAME: &str = "device.key";
const PERMISSION_ANSWER_TIMEOUT: Duration = Duration::from_secs(180);

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    safe fn CGPreflightScreenCaptureAccess() -> bool;
    safe fn CGRequestScreenCaptureAccess() -> bool;
}

enum Value {
    Text(String),
    Flag(bool),
    Number(i64),
}

struct Report {
    fields: Vec<(&'static str, Value)>,
}

impl Report {
    fn new(mode: &str) -> Self {
        let mut report = Self { fields: Vec::new() };
        report.text("mode", mode);
        report.text("version", VERSION);
        report.text("bundle_id", &bundle_identifier());
        report.text("executable", &home_relative(env::current_exe().ok()));
        report.number("pid", i64::from(std::process::id()));
        report.number("parent_pid", i64::from(std::os::unix::process::parent_id()));
        report
    }

    fn text(&mut self, key: &'static str, value: &str) {
        self.fields.push((key, Value::Text(value.to_string())));
    }

    fn flag(&mut self, key: &'static str, value: bool) {
        self.fields.push((key, Value::Flag(value)));
    }

    fn number(&mut self, key: &'static str, value: i64) {
        self.fields.push((key, Value::Number(value)));
    }

    fn security_result<T>(&mut self, result: &Result<T, SecError>) {
        match result {
            Ok(_) => {
                self.flag("ok", true);
                self.number("os_status", 0);
            }
            Err(error) => {
                self.flag("ok", false);
                self.number("os_status", i64::from(error.code()));
                self.text("os_message", &error.message().unwrap_or_default());
            }
        }
    }

    fn to_json(&self) -> String {
        let body: Vec<String> = self
            .fields
            .iter()
            .map(|(key, value)| {
                let rendered = match value {
                    Value::Text(text) => format!("\"{}\"", escape_json(text)),
                    Value::Flag(flag) => flag.to_string(),
                    Value::Number(number) => number.to_string(),
                };
                format!("\"{key}\":{rendered}")
            })
            .collect();
        format!("{{{}}}", body.join(","))
    }
}

fn escape_json(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            control if (control as u32) < 0x20 => {
                escaped.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => escaped.push(other),
        }
    }
    escaped
}

fn bundle_identifier() -> String {
    NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|identifier| identifier.to_string())
        .unwrap_or_default()
}

fn home_relative(path: Option<PathBuf>) -> String {
    let Some(path) = path else {
        return String::new();
    };
    let shown = path.display().to_string();
    match env::var("HOME") {
        Ok(home) if shown.starts_with(&home) => format!("~{}", &shown[home.len()..]),
        _ => shown,
    }
}

struct Arguments {
    mode: String,
    out: Option<PathBuf>,
    secret: Option<String>,
    service: Option<String>,
}

fn parse_arguments() -> Arguments {
    let mut arguments = Arguments {
        mode: String::new(),
        out: None,
        secret: None,
        service: None,
    };
    let mut raw = env::args().skip(1);
    while let Some(argument) = raw.next() {
        match argument.as_str() {
            "--out" => arguments.out = raw.next().map(PathBuf::from),
            "--secret" => arguments.secret = raw.next(),
            "--service" => arguments.service = raw.next(),
            mode if arguments.mode.is_empty() => arguments.mode = mode.to_string(),
            _ => {}
        }
    }
    arguments
}

fn data_protection_options(service: &str) -> PasswordOptions {
    let mut options = PasswordOptions::new_generic_password(service, ACCOUNT);
    options.use_protected_keychain();
    options
}

fn report_secret(report: &mut Report, result: Result<Vec<u8>, SecError>) {
    report.security_result(&result);
    if let Ok(bytes) = result {
        report.text("value", &String::from_utf8_lossy(&bytes));
    }
}

fn microphone_status() -> i64 {
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return -1;
    };
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) };
    status.0 as i64
}

fn microphone_status_name(status: i64) -> &'static str {
    match status {
        0 => "not_determined",
        1 => "restricted",
        2 => "denied",
        3 => "authorized",
        _ => "unknown",
    }
}

fn request_microphone() -> Option<bool> {
    let audio = unsafe { AVMediaTypeAudio }?;
    let (sender, receiver) = mpsc::channel();
    let handler = RcBlock::new(move |granted: Bool| {
        let _ = sender.send(granted.as_bool());
    });
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &handler) };
    receiver.recv_timeout(PERMISSION_ANSWER_TIMEOUT).ok()
}

fn key_file_path() -> Option<PathBuf> {
    let home = env::var("HOME").ok()?;
    Some(
        PathBuf::from(home)
            .join("Library/Application Support")
            .join(bundle_identifier())
            .join(KEY_FILE_NAME),
    )
}

fn store_key_file(secret: &str) -> std::io::Result<PathBuf> {
    let path = key_file_path().ok_or_else(|| std::io::Error::other("no home directory"))?;
    if let Some(directory) = path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    file.write_all(secret.as_bytes())?;
    Ok(path)
}

fn report_key_file(report: &mut Report) {
    let Some(path) = key_file_path() else {
        report.flag("ok", false);
        return;
    };
    report.text("path", &home_relative(Some(path.clone())));
    match (fs::read_to_string(&path), fs::metadata(&path)) {
        (Ok(value), Ok(metadata)) => {
            report.flag("ok", true);
            report.text("value", &value);
            report.text("file_mode", &format!("{:o}", metadata.mode() & 0o7777));
            report.flag("owned_by_current_user", metadata.uid() == current_user_id());
            if let Some(directory) = path.parent().and_then(|parent| fs::metadata(parent).ok()) {
                report.text("directory_mode", &format!("{:o}", directory.mode() & 0o7777));
            }
        }
        (Err(error), _) | (_, Err(error)) => {
            report.flag("ok", false);
            report.text("error", &error.to_string());
        }
    }
}

fn current_user_id() -> u32 {
    fs::metadata(env::var("HOME").unwrap_or_default())
        .map(|metadata| metadata.uid())
        .unwrap_or(u32::MAX)
}

fn run(arguments: &Arguments) -> Report {
    let mut report = Report::new(&arguments.mode);
    let service = arguments
        .service
        .clone()
        .unwrap_or_else(bundle_identifier);
    let secret = arguments.secret.clone().unwrap_or_default();
    let started = Instant::now();
    match arguments.mode.as_str() {
        "version" => report.flag("ok", true),
        "keychain-store" => {
            report.text("service", &service);
            report.security_result(&set_generic_password(&service, ACCOUNT, secret.as_bytes()));
        }
        "keychain-read" => {
            report.text("service", &service);
            report_secret(&mut report, get_generic_password(&service, ACCOUNT));
        }
        "keychain-read-quiet" => {
            report.text("service", &service);
            match SecKeychain::disable_user_interaction() {
                Ok(_interaction_disabled) => {
                    report_secret(&mut report, get_generic_password(&service, ACCOUNT));
                }
                Err(error) => report.security_result::<()>(&Err(error)),
            }
        }
        "keychain-delete" => {
            report.text("service", &service);
            report.security_result(&delete_generic_password(&service, ACCOUNT));
        }
        "dp-store" => {
            report.text("service", &service);
            report.security_result(&set_generic_password_options(
                secret.as_bytes(),
                data_protection_options(&service),
            ));
        }
        "dp-read" => {
            report.text("service", &service);
            report_secret(&mut report, generic_password(data_protection_options(&service)));
        }
        "dp-delete" => {
            report.text("service", &service);
            report.security_result(&delete_generic_password_options(data_protection_options(
                &service,
            )));
        }
        "mic-status" => {
            let status = microphone_status();
            report.flag("ok", true);
            report.number("microphone_status", status);
            report.text("microphone", microphone_status_name(status));
        }
        "mic-request" => {
            let before = microphone_status();
            let answer = request_microphone();
            let after = microphone_status();
            report.flag("ok", answer.is_some());
            report.text("microphone_before", microphone_status_name(before));
            report.text(
                "request_answer",
                match answer {
                    Some(true) => "granted",
                    Some(false) => "refused",
                    None => "no_answer",
                },
            );
            report.text("microphone", microphone_status_name(after));
        }
        "screen-status" => {
            report.flag("ok", true);
            report.flag("screen_recording", CGPreflightScreenCaptureAccess());
        }
        "screen-request" => {
            let before = CGPreflightScreenCaptureAccess();
            let returned = CGRequestScreenCaptureAccess();
            report.flag("ok", true);
            report.flag("screen_recording_before", before);
            report.flag("request_returned", returned);
            report.flag("screen_recording", CGPreflightScreenCaptureAccess());
        }
        "file-store" => match store_key_file(&secret) {
            Ok(path) => {
                report.flag("ok", true);
                report.text("path", &home_relative(Some(path)));
            }
            Err(error) => {
                report.flag("ok", false);
                report.text("error", &error.to_string());
            }
        },
        "file-read" => report_key_file(&mut report),
        "file-delete" => {
            let removed = key_file_path()
                .map(|path| {
                    let file_removed = fs::remove_file(&path).is_ok();
                    let directory_removed = path
                        .parent()
                        .map(|directory| fs::remove_dir(directory).is_ok())
                        .unwrap_or(false);
                    file_removed && directory_removed
                })
                .unwrap_or(false);
            report.flag("ok", removed);
        }
        _ => {
            report.flag("ok", false);
            report.text("error", "unknown mode");
        }
    }
    report.number("elapsed_ms", started.elapsed().as_millis() as i64);
    report
}

fn main() {
    let arguments = parse_arguments();
    let line = run(&arguments).to_json();
    println!("{line}");
    if let Some(out) = &arguments.out {
        let temporary = out.with_extension("partial");
        if fs::write(&temporary, format!("{line}\n")).is_ok() {
            let _ = fs::rename(&temporary, out);
        }
    }
}
