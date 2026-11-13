// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! Runs the speech worker for the app.
//!
//! The supervisor starts the worker the first time it's needed, keeps it for
//! later dictations, reloads when the model changes, retries once if the
//! worker crashes or hangs, and shuts it down after a quiet spell so the
//! model's memory goes back to the computer. The worker lives exactly as long
//! as the model does.

use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};
use tokio::sync::Mutex;

use crate::client::WorkerClient;
use crate::error::{Error, Result};
use crate::transcriber::{Audio, LoadSpec, Transcript};

pub type BoxRead = Box<dyn AsyncRead + Send + Unpin>;
pub type BoxWrite = Box<dyn AsyncWrite + Send + Unpin>;
/// The last few lines the worker wrote to stderr, kept for crash reports.
pub type Tail = Arc<StdMutex<VecDeque<String>>>;

const TAIL_LINES: usize = 12;

/// A worker that has been started: its pipes, plus something that kills it
/// when dropped.
pub struct Launched {
    pub reader: BoxRead,
    pub writer: BoxWrite,
    pub guard: Box<dyn Send>,
    pub stderr: Tail,
}

/// How to start a worker. The real one spawns the program; tests plug in an
/// in-process fake.
pub trait Launcher: Send + Sync + 'static {
    fn launch(&self) -> impl Future<Output = Result<Launched>> + Send;
}

#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// Shut the worker down after this long without a dictation. Zero never.
    pub idle_unload: Duration,
    pub handshake_timeout: Duration,
    /// Loading a large model can take a while on a slow disk.
    pub load_timeout: Duration,
    /// Time allowed for a transcription: this plus `per_audio_second` for
    /// every second of audio.
    pub transcribe_base_timeout: Duration,
    pub per_audio_second: Duration,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        SupervisorConfig {
            idle_unload: Duration::from_secs(300),
            handshake_timeout: Duration::from_secs(15),
            load_timeout: Duration::from_secs(180),
            transcribe_base_timeout: Duration::from_secs(60),
            per_audio_second: Duration::from_secs(4),
        }
    }
}

struct Running {
    client: WorkerClient<BoxRead, BoxWrite>,
    stderr: Tail,
    loaded: Option<LoadSpec>,
    last_used: Instant,
    // Dropping this ends the worker process.
    _guard: Box<dyn Send>,
}

pub struct Supervisor<L: Launcher> {
    launcher: L,
    cfg: SupervisorConfig,
    /// Milliseconds of quiet before the worker is shut down; 0 never. Held
    /// apart from `cfg` so a setting change takes effect without a restart.
    idle_ms: AtomicU64,
    state: Mutex<Option<Running>>,
}

impl<L: Launcher> Supervisor<L> {
    pub fn new(launcher: L, cfg: SupervisorConfig) -> Arc<Self> {
        let idle_ms = AtomicU64::new(cfg.idle_unload.as_millis() as u64);
        Arc::new(Supervisor { launcher, cfg, idle_ms, state: Mutex::new(None) })
    }

    /// Transcribes `audio` with the model in `spec`, starting the worker and
    /// loading the model as needed. One dictation at a time: concurrent calls
    /// queue up.
    pub async fn transcribe(&self, spec: &LoadSpec, audio: &Audio, language: Option<&str>) -> Result<Transcript> {
        let mut state = self.state.lock().await;
        match self.attempt(&mut state, spec, audio, language).await {
            Err(e) if is_worker_failure(&e) => {
                // A crash or a hang: throw the worker away and try once more
                // with a fresh one.
                *state = None;
                let retry = self.attempt(&mut state, spec, audio, language).await;
                if retry.as_ref().is_err_and(is_worker_failure) {
                    *state = None;
                }
                retry
            }
            other => other,
        }
    }

    async fn attempt(
        &self,
        state: &mut Option<Running>,
        spec: &LoadSpec,
        audio: &Audio,
        language: Option<&str>,
    ) -> Result<Transcript> {
        if state.is_none() {
            let launched = self.launcher.launch().await?;
            let client = tokio::time::timeout(
                self.cfg.handshake_timeout,
                WorkerClient::handshake(launched.reader, launched.writer),
            )
            .await
            .map_err(|_| Error::Timeout("the speech worker didn't answer its greeting".into()))??;
            *state = Some(Running {
                client,
                stderr: launched.stderr,
                loaded: None,
                last_used: Instant::now(),
                _guard: launched.guard,
            });
        }
        let running = state.as_mut().expect("just ensured");
        let result = Self::drive(&self.cfg, running, spec, audio, language).await;
        running.last_used = Instant::now();
        result.map_err(|e| add_stderr(e, &running.stderr))
    }

