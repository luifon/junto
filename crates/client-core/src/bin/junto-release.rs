//! Release creation on the build machine, and the update command of the release checks.
//! The installed packages carry `junto-recover` instead (ADR 0726).

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use base64::{Engine, prelude::BASE64_STANDARD};
use ed25519_dalek::SigningKey;
use junto_client_core::releases::install::{Package, Previous};
use junto_client_core::releases::{
    self, Error, HttpSource, Manifest, PlatformInstaller, Target, TrustedKeys,
};
use junto_client_core::updates::Version;
use zeroize::Zeroizing;

fn external(path: &Path) -> Result<PathBuf, Error> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = parent.canonicalize()?;
    if parent
        .ancestors()
        .any(|parent| parent.join(".git").exists())
    {
        return Err(Error::Invalid("private keys must be outside repositories"));
    }
    Ok(parent.join(path.file_name().ok_or(Error::Invalid("key file name"))?))
}

fn new_file(path: &Path) -> Result<File, Error> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

fn keygen(secret: &Path, public: &Path) -> Result<(), Error> {
    let secret = external(secret)?;
    if public.exists() {
        return Err(Error::Invalid("public key destination exists"));
    }
    let mut bytes = Zeroizing::new([0; 32]);
    getrandom::fill(bytes.as_mut_slice()).map_err(|_| Error::Invalid("random source"))?;
    let key = SigningKey::from_bytes(&bytes);
    let mut file = new_file(&secret)?;
    let result = (|| {
        file.write_all(bytes.as_slice())?;
        file.sync_all()?;
        let bytes = serde_json::to_vec(&[BASE64_STANDARD.encode(key.verifying_key().as_bytes())])?;
        let mut file = new_file(public)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(secret);
    }
    result
}

fn signing_key(path: &Path) -> Result<SigningKey, Error> {
    let path = external(path)?;
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_file() {
        return Err(Error::Invalid("key file type"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::Invalid("private key permissions"));
        }
    }
    let mut bytes = Zeroizing::new(Vec::new());
    File::open(path)?.take(33).read_to_end(&mut bytes)?;
    let seed: &[u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| Error::Invalid("key length"))?;
    Ok(SigningKey::from_bytes(seed))
}

#[allow(clippy::too_many_arguments)]
fn package(
    source: &Path,
    output: &Path,
    keys: &[SigningKey],
    freshness: &SigningKey,
    version: &str,
    system: &str,
    package: &str,
    architecture: &str,
) -> Result<(), Error> {
    let version = Version::parse(version).map_err(|_| Error::Invalid("version"))?;
    if !["macos", "linux", "windows"].contains(&system)
        || Package::parse(package).is_none()
        || !["aarch64", "x86_64"].contains(&architecture)
    {
        return Err(Error::Invalid("target"));
    }
    let target = Target::new(system, package, architecture)?;
    if output.canonicalize()?.starts_with(source.canonicalize()?) {
        return Err(Error::Invalid("output must be outside the archive source"));
    }
    let archive = target.archive(version);
    let archive_path = output.join(&archive);
    let mut file = new_file(&archive_path)?;
    let result = (|| {
        releases::archive::pack(source, &mut file)?;
        file.sync_all()?;
        let mut manifest = Manifest {
            format: 1,
            version: version.to_string(),
            system: target.system.clone(),
            package: target.package.clone(),
            architecture: target.architecture.clone(),
            archive,
            size: 0,
            sha256: String::new(),
            expires: releases::expires_in_thirty_days()?,
            archive_signatures: Vec::new(),
        };
        for key in keys {
            releases::sign_archive(&archive_path, &mut manifest, key)?;
        }
        let wire = releases::sign_manifest_many(
            &manifest,
            &keys.iter().collect::<Vec<_>>(),
            &[freshness],
        )?;
        // The source keeps the information of every version: an update downloads the
        // installed version from it when the installation holds no copy (ADR 0726).
        for name in [target.latest(), target.release(version)] {
            let mut metadata = new_file(&output.join(name))?;
            metadata.write_all(&wire)?;
            metadata.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(archive_path);
    }
    result
}

/// A macOS bundle is published only from macOS, where its code signature can be read: it must
/// carry the pinned project certificate and the version of this tool.
fn verify_bundle(bundle: &Path) -> Result<(), Error> {
    #[cfg(target_os = "macos")]
    {
        releases::macos::verify_release_identity(bundle)?;
        if releases::macos::verify_bundle(bundle)? != Version::THIS_BUILD {
            return Err(Error::Invalid("bundle and release versions differ"));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = bundle;
        Err(Error::Invalid("a macOS bundle is published on macOS"))
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let result = execute(&args).await;
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn signing_keys(first: &str, additional: &[&str]) -> Result<Vec<SigningKey>, Error> {
    std::iter::once(&first)
        .chain(additional)
        .map(|path| signing_key(Path::new(path)))
        .collect()
}

async fn update(source: &str, application: &str) -> Result<(), Error> {
    let installer = PlatformInstaller::at(Path::new(application))?;
    installer.location()?;
    let current = installer.installed_version()?;
    let source = HttpSource::new(
        source,
        TrustedKeys::compiled()?,
        current,
        installer.target().clone(),
    )?;
    let installed = releases::update(installer, &source).await?;
    println!("installed version {}", installed.version);
    match installed.previous {
        Previous::Kept(version) => println!("version {version} is kept for a rollback"),
        Previous::Missing(reason) => println!("no previous version is kept: {reason}"),
    }
    Ok(())
}

async fn execute(args: &[String]) -> Result<(), Error> {
    // The flag states that the person knows that this build is a test configuration. A
    // product build refuses it; the trusted keys are always the compiled ones.
    let (args, test_configuration) = match args.first().map(String::as_str) {
        Some("--test-configuration") => (&args[1..], true),
        _ => (args, false),
    };
    if test_configuration && !releases::TEST_CONFIGURATION {
        return Err(Error::Invalid(
            "--test-configuration is refused by a product build",
        ));
    }
    let trusted = TrustedKeys::compiled()?;
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["refresh", metadata, archive, version, secrets @ ..] if !secrets.is_empty() => {
            let keys = secrets
                .iter()
                .map(|path| signing_key(Path::new(path)))
                .collect::<Result<Vec<_>, _>>()?;
            let version =
                Version::parse(version).map_err(|_| Error::Invalid("version to refresh"))?;
            let wire = releases::refresh_release(
                &fs::read(metadata)?,
                Path::new(archive),
                version,
                &trusted,
                &keys.iter().collect::<Vec<_>>(),
            )?;
            releases::write_atomic(Path::new(metadata), &wire)
        }
        ["keygen", secret, public] => keygen(Path::new(secret), Path::new(public)),
        [
            "package",
            source,
            output,
            secret,
            freshness,
            version,
            system,
            package_name,
            architecture,
            additional @ ..,
        ] => {
            let keys = signing_keys(secret, additional)?;
            fs::create_dir(output)?;
            let result = package(
                Path::new(source),
                Path::new(output),
                &keys,
                &signing_key(Path::new(freshness))?,
                version,
                system,
                package_name,
                architecture,
            );
            if result.is_err() {
                let _ = fs::remove_dir_all(output);
            }
            result
        }
        [
            "publish",
            system,
            package_name,
            architecture,
            source,
            output,
            secret,
            freshness,
            additional @ ..,
        ] => {
            junto_client_core::release_configuration::publication(
                releases::TEST_CONFIGURATION,
                test_configuration,
            )
            .map_err(Error::Invalid)?;
            if trusted.is_empty() {
                return Err(Error::Invalid(
                    "release and freshness trust sets must both be configured",
                ));
            }
            releases::HttpSource::compiled()?
                .ok_or(Error::Invalid("no build-time update source"))?;
            Package::parse(package_name).ok_or(Error::Invalid("package"))?;
            let target = Target::new(system, package_name, architecture)?;
            let source = Path::new(source);
            // The release is of this checkout: the package states the version of this tool.
            releases::install::verify_source(&target, source, Version::THIS_BUILD)?;
            if target.system == "macos" {
                verify_bundle(&source.join("Junto.app"))?;
            }
            let keys = signing_keys(secret, additional)?;
            if keys
                .iter()
                .any(|key| !trusted.contains(&key.verifying_key()))
            {
                return Err(Error::Invalid(
                    "the signing key is not in the compiled trust set",
                ));
            }
            let freshness = signing_key(Path::new(freshness))?;
            if !trusted.contains_freshness(&freshness.verifying_key()) {
                return Err(Error::Invalid(
                    "the freshness key is not in the compiled trust set",
                ));
            }
            package(
                source,
                Path::new(output),
                &keys,
                &freshness,
                &Version::THIS_BUILD.to_string(),
                &target.system,
                &target.package,
                &target.architecture,
            )
        }
        ["update", source, application] => update(source, application).await,
        ["update-compiled", application] => {
            let source = option_env!("JUNTO_UPDATE_SOURCE")
                .filter(|source| !source.is_empty())
                .ok_or(Error::Invalid("no build-time source"))?;
            update(source, application).await
        }
        _ => Err(Error::Invalid(
            "usage: junto-release [--test-configuration] keygen SECRET PUBLIC | package SOURCE NEW-OUTPUT RELEASE-SECRET FRESHNESS-SECRET VERSION SYSTEM PACKAGE ARCH [RELEASE-SECRET...] | publish SYSTEM PACKAGE ARCH SOURCE OUTPUT RELEASE-SECRET FRESHNESS-SECRET [RELEASE-SECRET...] | update SOURCE APPLICATION | update-compiled APPLICATION | refresh METADATA ARCHIVE VERSION FRESHNESS-SECRET...",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_key_paths_inside_a_repository_are_refused_before_any_creation() {
        let existing = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(matches!(external(&existing), Err(Error::Invalid(_))));
    }
}
