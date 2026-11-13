// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! Downloads a model into the [`Store`] and proves it arrived intact.
//!
//! - **Resumable.** Data goes to `<file>.part`; an interrupted download picks
//!   up where it stopped (HTTP `Range`), restarting cleanly if the server
//!   ignores the range. Short network drops are retried automatically.
//! - **Verified.** Every file is checked against the catalog's sha256 before
//!   it gets its final name; a mismatch is deleted, never kept.
//! - **Safe to unpack.** Archives are unpacked by *listed* file only, so an
//!   archive can't write anywhere the catalog didn't name.
//! - **Checks disk space first**, and refuses with exact numbers.
//! - **Cancellable.** Cancelling leaves the partial data so it can resume.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::catalog::{ModelEntry, RemoteFile, Source};
use crate::error::{Error, Result};
use crate::store::{Status, Store};

/// Headroom kept free on top of what a download needs.
const SPACE_MARGIN: u64 = 128 * 1024 * 1024;
/// A transfer that produces no data for this long is treated as dropped.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Automatic retries (each resuming) before an error is reported.
const RETRIES: u32 = 3;
/// How often progress is reported at most.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Downloading,
    Verifying,
    Unpacking,
    Done,
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub model_id: String,
    pub phase: Phase,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// Cheap to clone; every clone cancels the same download.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

type SpaceFn = dyn Fn(&Path) -> std::io::Result<u64> + Send + Sync;
/// Called (throttled) as a download advances. Owned so it can be shared with
/// the thread that unpacks archives.
pub type ProgressFn = Arc<dyn Fn(Progress) + Send + Sync>;

pub struct Installer {
    client: reqwest::Client,
    store: Store,
    free_space: Arc<SpaceFn>,
    allow_http: bool,
    retry_delay: Duration,
    pub(crate) skip_smoke_test: bool,
}