    async fn drive(
        cfg: &SupervisorConfig,
        running: &mut Running,
        spec: &LoadSpec,
        audio: &Audio,
        language: Option<&str>,
    ) -> Result<Transcript> {
        if running.loaded.as_ref() != Some(spec) {
            running.loaded = None;
            tokio::time::timeout(cfg.load_timeout, running.client.load(spec))
                .await
                .map_err(|_| Error::Timeout(format!("loading {} took too long", spec.model_id)))??;
            running.loaded = Some(spec.clone());
        }
        let allowed = cfg.transcribe_base_timeout
            + cfg.per_audio_second.mul_f64(audio.duration_ms() as f64 / 1000.0);
        tokio::time::timeout(allowed, running.client.transcribe(audio, language))
            .await
            .map_err(|_| Error::Timeout(format!("transcribing took longer than {} s", allowed.as_secs())))?
    }

    /// Shuts the worker down, returning all its memory. The next dictation
    /// starts a fresh one.
    pub async fn unload(&self) {
        *self.state.lock().await = None;
    }

    pub async fn is_running(&self) -> bool {
        self.state.lock().await.is_some()
    }

    pub async fn loaded_model(&self) -> Option<String> {
        self.state.lock().await.as_ref().and_then(|r| r.loaded.as_ref().map(|s| s.model_id.clone()))
    }

    /// Changes how long the worker may sit unused before it is shut down to
    /// free the model's memory. Zero keeps it loaded.
    pub fn set_idle_unload(&self, idle: Duration) {
        self.idle_ms.store(idle.as_millis() as u64, Ordering::Relaxed);
    }

    fn idle(&self) -> Duration {
        Duration::from_millis(self.idle_ms.load(Ordering::Relaxed))
    }

    /// Background task that shuts the worker down after the idle time of
    /// quiet. Always running; it does nothing while the idle time is zero.
    pub fn spawn_idle_reaper(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let this = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let tick = match this.upgrade() {
                    Some(sup) => {
                        let idle = sup.idle();
                        if idle.is_zero() {
                            Duration::from_secs(1)
                        } else {
                            (idle / 4).clamp(Duration::from_millis(20), Duration::from_secs(30))
                        }
                    }
                    None => return,
                };
                tokio::time::sleep(tick).await;
                let Some(sup) = this.upgrade() else { return };
                let idle = sup.idle();
                if idle.is_zero() {
                    continue;
                }
                // Never block behind a dictation in progress: only look when free.
                let Ok(mut state) = sup.state.try_lock() else { continue };
                if state.as_ref().is_some_and(|r| r.last_used.elapsed() >= idle) {
                    *state = None;
                }
            }
        })
    }
}

fn is_worker_failure(e: &Error) -> bool {
    matches!(e, Error::WorkerGone(_) | Error::Timeout(_))
}

fn add_stderr(e: Error, tail: &Tail) -> Error {
    let lines: Vec<String> = tail.lock().map(|t| t.iter().cloned().collect()).unwrap_or_default();
    if lines.is_empty() {
        return e;
    }
    match e {
        Error::WorkerGone(m) => Error::WorkerGone(format!("{m} (last output: {})", lines.join(" | "))),
        other => other,
    }
}

// ---------- the real thing: a child process ----------

/// Starts the worker program from disk.
pub struct ProcessLauncher {
    pub path: PathBuf,
}

impl ProcessLauncher {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        ProcessLauncher { path: path.into() }
    }
}

/// Only these variables reach the worker. In particular, none of the app's
/// own environment (API keys and the like) does.
pub fn minimal_env(vars: impl Iterator<Item = (String, String)>) -> Vec<(String, String)> {
    const ALLOWED: &[&str] = &["SYSTEMROOT", "WINDIR", "TEMP", "TMP", "TMPDIR", "LANG", "LC_ALL"];
    vars.filter(|(k, _)| ALLOWED.iter().any(|a| a.eq_ignore_ascii_case(k))).collect()
}

