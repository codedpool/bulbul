// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! Getting the speech worker program onto the user's computer, safely.
//!
//! The worker is not shipped inside Bulbul's installer; it is downloaded the
//! first time someone chooses an on-device model. Because it's a program the
//! app will run, it is held to a higher standard than a model file:
//!
//! 1. A small **manifest** lists the worker for each platform with its hashes.
//!    The manifest is **signed** (minisign) and checked against a public key
//!    built into the app, so nothing the download host serves can change what
//!    gets installed.
//! 2. The download is checked against the manifest's hash, unpacked, and the
//!    result checked against a second hash.
//! 3. It is run once with `--version` and must report the protocol version this
//!    app speaks. Only then does it replace any older copy.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::catalog::RemoteFile;
use crate::download::{hex, CancelToken, HashWriter, Installer, Phase, Progress, ProgressFn};
use crate::error::{Error, Result};
use crate::protocol::PROTOCOL_VERSION;

pub const MANIFEST_SCHEMA: u32 = 1;

/// Progress for engine installs is reported under this id.
pub const ENGINE_ID: &str = "engine";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineManifest {
    pub schema: u32,
    /// Version of the worker program, e.g. `0.1.0`.
    pub version: String,
    /// The protocol version the worker speaks; must equal ours.
    pub protocol: u32,
    /// Per platform, keyed like `windows-x86_64` (see [`platform_key`]).
    pub assets: BTreeMap<String, EngineAsset>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineAsset {
    /// Where to download the gzip-compressed worker.
    pub url: String,
    /// Size and sha256 of that download.
    pub size: u64,
    pub sha256: String,
    /// Size and sha256 of the worker program once unpacked.
    pub exe_size: u64,
    pub exe_sha256: String,
}

impl EngineManifest {
    pub fn parse(bytes: &[u8]) -> Result<EngineManifest> {
        let m: EngineManifest = serde_json::from_slice(bytes)
            .map_err(|e| Error::InvalidCatalog(format!("the engine manifest isn't valid: {e}")))?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(Error::InvalidCatalog(format!("engine manifest: {m}")));
        if self.schema != MANIFEST_SCHEMA {
            return bad(format!("schema {} isn't supported", self.schema));
        }
        if self.version.trim().is_empty() {
            return bad("no version".into());
        }
        if self.assets.is_empty() {
            return bad("no assets".into());
        }
        for (platform, a) in &self.assets {
            if !a.url.starts_with("https://") {
                return bad(format!("{platform}: the URL must be https"));
            }
            if a.size == 0 || a.exe_size == 0 {
                return bad(format!("{platform}: sizes must be set"));
            }
            if !is_sha256(&a.sha256) || !is_sha256(&a.exe_sha256) {
                return bad(format!("{platform}: bad sha256"));
            }
        }
        Ok(())
    }

    pub fn asset_for(&self, platform: &str) -> Option<&EngineAsset> {
        self.assets.get(platform)
    }
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Which build of the worker this computer needs, e.g. `windows-x86_64`.
pub fn platform_key() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "windows-x86_64",
        ("windows", "aarch64") => "windows-aarch64",
        ("macos", "aarch64") => "macos-aarch64",
        ("macos", "x86_64") => "macos-x86_64",
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        _ => return None,
    })
}

/// File name of the worker program on this platform.
pub fn worker_file_name() -> &'static str {
    if cfg!(windows) {
        "bulbul-stt-worker.exe"
    } else {
        "bulbul-stt-worker"
    }
}

/// Checks a minisign signature over `manifest`. Both the public key and the
/// signature are the base64-wrapped minisign text files the Tauri updater
/// uses, so the same key that signs app updates can sign the engine manifest.
pub fn verify_manifest(manifest: &[u8], signature_b64: &str, public_key_b64: &str) -> Result<()> {
    let text = |b64: &str, what: &str| -> Result<String> {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|_| Error::Engine(format!("the {what} isn't valid base64")))?;
        String::from_utf8(raw).map_err(|_| Error::Engine(format!("the {what} isn't text")))
    };
    let key = minisign_verify::PublicKey::decode(&text(public_key_b64, "public key")?)
        .map_err(|e| Error::Engine(format!("the public key is unusable: {e}")))?;
    let sig = minisign_verify::Signature::decode(&text(signature_b64, "signature")?)
        .map_err(|e| Error::Engine(format!("the signature is unusable: {e}")))?;
    key.verify(manifest, &sig, false)
        .map_err(|_| Error::Engine("the engine manifest's signature doesn't match, so it was rejected".into()))
}