impl Installer {
    pub fn new(store: Store) -> Result<Installer> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .user_agent("Bulbul")
            .build()
            .map_err(|e| Error::Network { what: "the download client".into(), message: e.to_string() })?;
        Ok(Installer {
            client,
            store,
            free_space: Arc::new(|p| fs4::available_space(p)),
            allow_http: false,
            retry_delay: Duration::from_secs(1),
            skip_smoke_test: false,
        })
    }

    /// Test hook: pretend the disk has this much free space.
    #[doc(hidden)]
    pub fn with_free_space(mut self, f: impl Fn(&Path) -> std::io::Result<u64> + Send + Sync + 'static) -> Self {
        self.free_space = Arc::new(f);
        self
    }

    /// Test hook: allow `http://` (the catalog only ever contains `https://`).
    #[doc(hidden)]
    pub fn allow_insecure_http_for_testing(mut self) -> Self {
        self.allow_http = true;
        self.retry_delay = Duration::from_millis(10);
        self
    }

    /// Test hook: don't run a freshly installed worker (the test payload isn't a program).
    #[doc(hidden)]
    pub fn skip_engine_smoke_test_for_testing(mut self) -> Self {
        self.skip_smoke_test = true;
        self
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Bytes still to be written for this model, counting data already on disk.
    pub fn bytes_needed(&self, entry: &ModelEntry) -> Result<u64> {
        let dir = self.store.model_dir(&entry.id)?;
        match &entry.source {
            Source::Files { files, .. } => Ok(files
                .iter()
                .map(|f| {
                    if file_len(&dir.join(&f.path)) == Some(f.size) {
                        0
                    } else {
                        f.size.saturating_sub(file_len(&part_path(&dir.join(&f.path))).unwrap_or(0))
                    }
                })
                .sum()),
            Source::Archive { url, size, .. } => {
                let have = file_len(&archive_part(&dir, url)).unwrap_or(0).min(*size);
                // The archive and the unpacked files exist side by side briefly.
                Ok(size - have + entry.disk_bytes)
            }
        }
    }

    /// Fails with exact numbers when `dir` doesn't have `needed` bytes free
    /// (plus the usual margin).
    pub(crate) fn ensure_space(&self, dir: &Path, needed: u64) -> Result<()> {
        let available = (self.free_space)(dir).map_err(|e| Error::io(dir, e))?;
        if available < needed.saturating_add(SPACE_MARGIN) {
            return Err(Error::NotEnoughSpace { needed: needed + SPACE_MARGIN, available });
        }
        Ok(())
    }

    /// Downloads one file to `dest`, resuming and retrying, and checks its
    /// size and sha256. Used for things that aren't catalog models.
    pub(crate) async fn download_verified(
        &self,
        url: &str,
        dest: &Path,
        file: &RemoteFile,
        cancel: &CancelToken,
        label: &str,
        on_progress: ProgressFn,
    ) -> Result<()> {
        let report = Reporter::new(label, on_progress);
        self.fetch_verified(url, dest, file, cancel, &report, 0, file.size).await
    }

    /// Downloads and verifies `entry`, then marks it complete. Returns at once
    /// if it's already installed and verified.
    pub async fn install(
        &self,
        entry: &ModelEntry,
        cancel: &CancelToken,
        on_progress: ProgressFn,
    ) -> Result<()> {
        if self.store.status(entry)? == (Status::Installed { verified: true }) {
            return Ok(());
        }
        let dir = self.store.model_dir(&entry.id)?;
        fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;

        self.ensure_space(&dir, self.bytes_needed(entry)?)?;

        let report = Reporter::new(&entry.id, on_progress);
        match &entry.source {
            Source::Files { base_url, files } => {
                let total: u64 = files.iter().map(|f| f.size).sum();
                let mut done = 0u64;
                for f in files {
                    let url = format!("{}/{}", base_url.trim_end_matches('/'), f.path);
                    self.fetch_verified(&url, &dir.join(&f.path), f, cancel, &report, done, total).await?;
                    done += f.size;
                }
            }
            Source::Archive { url, size, sha256, strip_prefix, files, .. } => {
                let part = archive_part(&dir, url);
                if let Some(parent) = part.parent() {
                    fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
                }
                let whole = RemoteFile { path: url.clone(), size: *size, sha256: sha256.clone() };
                // `fetch_verified` finishes by renaming `.part` to the final
                // name; for an archive that name is the `.download/<name>` file.
                let archive = part.with_extension("");
                self.fetch_verified(url, &archive, &whole, cancel, &report, 0, *size).await?;
                let (dir2, files2, prefix2) = (dir.clone(), files.clone(), strip_prefix.clone());
                let archive2 = archive.clone();
                let cancel2 = cancel.clone();
                let id = entry.id.clone();
                let cb = report.clone();
                tokio::task::spawn_blocking(move || {
                    unpack_tar_bz2(&archive2, &dir2, prefix2.as_deref(), &files2, &cancel2, &cb, &id)
                })
                .await
                .map_err(|e| Error::Archive(e.to_string()))??;
                let _ = fs::remove_dir_all(dir.join(".download"));
            }
        }

        self.store.mark_complete(entry)?;
        report.send(Phase::Done, entry.disk_bytes, entry.disk_bytes, true);
        Ok(())
    }

    /// Downloads `url` into `dest` (via `dest.part`), resuming and retrying,
    /// then verifies the hash and renames it into place.
    #[allow(clippy::too_many_arguments)]
    async fn fetch_verified(
        &self,
        url: &str,
        dest: &Path,
        file: &RemoteFile,
        cancel: &CancelToken,
        report: &Reporter,
        done_before: u64,
        total: u64,
    ) -> Result<()> {
        if !self.allow_http && !url.starts_with("https://") {
            return Err(Error::Network { what: file.path.clone(), message: "only https:// downloads are allowed".into() });
        }
        if file_len(dest) == Some(file.size) && sha256_file(dest, cancel)? == file.sha256 {
            return Ok(());
        }
        let _ = fs::remove_file(dest);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let part = part_path(dest);

        let mut attempt = 0;
        loop {
            match self.download_into(url, &part, file, cancel, report, done_before, total).await {
                Ok(()) => break,
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(e @ Error::Network { .. }) if attempt < RETRIES && is_transient(&e) => {
                    attempt += 1;
                    tracing_hint(&format!("retrying {} ({attempt}/{RETRIES}): {e}", file.path));
                    tokio::time::sleep(self.retry_delay * attempt).await;
                }
                Err(e) => return Err(e),
            }
        }

        report.send(Phase::Verifying, done_before + file.size, total, true);
        let got = sha256_file(&part, cancel)?;
        if got != file.sha256 {
            let _ = fs::remove_file(&part);
            return Err(Error::HashMismatch { file: file.path.clone() });
        }
        fs::rename(&part, dest).map_err(|e| Error::io(dest, e))
    }

    #[allow(clippy::too_many_arguments)]
    async fn download_into(
        &self,
        url: &str,
        part: &Path,
        file: &RemoteFile,
        cancel: &CancelToken,
        report: &Reporter,
        done_before: u64,
        total: u64,
    ) -> Result<()> {
        let net = |m: String| Error::Network { what: file.path.clone(), message: m };
        let mut have = file_len(part).unwrap_or(0);
        if have > file.size {
            fs::remove_file(part).map_err(|e| Error::io(part, e))?;
            have = 0;
        }
        if have == file.size {
            return Ok(());
        }

        let mut req = self.client.get(url);
        if have > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let mut resp = req.send().await.map_err(|e| net(e.without_url().to_string()))?;
        let status = resp.status();
        let mut out = match status.as_u16() {
            206 => tokio::fs::OpenOptions::new().append(true).open(part).await,
            200 => {
                // Fresh start, or the server ignored our range: begin again.
                have = 0;
                tokio::fs::File::create(part).await
            }
            416 => {
                // We already have more than the server thinks exists.
                let _ = fs::remove_file(part);
                return Err(net("the server rejected the resume range; starting over".into()));
            }
            code => return Err(net(format!("the server answered {code}"))),
        }
        .map_err(|e| Error::io(part, e))?;

        let mut written = have;
        loop {
            if cancel.is_cancelled() {
                let _ = out.flush().await;
                return Err(Error::Cancelled);
            }
            let chunk = match tokio::time::timeout(STALL_TIMEOUT, resp.chunk()).await {
                Err(_) => return Err(net("the connection stalled".into())),
                Ok(Err(e)) => return Err(net(e.without_url().to_string())),
                Ok(Ok(c)) => c,
            };
            let Some(chunk) = chunk else { break };
            out.write_all(&chunk).await.map_err(|e| Error::io(part, e))?;
            written += chunk.len() as u64;
            if written > file.size {
                return Err(net("the server sent more data than expected".into()));
            }
            report.send(Phase::Downloading, done_before + written, total, false);
        }
        out.flush().await.map_err(|e| Error::io(part, e))?;
        // The throttle may have swallowed the last updates; always say where
        // this file ended up.
        report.send(Phase::Downloading, done_before + written, total, true);
        if written != file.size {
            return Err(net(format!("the transfer ended early ({written} of {} bytes)", file.size)));
        }
        Ok(())
    }
}