impl Launcher for ProcessLauncher {
    async fn launch(&self) -> Result<Launched> {
        let mut cmd = tokio::process::Command::new(&self.path);
        cmd.env_clear();
        for (k, v) in minimal_env(std::env::vars()) {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash

        let mut child = cmd.spawn().map_err(|e| {
            Error::WorkerGone(format!("couldn't start the speech worker at {}: {e}", self.path.display()))
        })?;
        let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(Error::WorkerGone("the speech worker's pipes weren't available".into()));
        };

        // Always drain stderr: a full pipe would freeze the worker.
        let tail: Tail = Arc::default();
        let sink = tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(mut t) = sink.lock() {
                    if t.len() == TAIL_LINES {
                        t.pop_front();
                    }
                    t.push_back(line);
                }
            }
        });

        Ok(Launched { reader: Box::new(stdout), writer: Box::new(stdin), guard: Box::new(child), stderr: tail })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::ModelKind;
    use crate::protocol::{decode_json, parse_header, write_frame, Request, Response};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

    #[derive(Clone, Default)]
    struct Counters {
        launches: Arc<AtomicUsize>,
        loads: Arc<AtomicUsize>,
        transcribes: Arc<AtomicUsize>,
    }

    #[derive(Clone)]
    struct Behaviour {
        reply: String,
        /// Hang up when the Nth transcribe request (counted across launches,
        /// starting at 1) arrives; `usize::MAX` means every time from then on.
        crash_from: Option<usize>,
        hang: bool,
        fail_load: Option<String>,
    }

    impl Default for Behaviour {
        fn default() -> Self {
            Behaviour { reply: "hello there".into(), crash_from: None, hang: false, fail_load: None }
        }
    }

    struct AbortOnDrop(tokio::task::JoinHandle<()>);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    struct FakeLauncher {
        counters: Counters,
        behaviour: Behaviour,
    }

    impl Launcher for FakeLauncher {
        async fn launch(&self) -> Result<Launched> {
            self.counters.launches.fetch_add(1, Ordering::SeqCst);
            let (srv_out, cli_in) = duplex(1 << 20);
            let (cli_out, srv_in) = duplex(1 << 20);
            let c = self.counters.clone();
            let b = self.behaviour.clone();
            let task = tokio::spawn(async move {
                let (mut input, mut output) = (srv_in, srv_out);
                loop {
                    let mut header = [0u8; 8];
                    if input.read_exact(&mut header).await.is_err() {
                        return;
                    }
                    let (jl, pl) = parse_header(header).unwrap();
                    let mut json = vec![0u8; jl];
                    input.read_exact(&mut json).await.unwrap();
                    let mut payload = vec![0u8; pl];
                    input.read_exact(&mut payload).await.unwrap();
                    let resp = match decode_json::<Request>(&json).unwrap() {
                        Request::Hello { protocol } => Response::Ready { protocol, engine: "fake".into() },
                        Request::Load { model_id, .. } => {
                            c.loads.fetch_add(1, Ordering::SeqCst);
                            match &b.fail_load {
                                Some(m) => Response::Error { id: None, message: m.clone() },
                                None => Response::Loaded { model_id },
                            }
                        }
                        Request::Transcribe { id, .. } => {
                            let n = c.transcribes.fetch_add(1, Ordering::SeqCst) + 1;
                            if b.hang {
                                std::future::pending::<()>().await;
                            }
                            if b.crash_from.is_some_and(|from| n >= from) {
                                return; // drops the pipes: the "process" died
                            }
                            Response::Transcript { id, text: b.reply.clone(), language: None }
                        }
                        Request::Unload => Response::Unloaded,
                        Request::Shutdown => return,
                    };
                    let mut buf = Vec::new();
                    write_frame(&mut buf, &resp, &[]).unwrap();
                    if output.write_all(&buf).await.is_err() {
                        return;
                    }
                }
            });
            Ok(Launched {
                reader: Box::new(cli_in),
                writer: Box::new(cli_out),
                guard: Box::new(AbortOnDrop(task)),
                stderr: Arc::default(),
            })
        }
    }

    fn sup(behaviour: Behaviour, cfg: SupervisorConfig) -> (Arc<Supervisor<FakeLauncher>>, Counters) {
        let counters = Counters::default();
        let s = Supervisor::new(FakeLauncher { counters: counters.clone(), behaviour }, cfg);
        (s, counters)
    }

    fn quick() -> SupervisorConfig {
        SupervisorConfig {
            idle_unload: Duration::ZERO,
            handshake_timeout: Duration::from_millis(500),
            load_timeout: Duration::from_millis(500),
            transcribe_base_timeout: Duration::from_millis(150),
            per_audio_second: Duration::ZERO,
        }
    }

    fn spec(id: &str) -> LoadSpec {
        LoadSpec { model_id: id.into(), kind: ModelKind::Whisper, paths: Default::default(), num_threads: None }
    }

    fn audio() -> Audio {
        Audio { sample_rate: 16_000, samples: vec![0.0; 1_600] }
    }

    #[tokio::test]
    async fn the_worker_starts_on_first_use_and_is_reused() {
        let (s, c) = sup(Behaviour::default(), quick());
        assert!(!s.is_running().await, "nothing is started until it's needed");
        for _ in 0..3 {
            assert_eq!(s.transcribe(&spec("a"), &audio(), None).await.unwrap().text, "hello there");
        }
        assert_eq!(c.launches.load(Ordering::SeqCst), 1);
        assert_eq!(c.loads.load(Ordering::SeqCst), 1);
        assert_eq!(c.transcribes.load(Ordering::SeqCst), 3);
        assert_eq!(s.loaded_model().await.as_deref(), Some("a"));
    }

    #[tokio::test]
    async fn a_different_model_is_loaded_into_the_same_worker() {
        let (s, c) = sup(Behaviour::default(), quick());
        s.transcribe(&spec("a"), &audio(), None).await.unwrap();
        s.transcribe(&spec("b"), &audio(), None).await.unwrap();
        s.transcribe(&spec("b"), &audio(), None).await.unwrap();
        assert_eq!(c.launches.load(Ordering::SeqCst), 1);
        assert_eq!(c.loads.load(Ordering::SeqCst), 2);
        assert_eq!(s.loaded_model().await.as_deref(), Some("b"));
    }

    #[tokio::test]
    async fn a_worker_that_keeps_crashing_is_given_up_on_after_one_retry() {
        // From the first transcribe request on, every worker dies.
        let b = Behaviour { crash_from: Some(1), ..Default::default() };
        let (s, c) = sup(b, quick());
        let out = s.transcribe(&spec("a"), &audio(), None).await;
        assert!(matches!(out, Err(Error::WorkerGone(_))), "{out:?}");
        assert_eq!(c.launches.load(Ordering::SeqCst), 2, "relaunched once, then gave up");
        assert!(!s.is_running().await, "a dead worker is not kept");
    }

    #[tokio::test]
    async fn a_one_off_crash_is_invisible_to_the_caller() {
        struct OneShot(FakeLauncher, AtomicUsize);
        impl Launcher for OneShot {
            async fn launch(&self) -> Result<Launched> {
                // The first worker crashes on its first transcribe; later ones are healthy.
                let n = self.1.fetch_add(1, Ordering::SeqCst);
                let mut l = FakeLauncher { counters: self.0.counters.clone(), behaviour: self.0.behaviour.clone() };
                l.behaviour.crash_from = if n == 0 { Some(1) } else { None };
                l.launch().await
            }
        }
        let counters = Counters::default();
        let s = Supervisor::new(
            OneShot(FakeLauncher { counters: counters.clone(), behaviour: Behaviour::default() }, AtomicUsize::new(0)),
            quick(),
        );
        let out = s.transcribe(&spec("a"), &audio(), None).await.unwrap();
        assert_eq!(out.text, "hello there");
        assert_eq!(counters.launches.load(Ordering::SeqCst), 2);
        assert_eq!(counters.loads.load(Ordering::SeqCst), 2, "the model was reloaded into the new worker");
    }

    #[tokio::test]
    async fn a_hung_worker_times_out_is_killed_and_retried() {
        let b = Behaviour { hang: true, ..Default::default() };
        let (s, c) = sup(b, quick());
        let started = Instant::now();
        let out = s.transcribe(&spec("a"), &audio(), None).await;
        assert!(matches!(out, Err(Error::Timeout(_))), "{out:?}");
        assert_eq!(c.launches.load(Ordering::SeqCst), 2);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!s.is_running().await);
    }

    #[tokio::test]
    async fn an_engine_error_is_not_retried_and_keeps_the_worker() {
        let b = Behaviour { fail_load: Some("model file is damaged".into()), ..Default::default() };
        let (s, c) = sup(b, quick());
        let err = s.transcribe(&spec("a"), &audio(), None).await.unwrap_err();
        assert!(err.to_string().contains("damaged"), "{err}");
        assert_eq!(c.launches.load(Ordering::SeqCst), 1);
        assert_eq!(c.loads.load(Ordering::SeqCst), 1);
        assert_eq!(s.loaded_model().await, None, "nothing is loaded after a failed load");
    }

    #[tokio::test]
    async fn the_worker_is_shut_down_after_an_idle_spell() {
        let cfg = SupervisorConfig { idle_unload: Duration::from_millis(120), ..quick() };
        let (s, c) = sup(Behaviour::default(), cfg);
        let _reaper = s.spawn_idle_reaper();
        s.transcribe(&spec("a"), &audio(), None).await.unwrap();
        assert!(s.is_running().await);
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!s.is_running().await, "memory is returned after a quiet spell");
        // The next dictation just starts a fresh worker.
        s.transcribe(&spec("a"), &audio(), None).await.unwrap();
        assert_eq!(c.launches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn idle_unloading_can_be_turned_off_and_back_on() {
        let cfg = SupervisorConfig { idle_unload: Duration::ZERO, ..quick() };
        let (s, _) = sup(Behaviour::default(), cfg);
        let _reaper = s.spawn_idle_reaper();
        s.transcribe(&spec("a"), &audio(), None).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(s.is_running().await, "zero means never unload");

        // Turned on at runtime, the same worker is now reaped.
        s.set_idle_unload(Duration::from_millis(100));
        for _ in 0..50 {
            if !s.is_running().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the worker should have been unloaded once idle unloading was switched on");
    }

    #[tokio::test]
    async fn unload_stops_the_worker() {
        let (s, _) = sup(Behaviour::default(), quick());
        s.transcribe(&spec("a"), &audio(), None).await.unwrap();
        s.unload().await;
        assert!(!s.is_running().await);
    }

    #[tokio::test]
    async fn simultaneous_dictations_queue_up_on_one_worker() {
        let (s, c) = sup(Behaviour::default(), quick());
        let mut tasks = Vec::new();
        for _ in 0..5 {
            let s = s.clone();
            tasks.push(tokio::spawn(async move { s.transcribe(&spec("a"), &audio(), None).await }));
        }
        for t in tasks {
            assert_eq!(t.await.unwrap().unwrap().text, "hello there");
        }
        assert_eq!(c.launches.load(Ordering::SeqCst), 1);
        assert_eq!(c.transcribes.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn a_missing_worker_program_fails_with_a_clear_message() {
        let s = Supervisor::new(ProcessLauncher::new("/definitely/not/here/bulbul-stt-worker"), quick());
        let err = s.transcribe(&spec("a"), &audio(), None).await.unwrap_err();
        assert!(err.to_string().contains("couldn't start"), "{err}");
    }

    #[test]
    fn the_worker_never_sees_the_apps_secrets() {
        let env = vec![
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("GROQ_API_KEY".to_string(), "gsk_secret".to_string()),
            ("HOME".to_string(), "/home/u".to_string()),
            ("SystemRoot".to_string(), "C:\\Windows".to_string()),
            ("TEMP".to_string(), "C:\\Temp".to_string()),
            ("LANG".to_string(), "en_US.UTF-8".to_string()),
        ];
        let kept: Vec<String> = minimal_env(env.into_iter()).into_iter().map(|(k, _)| k).collect();
        assert!(kept.contains(&"SystemRoot".to_string()));
        assert!(kept.contains(&"TEMP".to_string()));
        assert!(kept.contains(&"LANG".to_string()));
        for secret in ["GROQ_API_KEY", "PATH", "HOME"] {
            assert!(!kept.contains(&secret.to_string()), "{secret} must not be passed on");
        }
    }
}