/// Reads the protocol version out of `bulbul-stt-worker --version` output.
pub fn parse_version_output(out: &str) -> Option<u32> {
    let mut words = out.split_whitespace();
    while let Some(w) = words.next() {
        if w == "protocol" {
            return words.next()?.parse().ok();
        }
    }
    None
}

/// Runs the worker with `--version` and confirms it speaks our protocol.
pub async fn check_worker_version(exe: &Path) -> Result<()> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("--version").env_clear().kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = tokio::time::timeout(std::time::Duration::from_secs(15), cmd.output())
        .await
        .map_err(|_| Error::Timeout("the new speech worker didn't answer --version".into()))?
        .map_err(|e| Error::WorkerGone(format!("couldn't run the new speech worker: {e}")))?;
    if !out.status.success() {
        return Err(Error::WorkerGone("the new speech worker exited with an error".into()));
    }
    match parse_version_output(&String::from_utf8_lossy(&out.stdout)) {
        Some(v) if v == PROTOCOL_VERSION => Ok(()),
        Some(v) => Err(Error::Engine(format!(
            "the downloaded speech worker speaks protocol {v}, this app speaks {PROTOCOL_VERSION}"
        ))),
        None => Err(Error::Engine("the downloaded speech worker didn't report its version".into())),
    }
}

impl Installer {
    /// Installs the worker described by a **verified** `manifest` into
    /// `dest_dir`, replacing any previous copy. Returns the program's path.
    ///
    /// The caller must have run [`verify_manifest`] on the manifest's bytes
    /// first; this function trusts what the manifest says.
    pub async fn install_engine(
        &self,
        manifest: &EngineManifest,
        dest_dir: &Path,
        cancel: &CancelToken,
        on_progress: ProgressFn,
    ) -> Result<PathBuf> {
        if manifest.protocol != PROTOCOL_VERSION {
            return Err(Error::Engine(format!(
                "the available speech engine speaks protocol {}, this app speaks {PROTOCOL_VERSION}; update Bulbul to use on-device models",
                manifest.protocol
            )));
        }
        let key = platform_key().ok_or_else(|| Error::Engine("on-device models aren't supported on this platform".into()))?;
        let asset = manifest
            .asset_for(key)
            .ok_or_else(|| Error::Engine(format!("there is no speech engine for {key} yet")))?
            .clone();

        let parent = dest_dir.parent().ok_or_else(|| Error::Engine("the engine folder has no parent".into()))?;
        let work = parent.join(".engine-work");
        std::fs::create_dir_all(&work).map_err(|e| Error::io(&work, e))?;
        self.ensure_space(&work, asset.size + asset.exe_size)?;

        // 1. Download the compressed worker (resumable, hash-checked).
        let gz = work.join("engine.gz");
        let listed = RemoteFile { path: "engine.gz".into(), size: asset.size, sha256: asset.sha256.clone() };
        self.download_verified(&asset.url, &gz, &listed, cancel, ENGINE_ID, on_progress.clone()).await?;

        // 2. Unpack it, checking the result against the manifest.
        let staged_dir = work.join("new");
        let _ = std::fs::remove_dir_all(&staged_dir);
        std::fs::create_dir_all(&staged_dir).map_err(|e| Error::io(&staged_dir, e))?;
        let exe = staged_dir.join(worker_file_name());
        {
            let (gz, exe, asset, cancel) = (gz.clone(), exe.clone(), asset.clone(), cancel.clone());
            let progress = on_progress.clone();
            tokio::task::spawn_blocking(move || unpack_gz(&gz, &exe, &asset, &cancel, &progress))
                .await
                .map_err(|e| Error::Archive(e.to_string()))??;
        }

        // 3. It has to run, and speak our protocol.
        if !self.skip_smoke_test {
            check_worker_version(&exe).await?;
        }

        // 4. Swap it in. The old copy is only removed once the new one is in place.
        let old = parent.join(".engine-old");
        let _ = std::fs::remove_dir_all(&old);
        if dest_dir.exists() {
            std::fs::rename(dest_dir, &old).map_err(|e| Error::io(dest_dir, e))?;
        }
        if let Err(e) = std::fs::rename(&staged_dir, dest_dir) {
            // Put the old one back rather than leaving nothing.
            if old.exists() {
                let _ = std::fs::rename(&old, dest_dir);
            }
            return Err(Error::io(dest_dir, e));
        }
        let _ = std::fs::remove_dir_all(&old);
        let _ = std::fs::remove_dir_all(&work);

        on_progress(Progress { model_id: ENGINE_ID.into(), phase: Phase::Done, bytes_done: asset.exe_size, bytes_total: asset.exe_size });
        Ok(dest_dir.join(worker_file_name()))
    }
}