fn is_transient(e: &Error) -> bool {
    // Everything that isn't a clear client/server rejection is worth a retry.
    match e {
        Error::Network { message, .. } => !message.starts_with("the server answered 4"),
        _ => false,
    }
}

// No logging dependency here on purpose: callers watch progress, not logs.
fn tracing_hint(_msg: &str) {}

pub(crate) fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

fn archive_part(dir: &Path, url: &str) -> PathBuf {
    let name = url.rsplit('/').next().unwrap_or("archive");
    part_path(&dir.join(".download").join(name))
}

fn file_len(p: &Path) -> Option<u64> {
    fs::metadata(p).ok().map(|m| m.len())
}

pub(crate) fn sha256_file(path: &Path, cancel: &CancelToken) -> Result<String> {
    let mut f = fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let n = f.read(&mut buf).map_err(|e| Error::io(path, e))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(D[(b >> 4) as usize] as char);
        s.push(D[(b & 15) as usize] as char);
    }
    s
}

/// Throttled progress reporting, shareable with the unpacking thread.
#[derive(Clone)]
struct Reporter {
    id: String,
    cb: ProgressFn,
    last: Arc<std::sync::Mutex<Instant>>,
}

impl Reporter {
    fn new(id: &str, cb: ProgressFn) -> Reporter {
        Reporter {
            id: id.to_string(),
            cb,
            last: Arc::new(std::sync::Mutex::new(Instant::now() - PROGRESS_EVERY)),
        }
    }

