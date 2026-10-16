#![allow(unsafe_code)]

use core_foundation::{
    base::TCFType,
    string::{CFString, CFStringRef},
};
use cpal::traits::{DeviceTrait, HostTrait};
use junto_media::audio::{AudioError, DeviceId};
use std::ffi::{OsStr, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use super::admission::Client;

#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyDataSize(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> i32;
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
}

#[link(name = "System")]
unsafe extern "C" {
    fn proc_pidpath(pid: i32, buffer: *mut c_void, size: u32) -> i32;
}

fn address(selector: &[u8; 4], scope: &[u8; 4]) -> Address {
    Address {
        selector: u32::from_be_bytes(*selector),
        scope: u32::from_be_bytes(*scope),
        element: 0,
    }
}
fn status(code: i32, object: u32, address: &Address) -> Result<(), AudioError> {
    if code == 0 {
        Ok(())
    } else {
        Err(AudioError::Failed(format!(
            "CoreAudio object {object} property {:?} scope {:?} status {code}",
            address.selector.to_be_bytes(),
            address.scope.to_be_bytes()
        )))
    }
}

fn numbers(object: u32, selector: &[u8; 4], scope: &[u8; 4]) -> Result<Vec<u32>, AudioError> {
    let address = address(selector, scope);
    let mut size = 0;
    // SAFETY: the addresses and buffers are valid for each synchronous property read.
    unsafe {
        status(
            AudioObjectGetPropertyDataSize(object, &address, 0, std::ptr::null(), &mut size),
            object,
            &address,
        )?;
    }
    if size % 4 != 0 {
        return Err(AudioError::NoDevice);
    }
    if size == 0 {
        return Ok(Vec::new());
    }
    let mut values = vec![0u32; size as usize / 4];
    // SAFETY: the allocation has the size returned by CoreAudio. A changed size fails.
    unsafe {
        status(
            AudioObjectGetPropertyData(
                object,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                values.as_mut_ptr().cast(),
            ),
            object,
            &address,
        )?;
    }
    values.truncate(size as usize / 4);
    Ok(values)
}
fn number(object: u32, selector: &[u8; 4]) -> Result<u32, AudioError> {
    let values = numbers(object, selector, b"glob")?;
    if values.len() != 1 {
        return Err(AudioError::NoDevice);
    }
    Ok(values[0])
}
fn string(object: u32, selector: &[u8; 4]) -> Result<String, AudioError> {
    let address = address(selector, b"glob");
    let mut value: CFStringRef = std::ptr::null();
    let mut size = size_of::<CFStringRef>() as u32;
    // SAFETY: these properties return a retained CFString, released by CFString.
    unsafe {
        status(
            AudioObjectGetPropertyData(
                object,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                (&mut value as *mut CFStringRef).cast(),
            ),
            object,
            &address,
        )?;
        if value.is_null() {
            return Err(AudioError::NoDevice);
        }
        Ok(CFString::wrap_under_create_rule(value).to_string())
    }
}

fn executable(pid: i32) -> Option<PathBuf> {
    let mut buffer = vec![0u8; 4096];
    // SAFETY: the buffer is writable for the length passed to the system.
    let length = unsafe { proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    buffer.truncate(usize::try_from(length).ok().filter(|l| *l > 0)?);
    std::fs::canonicalize(OsStr::from_bytes(&buffer)).ok()
}

/// The CoreAudio client processes with the devices that each uses for input or output.
pub fn clients() -> Result<Vec<Client>, AudioError> {
    let mut clients = Vec::new();
    for process in numbers(1, b"prs#", b"glob")? {
        let pid = number(process, b"ppid").ok().map(|pid| pid as i32);
        let devices = match (
            numbers(process, b"pdv#", b"inpt"),
            numbers(process, b"pdv#", b"outp"),
        ) {
            (Ok(mut input), Ok(output)) => {
                input.extend(output);
                Some(input)
            }
            _ => None,
        };
        clients.push(Client {
            pid,
            executable: pid.and_then(executable),
            devices,
        });
    }
    Ok(clients)
}

/// The CoreAudio object of the one device with the BlackHole identifier.
pub fn object() -> Result<u32, AudioError> {
    let mut found = Vec::new();
    for object in numbers(1, b"dev#", b"glob")? {
        if string(object, b"uid ")? == "BlackHole2ch_UID" {
            found.push(object);
        }
    }
    if found.len() != 1 {
        return Err(AudioError::NoDevice);
    }
    Ok(found[0])
}

/// Refuses when a process other than this runner and its helpers uses BlackHole now.
pub fn exclusive() -> Result<(), Box<dyn std::error::Error>> {
    let own = std::fs::canonicalize(std::env::current_exe()?)?;
    let own = (std::process::id() as i32, own.as_path());
    Ok(super::admission::exclusive(&clients()?, object()?, own)?)
}

pub fn verify() -> Result<DeviceId, AudioError> {
    verify_route(false)
}

pub fn verify_idle() -> Result<DeviceId, AudioError> {
    verify_route(true)
}

fn verify_route(require_idle: bool) -> Result<DeviceId, AudioError> {
    let mut plugins = Vec::new();
    for plugin in numbers(1, b"plg#", b"glob")? {
        if string(plugin, b"piid")? == "audio.existential.BlackHole2ch" {
            plugins.push(plugin);
        }
    }
    if plugins.len() != 1 {
        return Err(AudioError::Refused);
    }
    let owned = numbers(plugins[0], b"dev#", b"glob")?;
    let mut found = Vec::new();
    for object in numbers(1, b"dev#", b"glob")? {
        let uid = string(object, b"uid ")?;
        if uid != "BlackHole2ch_UID" {
            continue;
        }
        if number(object, b"clas")? != u32::from_be_bytes(*b"adev")
            || number(object, b"tran")? != u32::from_be_bytes(*b"virt")
            || number(object, b"livn")? != 1
            || !owned.contains(&object)
        {
            return Err(AudioError::Refused);
        }
        if require_idle && !super::admission::idle(number(object, b"gone").ok()) {
            return Err(AudioError::Refused);
        }
        let host = cpal::default_host();
        let mut inputs = host
            .input_devices()
            .map_err(|e| AudioError::Failed(e.to_string()))?
            .filter(|d| d.id().is_ok_and(|id| id.id() == uid));
        let mut outputs = host
            .output_devices()
            .map_err(|e| AudioError::Failed(e.to_string()))?
            .filter(|d| d.id().is_ok_and(|id| id.id() == uid));
        let input = inputs.next().ok_or(AudioError::NoDevice)?;
        let output = outputs.next().ok_or(AudioError::NoDevice)?;
        if inputs.next().is_some() || outputs.next().is_some() {
            return Err(AudioError::Refused);
        }
        for format in [input.default_input_config(), output.default_output_config()] {
            let format = format.map_err(|e| AudioError::Failed(e.to_string()))?;
            if format.channels() != 2 || !junto_media::audio::rate::converts(format.sample_rate()) {
                return Err(AudioError::Unsupported(format!(
                    "{} Hz, {} channels",
                    format.sample_rate(),
                    format.channels()
                )));
            }
        }
        let id = output.id().map_err(|e| AudioError::Failed(e.to_string()))?;
        if input.id().map_err(|e| AudioError::Failed(e.to_string()))? != id {
            return Err(AudioError::Refused);
        }
        found.push(DeviceId(id.to_string()));
    }
    if found.len() != 1 {
        return Err(AudioError::NoDevice);
    }
    Ok(found.remove(0))
}