fn unpack_gz(gz: &Path, exe: &Path, asset: &EngineAsset, cancel: &CancelToken, progress: &ProgressFn) -> Result<()> {
    let input = std::fs::File::open(gz).map_err(|e| Error::io(gz, e))?;
    let mut decoder = flate2::read::GzDecoder::new(input);
    let out = std::fs::File::create(exe).map_err(|e| Error::io(exe, e))?;
    let mut w = HashWriter { inner: out, hasher: sha2::Sha256::new(), written: 0 };

    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let n = std::io::Read::read(&mut decoder, &mut buf).map_err(|e| Error::Archive(e.to_string()))?;
        if n == 0 {
            break;
        }
        // A hostile download can't unpack into more than the manifest said.
        if w.written + n as u64 > asset.exe_size {
            return Err(Error::Archive("the engine unpacked larger than expected".into()));
        }
        w.write_all(&buf[..n]).map_err(|e| Error::io(exe, e))?;
        progress(Progress {
            model_id: ENGINE_ID.into(),
            phase: Phase::Unpacking,
            bytes_done: w.written,
            bytes_total: asset.exe_size,
        });
    }
    w.flush().map_err(|e| Error::io(exe, e))?;
    if w.written != asset.exe_size || hex(&w.hasher.finalize()) != asset.exe_sha256 {
        let _ = std::fs::remove_file(exe);
        return Err(Error::HashMismatch { file: "the speech engine".into() });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(exe, std::fs::Permissions::from_mode(0o755)).map_err(|e| Error::io(exe, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::test_server::{serve, Behaviour};
    use flate2::write::GzEncoder;
    use sha2::{Digest, Sha256};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    // Real vectors made with `tauri signer`: a throwaway key signing MESSAGE.
    const MESSAGE: &[u8] = b"bulbul engine manifest test message\n";
    const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDMwMzgyQ0U4ODUyMDM4NzUKUldSMU9DQ0Y2Q3c0TUtsU3JLSXBVaEwwVTNEZjJCK1J0Y3JpaW92OVRLRnVQeWttbUdZRTZzWGUK";
    const SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVSMU9DQ0Y2Q3c0TU0rZG54ajBYdmVoNGFzdFdLNlVyUlBKS0I5SmxDWEtkcUhqUHdmT05yc2JOSDRDUkI2b01Nd0l6Y2poQlFyTnBQV2pxdFlZdTlPVnQvZmlUaGRKWndnPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0NTU2MzQ4CWZpbGU6bXNnLnR4dApEY3gzK2hOMjFPdDFKMlIwR0NwWnRFMFZJNnNIZHhhRkhGRXRHRHlvRFJFYnY4UEJHMUYzWEFBSWlTcXh3cXFvN2hiQ0ZPVWhwS0U3aStpN21UUjBDUT09Cg==";
    /// The app updater's real public key (from tauri.conf.json): a different key.
    const OTHER_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDRCMEQ1M0FFRUNEQkJEQ0IKUldUTHZkdnNybE1OUzRMUXZzS08wM1Q4a0YrNWpaMXM3S2l5VTRsS1ptWVBjZDArMXF4bTJnS3QK";

    #[test]
    fn a_genuine_signature_verifies() {
        verify_manifest(MESSAGE, SIGNATURE, PUBKEY).unwrap();
    }

    #[test]
    fn a_changed_manifest_is_rejected() {
        let mut tampered = MESSAGE.to_vec();
        tampered[0] ^= 1;
        let err = verify_manifest(&tampered, SIGNATURE, PUBKEY).unwrap_err().to_string();
        assert!(err.contains("signature doesn't match"), "{err}");
    }

    #[test]
    fn a_signature_from_another_key_is_rejected() {
        assert!(verify_manifest(MESSAGE, SIGNATURE, OTHER_PUBKEY).is_err());
    }

    #[test]
    fn garbage_keys_and_signatures_are_errors_not_panics() {
        assert!(verify_manifest(MESSAGE, "not base64!!", PUBKEY).is_err());
        assert!(verify_manifest(MESSAGE, SIGNATURE, "not base64!!").is_err());
        assert!(verify_manifest(MESSAGE, "aGVsbG8=", PUBKEY).is_err()); // valid base64, not a signature
        assert!(verify_manifest(MESSAGE, SIGNATURE, "aGVsbG8=").is_err());
        assert!(verify_manifest(MESSAGE, "", PUBKEY).is_err());
    }

    fn asset(url: &str, gz: &[u8], exe: &[u8]) -> EngineAsset {
        EngineAsset {
            url: url.into(),
            size: gz.len() as u64,
            sha256: hex(&Sha256::digest(gz)),
            exe_size: exe.len() as u64,
            exe_sha256: hex(&Sha256::digest(exe)),
        }
    }

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut e = GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn manifest_for(url: &str, gz: &[u8], exe: &[u8], protocol: u32) -> EngineManifest {
        let key = platform_key().expect("tests run on a supported platform");
        EngineManifest {
            schema: MANIFEST_SCHEMA,
            version: "0.1.0".into(),
            protocol,
            assets: BTreeMap::from([(key.to_string(), asset(url, gz, exe))]),
        }
    }

    #[test]
    fn manifests_validate() {
        let m = manifest_for("https://example.com/w.gz", b"gz", b"exe", PROTOCOL_VERSION);
        m.validate().unwrap();
        let json = serde_json::to_vec(&m).unwrap();
        assert_eq!(EngineManifest::parse(&json).unwrap(), m);

        let mut bad = m.clone();
        bad.assets.values_mut().next().unwrap().url = "http://example.com/w.gz".into();
        assert!(bad.validate().is_err(), "plain http is refused");
        let mut bad = m.clone();
        bad.assets.values_mut().next().unwrap().sha256 = "nope".into();
        assert!(bad.validate().is_err());
        let mut bad = m.clone();
        bad.schema = 9;
        assert!(bad.validate().is_err());
        let mut bad = m;
        bad.assets.clear();
        assert!(bad.validate().is_err());
        assert!(EngineManifest::parse(b"{ nope").is_err());
    }

    #[test]
    fn the_version_line_is_parsed() {
        assert_eq!(parse_version_output("bulbul-stt-worker 0.1.0 protocol 1 sherpa-onnx 2.0.1"), Some(1));
        assert_eq!(parse_version_output("bulbul-stt-worker protocol 7"), Some(7));
        assert_eq!(parse_version_output("hello"), None);
        assert_eq!(parse_version_output("protocol"), None);
        assert_eq!(parse_version_output("protocol x"), None);
    }

    #[test]
    fn this_platform_has_a_key() {
        assert!(platform_key().is_some());
        assert_eq!(worker_file_name().ends_with(".exe"), cfg!(windows));
    }

    fn quiet() -> ProgressFn {
        Arc::new(|_| {})
    }

    fn installer(root: &Path) -> Installer {
        Installer::new(Store::new(root))
            .unwrap()
            .allow_insecure_http_for_testing()
            .with_free_space(|_| Ok(u64::MAX / 2))
            .skip_engine_smoke_test_for_testing()
    }

    /// A fake "worker" payload: any bytes will do, the smoke test is bypassed.
    fn payload() -> (Vec<u8>, Vec<u8>) {
        let exe: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
        (gzip(&exe), exe)
    }

    #[tokio::test]
    async fn installs_a_verified_engine_and_replaces_an_older_one() {
        let (gz, exe) = payload();
        let server = serve(HashMap::from([("/w.gz".to_string(), gz.clone())]), Behaviour::default()).await;
        let url = format!("http://127.0.0.1:{}/w.gz", server.port);
        let m = manifest_for(&url, &gz, &exe, PROTOCOL_VERSION);

        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("engine");
        // An older install is already there.
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("old-marker"), b"old").unwrap();

        let phases: Arc<Mutex<Vec<Phase>>> = Arc::default();
        let p = phases.clone();
        let cb: ProgressFn = Arc::new(move |e| p.lock().unwrap().push(e.phase));
        let path = installer(tmp.path()).install_engine(&m, &dest, &CancelToken::new(), cb).await.unwrap();

        assert_eq!(path, dest.join(worker_file_name()));
        assert_eq!(std::fs::read(&path).unwrap(), exe);
        assert!(!dest.join("old-marker").exists(), "the old copy was replaced");
        assert!(!tmp.path().join(".engine-work").exists());
        assert!(!tmp.path().join(".engine-old").exists());
        let phases = phases.lock().unwrap();
        assert!(phases.contains(&Phase::Downloading) && phases.contains(&Phase::Unpacking));
        assert_eq!(phases.last(), Some(&Phase::Done));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o111, 0o111, "executable");
        }
    }

    #[tokio::test]
    async fn a_tampered_download_never_reaches_the_engine_folder() {
        let (gz, exe) = payload();
        let mut served = gz.clone();
        let mid = served.len() / 2;
        served[mid] ^= 0xff;
        let server = serve(HashMap::from([("/w.gz".to_string(), served)]), Behaviour::default()).await;
        let m = manifest_for(&format!("http://127.0.0.1:{}/w.gz", server.port), &gz, &exe, PROTOCOL_VERSION);
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("engine");
        let err = installer(tmp.path()).install_engine(&m, &dest, &CancelToken::new(), quiet()).await.unwrap_err();
        assert!(matches!(err, Error::HashMismatch { .. }), "{err}");
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn an_engine_that_unpacks_to_the_wrong_program_is_rejected() {
        let (gz, _exe) = payload();
        let server = serve(HashMap::from([("/w.gz".to_string(), gz.clone())]), Behaviour::default()).await;
        // The download is exactly what the manifest says, but the unpacked
        // program doesn't match its own declared hash.
        let other = vec![9u8; 200_000];
        let m = manifest_for(&format!("http://127.0.0.1:{}/w.gz", server.port), &gz, &other, PROTOCOL_VERSION);
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("engine");
        let err = installer(tmp.path()).install_engine(&m, &dest, &CancelToken::new(), quiet()).await.unwrap_err();
        assert!(matches!(err, Error::HashMismatch { .. } | Error::Archive(_)), "{err}");
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn an_engine_for_a_different_protocol_is_refused_before_downloading() {
        let (gz, exe) = payload();
        let server = serve(HashMap::from([("/w.gz".to_string(), gz.clone())]), Behaviour::default()).await;
        let m = manifest_for(&format!("http://127.0.0.1:{}/w.gz", server.port), &gz, &exe, PROTOCOL_VERSION + 1);
        let tmp = tempfile::tempdir().unwrap();
        let err = installer(tmp.path())
            .install_engine(&m, &tmp.path().join("engine"), &CancelToken::new(), quiet())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("update Bulbul"), "{err}");
        assert_eq!(server.count(), 0);
    }

    #[tokio::test]
    async fn a_platform_without_an_engine_says_so() {
        let (gz, exe) = payload();
        let mut m = manifest_for("https://example.com/w.gz", &gz, &exe, PROTOCOL_VERSION);
        m.assets = BTreeMap::from([("plan9-mips".to_string(), m.assets.values().next().unwrap().clone())]);
        let tmp = tempfile::tempdir().unwrap();
        let err = installer(tmp.path())
            .install_engine(&m, &tmp.path().join("engine"), &CancelToken::new(), quiet())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no speech engine for"), "{err}");
    }

    #[tokio::test]
    async fn a_failed_install_leaves_the_existing_engine_untouched() {
        let (gz, exe) = payload();
        let mut served = gz.clone();
        served[10] ^= 0xff;
        let server = serve(HashMap::from([("/w.gz".to_string(), served)]), Behaviour::default()).await;
        let m = manifest_for(&format!("http://127.0.0.1:{}/w.gz", server.port), &gz, &exe, PROTOCOL_VERSION);
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("engine");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("keep-me"), b"working engine").unwrap();
        assert!(installer(tmp.path()).install_engine(&m, &dest, &CancelToken::new(), quiet()).await.is_err());
        assert_eq!(std::fs::read(dest.join("keep-me")).unwrap(), b"working engine");
    }

    /// Needs the real worker: `BULBUL_STT_WORKER=<path> cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn the_real_worker_passes_the_version_check() {
        let path = std::env::var("BULBUL_STT_WORKER").expect("set BULBUL_STT_WORKER to the worker program");
        check_worker_version(Path::new(&path)).await.unwrap();
    }
}