    fn send(&self, phase: Phase, bytes_done: u64, bytes_total: u64, force: bool) {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if !force && last.elapsed() < PROGRESS_EVERY {
            return;
        }
        *last = Instant::now();
        drop(last);
        (self.cb)(Progress { model_id: self.id.clone(), phase, bytes_done, bytes_total });
    }
}

/// Writes through to `inner` while hashing what passes.
pub(crate) struct HashWriter<W: Write> {
    pub(crate) inner: W,
    pub(crate) hasher: Sha256,
    pub(crate) written: u64,
}

impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.written += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Unpacks only the listed files from a `.tar.bz2`, checking each one's size
/// and hash, into a staging folder, then moves them into place.
fn unpack_tar_bz2(
    archive: &Path,
    dir: &Path,
    strip_prefix: Option<&str>,
    wanted: &[RemoteFile],
    cancel: &CancelToken,
    report: &Reporter,
    _id: &str,
) -> Result<()> {
    let bad = |m: String| Error::Archive(m);
    let staging = dir.join(".unpack");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| Error::io(&staging, e))?;

    let total: u64 = wanted.iter().map(|f| f.size).sum();
    let mut done = 0u64;
    let mut found = std::collections::HashSet::new();

    let file = fs::File::open(archive).map_err(|e| Error::io(archive, e))?;
    let mut tar = tar::Archive::new(bzip2::read::BzDecoder::new(file));
    for entry in tar.entries().map_err(|e| bad(e.to_string()))? {
        if cancel.is_cancelled() {
            let _ = fs::remove_dir_all(&staging);
            return Err(Error::Cancelled);
        }
        let mut entry = entry.map_err(|e| bad(e.to_string()))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let raw = entry.path().map_err(|e| bad(e.to_string()))?.to_string_lossy().replace('\\', "/");
        let name = raw.strip_prefix("./").unwrap_or(&raw);
        let rel = match strip_prefix {
            Some(p) => match name.strip_prefix(p).and_then(|r| r.strip_prefix('/')) {
                Some(r) => r,
                None => continue,
            },
            None => name,
        };
        let Some(want) = wanted.iter().find(|f| f.path == rel) else { continue };

        let target = staging.join(&want.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let out = fs::File::create(&target).map_err(|e| Error::io(&target, e))?;
        let mut w = HashWriter { inner: out, hasher: Sha256::new(), written: 0 };
        std::io::copy(&mut entry, &mut w).map_err(|e| bad(e.to_string()))?;
        w.flush().map_err(|e| Error::io(&target, e))?;
        if w.written != want.size || hex(&w.hasher.finalize()) != want.sha256 {
            let _ = fs::remove_dir_all(&staging);
            return Err(Error::HashMismatch { file: want.path.clone() });
        }
        found.insert(want.path.as_str());
        done += want.size;
        // One update per extracted file (there are only a handful), so the
        // throttle never hides the unpacking phase.
        report.send(Phase::Unpacking, done, total, true);
    }
    if let Some(missing) = wanted.iter().find(|f| !found.contains(f.path.as_str())) {
        let _ = fs::remove_dir_all(&staging);
        return Err(bad(format!("the archive is missing {}", missing.path)));
    }

    for f in wanted {
        let from = staging.join(&f.path);
        let to = dir.join(&f.path);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let _ = fs::remove_file(&to);
        fs::rename(&from, &to).map_err(|e| Error::io(&to, e))?;
    }
    let _ = fs::remove_dir_all(&staging);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{ArchiveFormat, License, ModelKind};
    use crate::test_server::{serve, Behaviour};
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Mutex;

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    /// Deterministic filler so every file has distinct, checkable content.
    fn blob(seed: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
    }

    fn license() -> License {
        License {
            spdx: "MIT".into(),
            author: "a".into(),
            url: "https://example.com".into(),
            attribution: None,
            converted: false,
        }
    }

    fn files_entry(port: u16, files: &[(&str, &[u8])]) -> ModelEntry {
        let listed: Vec<RemoteFile> = files
            .iter()
            .map(|(p, d)| RemoteFile { path: p.to_string(), size: d.len() as u64, sha256: sha(d) })
            .collect();
        let total: u64 = listed.iter().map(|f| f.size).sum();
        ModelEntry {
            id: "tiny".into(),
            name: "Tiny".into(),
            summary: "x".into(),
            kind: ModelKind::Whisper,
            languages: vec![],
            multilingual: true,
            experimental: false,
            license: license(),
            layout: BTreeMap::from([("encoder".to_string(), files[0].0.to_string())]),
            source: Source::Files { base_url: format!("http://127.0.0.1:{port}/m"), files: listed },
            download_bytes: total,
            disk_bytes: total,
        }
    }

    fn map(files: &[(&str, &[u8])]) -> HashMap<String, Vec<u8>> {
        files.iter().map(|(p, d)| (format!("/m/{p}"), d.to_vec())).collect()
    }

    fn installer(root: &Path) -> Installer {
        Installer::new(Store::new(root))
            .unwrap()
            .allow_insecure_http_for_testing()
            .with_free_space(|_| Ok(u64::MAX / 2))
    }

    fn recorder() -> (ProgressFn, Arc<Mutex<Vec<Progress>>>) {
        let log: Arc<Mutex<Vec<Progress>>> = Arc::default();
        let l = log.clone();
        (Arc::new(move |p| l.lock().unwrap().push(p)), log)
    }

    #[tokio::test]
    async fn downloads_verifies_and_installs() {
        let (a, t) = (blob(1, 50_000), blob(2, 700));
        let server = serve(map(&[("enc.onnx", &a), ("tokens.txt", &t)]), Behaviour::default()).await;
        let entry = files_entry(server.port, &[("enc.onnx", &a), ("tokens.txt", &t)]);
        let tmp = tempfile::tempdir().unwrap();
        let inst = installer(tmp.path());
        let (cb, log) = recorder();

        inst.install(&entry, &CancelToken::new(), cb).await.unwrap();

        let dir = tmp.path().join("tiny");
        assert_eq!(fs::read(dir.join("enc.onnx")).unwrap(), a);
        assert_eq!(fs::read(dir.join("tokens.txt")).unwrap(), t);
        assert!(!dir.join("enc.onnx.part").exists());
        assert_eq!(inst.store().status(&entry).unwrap(), Status::Installed { verified: true });

        let log = log.lock().unwrap();
        assert_eq!(log.last().unwrap().phase, Phase::Done);
        assert!(log.iter().any(|p| p.phase == Phase::Downloading));
        assert!(log.iter().any(|p| p.phase == Phase::Verifying));
        let max_done = log.iter().filter(|p| p.phase == Phase::Downloading).map(|p| p.bytes_done).max().unwrap();
        assert_eq!(max_done, (a.len() + t.len()) as u64);
    }

    #[tokio::test]
    async fn an_installed_model_is_not_downloaded_again() {
        let a = blob(1, 2_000);
        let server = serve(map(&[("enc.onnx", &a)]), Behaviour::default()).await;
        let entry = files_entry(server.port, &[("enc.onnx", &a)]);
        let tmp = tempfile::tempdir().unwrap();
        let inst = installer(tmp.path());
        inst.install(&entry, &CancelToken::new(), recorder().0).await.unwrap();
        let before = server.count();
        inst.install(&entry, &CancelToken::new(), recorder().0).await.unwrap();
        assert_eq!(server.count(), before);
    }

    #[tokio::test]
    async fn resumes_from_a_partial_file_with_a_range_request() {
        let a = blob(3, 40_000);
        let server = serve(map(&[("enc.onnx", &a)]), Behaviour::default()).await;
        let entry = files_entry(server.port, &[("enc.onnx", &a)]);
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("tiny");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("enc.onnx.part"), &a[..12_345]).unwrap();

        installer(tmp.path()).install(&entry, &CancelToken::new(), recorder().0).await.unwrap();

        assert_eq!(fs::read(dir.join("enc.onnx")).unwrap(), a);
        let reqs = server.requests.lock().unwrap();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].1, Some(12_345), "asked only for the missing part");
    }

    #[tokio::test]
    async fn starts_over_when_the_server_ignores_the_range() {
        let a = blob(4, 30_000);
        let server = serve(
            map(&[("enc.onnx", &a)]),
            Behaviour { ignore_range: true, ..Default::default() },
        )
        .await;
        let entry = files_entry(server.port, &[("enc.onnx", &a)]);
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("tiny");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("enc.onnx.part"), &a[..9_000]).unwrap();

        installer(tmp.path()).install(&entry, &CancelToken::new(), recorder().0).await.unwrap();
        assert_eq!(fs::read(dir.join("enc.onnx")).unwrap(), a, "no duplicated or missing bytes");
    }

    #[tokio::test]
    async fn recovers_by_itself_when_the_connection_drops_mid_file() {
        let a = blob(5, 60_000);
        let server = serve(
            map(&[("enc.onnx", &a)]),
            Behaviour { drop_first_after: Some(7_000), ..Default::default() },
        )
        .await;
        let entry = files_entry(server.port, &[("enc.onnx", &a)]);
        let tmp = tempfile::tempdir().unwrap();

        installer(tmp.path()).install(&entry, &CancelToken::new(), recorder().0).await.unwrap();

        assert_eq!(fs::read(tmp.path().join("tiny/enc.onnx")).unwrap(), a);
        assert!(server.count() >= 2, "it retried");
    }

    #[tokio::test]
    async fn a_corrupt_download_is_discarded_not_kept() {
        let real = blob(6, 5_000);
        let served = blob(7, 5_000); // same length, different bytes
        let server = serve(map(&[("enc.onnx", &served)]), Behaviour::default()).await;
        let entry = files_entry(server.port, &[("enc.onnx", &real)]);
        let tmp = tempfile::tempdir().unwrap();
        let inst = installer(tmp.path());

        let err = inst.install(&entry, &CancelToken::new(), recorder().0).await.unwrap_err();
        assert!(matches!(err, Error::HashMismatch { .. }), "{err}");
        let dir = tmp.path().join("tiny");
        assert!(!dir.join("enc.onnx").exists());
        assert!(!dir.join("enc.onnx.part").exists());
        assert_ne!(inst.store().status(&entry).unwrap(), Status::Installed { verified: true });
    }

    #[tokio::test]
    async fn cancelling_keeps_the_partial_data_so_it_can_resume() {
        let a = blob(8, 4_000_000);
        let server = serve(map(&[("enc.onnx", &a)]), Behaviour::default()).await;
        let entry = files_entry(server.port, &[("enc.onnx", &a)]);
        let tmp = tempfile::tempdir().unwrap();
        let inst = installer(tmp.path());

        let cancel = CancelToken::new();
        let c = cancel.clone();
        let cb: ProgressFn = Arc::new(move |p| {
            if p.phase == Phase::Downloading && p.bytes_done > 0 {
                c.cancel();
            }
        });
        let err = inst.install(&entry, &cancel, cb).await.unwrap_err();
        assert!(matches!(err, Error::Cancelled), "{err}");
        let part = tmp.path().join("tiny/enc.onnx.part");
        let kept = fs::metadata(&part).unwrap().len();
        assert!(kept > 0 && kept < a.len() as u64, "partial data kept ({kept} bytes)");
        assert_eq!(inst.store().status(&entry).unwrap(), Status::Incomplete);

        // Resuming later finishes the job.
        inst.install(&entry, &CancelToken::new(), recorder().0).await.unwrap();
        assert_eq!(fs::read(tmp.path().join("tiny/enc.onnx")).unwrap(), a);
        let reqs = server.requests.lock().unwrap();
        assert_eq!(reqs.last().unwrap().1, Some(kept), "the second run continued from the cut");
    }

    #[tokio::test]
    async fn refuses_up_front_when_there_is_not_enough_disk_space() {
        let a = blob(9, 10_000);
        let server = serve(map(&[("enc.onnx", &a)]), Behaviour::default()).await;
        let entry = files_entry(server.port, &[("enc.onnx", &a)]);
        let tmp = tempfile::tempdir().unwrap();
        let inst = Installer::new(Store::new(tmp.path()))
            .unwrap()
            .allow_insecure_http_for_testing()
            .with_free_space(|_| Ok(1_000));

        let err = inst.install(&entry, &CancelToken::new(), recorder().0).await.unwrap_err();
        match err {
            Error::NotEnoughSpace { needed, available } => {
                assert_eq!(available, 1_000);
                assert!(needed >= 10_000);
            }
            other => panic!("expected NotEnoughSpace, got {other}"),
        }
        assert_eq!(server.count(), 0, "nothing was downloaded");
    }

    #[tokio::test]
    async fn space_needed_shrinks_by_what_is_already_downloaded() {
        let a = blob(10, 10_000);
        let entry = files_entry(1, &[("enc.onnx", &a)]);
        let tmp = tempfile::tempdir().unwrap();
        let inst = installer(tmp.path());
        assert_eq!(inst.bytes_needed(&entry).unwrap(), 10_000);
        let dir = tmp.path().join("tiny");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("enc.onnx.part"), &a[..4_000]).unwrap();
        assert_eq!(inst.bytes_needed(&entry).unwrap(), 6_000);
        fs::write(dir.join("enc.onnx"), &a).unwrap();
        assert_eq!(inst.bytes_needed(&entry).unwrap(), 0);
    }

    #[tokio::test]
    async fn a_missing_file_fails_clearly_without_retrying() {
        let a = blob(11, 1_000);
        let server = serve(HashMap::new(), Behaviour::default()).await;
        let entry = files_entry(server.port, &[("enc.onnx", &a)]);
        let tmp = tempfile::tempdir().unwrap();
        let err = installer(tmp.path()).install(&entry, &CancelToken::new(), recorder().0).await.unwrap_err();
        assert!(err.to_string().contains("404"), "{err}");
        assert_eq!(server.count(), 1, "a 404 isn't worth retrying");
    }

    #[tokio::test]
    async fn plain_http_is_refused_outside_tests() {
        let a = blob(12, 1_000);
        let entry = files_entry(1, &[("enc.onnx", &a)]);
        let tmp = tempfile::tempdir().unwrap();
        let inst = Installer::new(Store::new(tmp.path()))
            .unwrap()
            .with_free_space(|_| Ok(u64::MAX / 2));
        let err = inst.install(&entry, &CancelToken::new(), recorder().0).await.unwrap_err();
        assert!(err.to_string().contains("https"), "{err}");
    }

    // ---------- archives ----------

    fn tar_bz2(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut b = tar::Builder::new(&mut tar_bytes);
            for (name, data) in entries {
                let mut h = tar::Header::new_gnu();
                h.set_size(data.len() as u64);
                h.set_mode(0o644);
                h.set_cksum();
                b.append_data(&mut h, name, *data).unwrap();
            }
            b.finish().unwrap();
        }
        let mut enc = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
        enc.write_all(&tar_bytes).unwrap();
        enc.finish().unwrap()
    }

    fn archive_entry(port: u16, archive: &[u8], wanted: &[(&str, &[u8])]) -> ModelEntry {
        let listed: Vec<RemoteFile> = wanted
            .iter()
            .map(|(p, d)| RemoteFile { path: p.to_string(), size: d.len() as u64, sha256: sha(d) })
            .collect();
        ModelEntry {
            id: "packed".into(),
            name: "Packed".into(),
            summary: "x".into(),
            kind: ModelKind::Transducer,
            languages: vec![],
            multilingual: false,
            experimental: false,
            license: license(),
            layout: BTreeMap::from([("encoder".to_string(), wanted[0].0.to_string())]),
            disk_bytes: listed.iter().map(|f| f.size).sum(),
            download_bytes: archive.len() as u64,
            source: Source::Archive {
                url: format!("http://127.0.0.1:{port}/pkg.tar.bz2"),
                size: archive.len() as u64,
                sha256: sha(archive),
                format: ArchiveFormat::TarBz2,
                strip_prefix: Some("pkg".into()),
                files: listed,
            },
        }
    }

    #[tokio::test]
    async fn unpacks_only_the_listed_files_from_an_archive() {
        let (enc, tok) = (blob(20, 30_000), blob(21, 400));
        let junk = blob(22, 9_000);
        let archive = tar_bz2(&[
            ("pkg/enc.onnx", &enc),
            ("pkg/tokens.txt", &tok),
            ("pkg/test_wavs/a.wav", &junk),
            ("pkg/README.md", b"read me"),
        ]);
        let server = serve(
            HashMap::from([("/pkg.tar.bz2".to_string(), archive.clone())]),
            Behaviour::default(),
        )
        .await;
        let entry = archive_entry(server.port, &archive, &[("enc.onnx", &enc), ("tokens.txt", &tok)]);
        let tmp = tempfile::tempdir().unwrap();
        let inst = installer(tmp.path());
        let (cb, log) = recorder();

        inst.install(&entry, &CancelToken::new(), cb).await.unwrap();

        let dir = tmp.path().join("packed");
        assert_eq!(fs::read(dir.join("enc.onnx")).unwrap(), enc);
        assert_eq!(fs::read(dir.join("tokens.txt")).unwrap(), tok);
        assert!(!dir.join("test_wavs").exists(), "unlisted files are not extracted");
        assert!(!dir.join("README.md").exists());
        assert!(!dir.join(".download").exists(), "the archive is cleaned up");
        assert!(!dir.join(".unpack").exists());
        assert_eq!(inst.store().status(&entry).unwrap(), Status::Installed { verified: true });
        assert!(log.lock().unwrap().iter().any(|p| p.phase == Phase::Unpacking));
    }

    #[tokio::test]
    async fn a_file_that_differs_from_the_catalog_inside_an_archive_is_rejected() {
        let enc = blob(23, 5_000);
        let archive = tar_bz2(&[("pkg/enc.onnx", &enc)]);
        let server = serve(
            HashMap::from([("/pkg.tar.bz2".to_string(), archive.clone())]),
            Behaviour::default(),
        )
        .await;
        let wrong = blob(24, 5_000);
        let entry = archive_entry(server.port, &archive, &[("enc.onnx", &wrong)]);
        let tmp = tempfile::tempdir().unwrap();
        let inst = installer(tmp.path());

        let err = inst.install(&entry, &CancelToken::new(), recorder().0).await.unwrap_err();
        assert!(matches!(err, Error::HashMismatch { .. }), "{err}");
        let dir = tmp.path().join("packed");
        assert!(!dir.join("enc.onnx").exists());
        assert!(!dir.join(".unpack").exists(), "staging is cleaned up");
    }

    #[tokio::test]
    async fn an_archive_missing_a_listed_file_is_rejected() {
        let enc = blob(25, 5_000);
        let archive = tar_bz2(&[("pkg/enc.onnx", &enc)]);
        let server = serve(
            HashMap::from([("/pkg.tar.bz2".to_string(), archive.clone())]),
            Behaviour::default(),
        )
        .await;
        let tok = blob(26, 100);
        let entry = archive_entry(server.port, &archive, &[("enc.onnx", &enc), ("tokens.txt", &tok)]);
        let tmp = tempfile::tempdir().unwrap();
        let err = installer(tmp.path()).install(&entry, &CancelToken::new(), recorder().0).await.unwrap_err();
        assert!(matches!(err, Error::Archive(_)), "{err}");
    }

    #[tokio::test]
    async fn a_corrupt_archive_download_is_rejected_before_unpacking() {
        let enc = blob(27, 5_000);
        let archive = tar_bz2(&[("pkg/enc.onnx", &enc)]);
        let mut served = archive.clone();
        let mid = served.len() / 2;
        served[mid] ^= 0xff; // flip a byte; same length
        let server = serve(
            HashMap::from([("/pkg.tar.bz2".to_string(), served)]),
            Behaviour::default(),
        )
        .await;
        let entry = archive_entry(server.port, &archive, &[("enc.onnx", &enc)]);
        let tmp = tempfile::tempdir().unwrap();
        let err = installer(tmp.path()).install(&entry, &CancelToken::new(), recorder().0).await.unwrap_err();
        assert!(matches!(err, Error::HashMismatch { .. }), "{err}");
        assert!(!tmp.path().join("packed/enc.onnx").exists());
    }
}
