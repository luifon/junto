//! Offline release creation and the platform update interface.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use base64::{Engine, prelude::BASE64_STANDARD};
use ed25519_dalek::SigningKey;
use junto_client_core::releases::{self, Error, Manifest, TrustedKeys};
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

fn package(
    source: &Path,
    output: &Path,
    keys: &[SigningKey],
    freshness: &SigningKey,
    version: &str,
    system: &str,
    architecture: &str,
) -> Result<(), Error> {
    Version::parse(version).map_err(|_| Error::Invalid("version"))?;
    if !["macos", "linux", "windows"].contains(&system)
        || !["aarch64", "x86_64"].contains(&architecture)
    {
        return Err(Error::Invalid("target"));
    }
    if output.canonicalize()?.starts_with(source.canonicalize()?) {
        return Err(Error::Invalid("output must be outside the archive source"));
    }
    let archive = format!("junto-{version}-{system}-{architecture}.junto");
    let archive_path = output.join(&archive);
    let mut file = new_file(&archive_path)?;
    let result = (|| {
        releases::archive::pack(source, &mut file)?;
        file.sync_all()?;
        let mut manifest = Manifest {
            format: 1,
            version: version.into(),
            system: system.into(),
            architecture: architecture.into(),
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
        let mut metadata = new_file(&output.join(format!("latest-{system}-{architecture}.json")))?;
        metadata.write_all(&wire)?;
        metadata.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(archive_path);
    }
    result
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

async fn execute(args: &[String]) -> Result<(), Error> {
    let (args, test_configuration, trusted) =
        if args.first().map(String::as_str) == Some("--test-configuration") {
            if args.len() < 4 {
                return Err(Error::Invalid(
                    "test configuration requires release and freshness public-key files",
                ));
            }
            let trusted = TrustedKeys::from_json(&fs::read(&args[1])?)?
                .with_freshness(TrustedKeys::from_json(&fs::read(&args[2])?)?);
            (&args[3..], true, trusted)
        } else {
            (args, false, TrustedKeys::compiled()?)
        };
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
            architecture,
            additional @ ..,
        ] => {
            let mut keys = vec![signing_key(Path::new(secret))?];
            for secret in additional {
                keys.push(signing_key(Path::new(secret))?);
            }
            fs::create_dir(output)?;
            let result = package(
                Path::new(source),
                Path::new(output),
                &keys,
                &signing_key(Path::new(freshness))?,
                version,
                system,
                architecture,
            );
            if result.is_err() {
                let _ = fs::remove_dir_all(output);
            }
            result
        }
        [
            "publish",
            bundle,
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
            #[cfg(target_os = "macos")]
            if {
                releases::macos::verify_release_identity(Path::new(bundle))?;
                releases::macos::verify_bundle(Path::new(bundle))?
            } != Version::THIS_BUILD
            {
                return Err(Error::Invalid("bundle and release versions differ"));
            }
            let mut keys = vec![signing_key(Path::new(secret))?];
            for secret in additional {
                keys.push(signing_key(Path::new(secret))?);
            }
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
            let source = Path::new(bundle)
                .parent()
                .ok_or(Error::Invalid("bundle parent"))?;
            package(
                source,
                Path::new(output),
                &keys,
                &freshness,
                &Version::THIS_BUILD.to_string(),
                std::env::consts::OS,
                std::env::consts::ARCH,
            )
        }
        #[cfg(target_os = "macos")]
        ["update", source, public, freshness_public, application] => {
            use releases::{
                HttpSource, Installer,
                macos::{MacInstaller, version},
            };
            let application = Path::new(application);
            let keys = TrustedKeys::from_json(&fs::read(public)?)?
                .with_freshness(TrustedKeys::from_json(&fs::read(freshness_public)?)?);
            let source = HttpSource::new(
                source,
                keys,
                version(application)?,
                "macos",
                std::env::consts::ARCH,
            )?;
            let mut random = [0; 16];
            getrandom::fill(&mut random).map_err(|_| Error::Invalid("random source"))?;
            let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
            let download = application
                .parent()
                .ok_or(Error::Invalid("bundle parent"))?
                .join(format!(".Junto.download-{suffix}"));
            let installer = MacInstaller::new(application)?;
            installer.location()?;
            let mut archive = source.download(&download).await?;
            installer.install(&mut archive)?;
            Ok(())
        }
        #[cfg(target_os = "macos")]
        ["update-compiled", application] => {
            use releases::{
                Installer,
                macos::{MacInstaller, version},
            };
            let application = Path::new(application);
            if version(application)? != Version::THIS_BUILD {
                return Err(Error::Invalid("updater and application versions differ"));
            }
            let source =
                releases::HttpSource::compiled()?.ok_or(Error::Invalid("no build-time source"))?;
            let mut random = [0; 16];
            getrandom::fill(&mut random).map_err(|_| Error::Invalid("random source"))?;
            let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let download = application
                .parent()
                .ok_or(Error::Invalid("bundle parent"))?
                .join(format!(".Junto.download-{suffix}"));
            let installer = MacInstaller::new(application)?;
            installer.location()?;
            let mut archive = source.download(&download).await?;
            installer.install(&mut archive)?;
            Ok(())
        }
        #[cfg(target_os = "macos")]
        ["rollback", application, pin @ ..]
            if pin.len() < 2 && (pin.is_empty() || test_configuration) =>
        {
            let mut installer = releases::macos::MacInstaller::new(Path::new(application))?;
            if let Some(pin) = pin.first() {
                installer = installer.with_trust(trusted, fs::read_to_string(pin)?.trim().into());
            }
            installer.rollback()
        }
        _ => Err(Error::Invalid(
            "usage: junto-release keygen SECRET PUBLIC | package SOURCE NEW-OUTPUT RELEASE-SECRET FRESHNESS-SECRET VERSION SYSTEM ARCH [RELEASE-SECRET...] | publish BUNDLE OUTPUT RELEASE-SECRET FRESHNESS-SECRET [RELEASE-SECRET...] | update SOURCE RELEASE-PUBLIC FRESHNESS-PUBLIC BUNDLE | update-compiled BUNDLE | rollback BUNDLE | refresh METADATA ARCHIVE VERSION FRESHNESS-SECRET...",
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
