//! The crash-isolated embedding worker (FB.14).
//!
//! A GPU session always runs in a CHILD process: Dawn / Vulkan and CUDA can
//! abort the whole process (the spike saw `free(): invalid pointer` and a
//! core dump from WebGPU at batch >= 64), and an abort in the child costs
//! only the GPU. The parent's [`WorkerEmbedder`] (an [`Embedder`]) starts the
//! child lazily on its first call, reuses it across calls and kills it on
//! drop; one per process, as FB.4's one-FastEmbedder rule. The CPU path never
//! spawns a child.
//!
//! THE PROTOCOL: length-prefixed frames on the child's stdin / stdout, each
//! `[tag u8][len u32 LE][payload]`.
//! - child -> parent `H` hello, first, before anything loads: `<PROTOCOL>\n<the
//!   devices the worker binary runs, comma-joined>`. A different [`PROTOCOL`]
//!   line (another glia build) is refused: fallback `no_worker:version ..`.
//! - parent -> child `I` init: `<model>\n<device>` (`bge-small` | `test-hash`;
//!   `cpu` | `webgpu` | `cuda`).
//! - child -> parent `R` ready: `<adapter name>\n<driver>\n<batch>`, once the
//!   session committed; or `X` with the reason it could not:
//!   `no_adapter:<first line of the error>`.
//! - parent -> child `E` embed: `[count u32]` then per text `[len u32][utf-8]`,
//!   at most `batch` texts.
//! - child -> parent `V` vectors: `[count u32][dims u32]` then `count * dims`
//!   f32 LE; or `X` with a reason: `oom` (CUDA, after one halved batch) or
//!   `error:<first line>`.
//! - stdin EOF ends the child with status 0.
//!
//! FALLBACK: when the worker cannot be started or dies before its hello
//! (`no_worker`, `no_worker:status=127` for a loader error), lacks the device
//! (`not_built`), reports `X` (its reason), or dies (`gpu_crash:signal=<n>` /
//! `gpu_crash:status=<n>`: an abort, a signal, a non-zero exit, a broken
//! pipe), the parent re-embeds every text not yet returned on an in-process
//! CPU embedder and keeps it for the rest of its life, printing the
//! [`crate::device::marker`] line once more with the reason. Only a start
//! failure under an explicit `GLIA_EMBED_DEVICE` is an Err instead (strict).
//!
//! The child's stderr is forwarded line by line, minus Dawn's `maxDynamic..
//! artificially reduced` limit warnings. `GLIA_EMBED_WORKER_TEST` is a
//! test-only knob read by the child: `abort_after=<n>` aborts on the embed
//! request after `n` answered ones, `no_adapter` answers init with `X
//! no_adapter:simulated`.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::thread::JoinHandle;

use crate::device::{self, Adapter, DEVICE_ENV, Device};
use crate::{DIMS_TEST_HASH, Embedder, HashEmbedder, MODEL_TEST_HASH, Tagged};

/// The hello line both sides must agree on: the frame version and this
/// glia-embed's release.
pub const PROTOCOL: &str = concat!("glia-embed-worker/1 ", env!("CARGO_PKG_VERSION"));

/// The test-only knob the child reads (module doc).
pub const WORKER_TEST_ENV: &str = "GLIA_EMBED_WORKER_TEST";

/// The worker binary glia-embed builds (`[[bin]]`), and glia-py's wheel
/// installs into its scripts directory.
pub const WORKER_BIN: &str = "glia-embed-worker";

/// The largest frame either side accepts (a guard against a corrupt length).
const MAX_FRAME: usize = 1 << 30;

/// Dawn's per-adapter limit warning, dropped from the forwarded stderr.
const DAWN_LIMIT_WARNING: &str = "artificially reduced";

/// How to start a worker process.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorkerProgram {
    /// The executable.
    pub program: PathBuf,
    /// Its arguments (`["embed-worker"]` for the `glia` binary).
    pub args: Vec<OsString>,
    /// The worker is this very binary, so its devices are this build's: a
    /// device it lacks is `not_built` without a spawn.
    pub builtin: bool,
    /// Extra environment for the child (tests set [`WORKER_TEST_ENV`] here,
    /// never in their own process).
    pub env: Vec<(OsString, OsString)>,
}

impl WorkerProgram {
    /// A separate worker executable (glia-py's `glia-embed-worker`).
    pub fn external(program: impl Into<PathBuf>) -> Self {
        WorkerProgram {
            program: program.into(),
            args: Vec::new(),
            builtin: false,
            env: Vec::new(),
        }
    }

    /// This program with `key=value` in the child's environment.
    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// This binary, run with `args` (the `glia` CLI: `embed-worker`).
    pub fn this_binary(args: &[&str]) -> Result<Self, String> {
        let program = std::env::current_exe().map_err(|e| format!("current executable: {e}"))?;
        Ok(WorkerProgram {
            program,
            args: args.iter().map(OsString::from).collect(),
            builtin: true,
            env: Vec::new(),
        })
    }

    /// The first `dirs` entry holding a [`WORKER_BIN`] executable.
    pub fn locate(dirs: &[PathBuf]) -> Option<Self> {
        let name = format!("{WORKER_BIN}{}", std::env::consts::EXE_SUFFIX);
        dirs.iter()
            .map(|d| d.join(&name))
            .find(|p| p.is_file())
            .map(Self::external)
    }

    /// [`WorkerProgram::locate`] in the current executable's directory and its
    /// parent (a cargo test or example sits one below the worker).
    pub fn beside_current_exe() -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?;
        let mut dirs = vec![dir.to_path_buf()];
        if let Some(up) = dir.parent() {
            dirs.push(up.to_path_buf());
        }
        Self::locate(&dirs)
    }
}

// ---------------------------------------------------------------- frames ---

fn write_frame(w: &mut impl Write, tag: u8, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    w.write_all(&[tag])?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

/// The next frame, or None on a clean EOF before its first byte.
fn read_frame(r: &mut impl Read) -> io::Result<Option<(u8, Vec<u8>)>> {
    let mut tag = [0u8; 1];
    if r.read(&mut tag)? == 0 {
        return Ok(None);
    }
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a frame of {len} bytes"),
        ));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok(Some((tag[0], payload)))
}

fn u32_at(b: &[u8], at: usize) -> io::Result<u32> {
    b.get(at..at + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a truncated frame"))
}

fn encode_texts(texts: &[String]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + texts.iter().map(|t| 4 + t.len()).sum::<usize>());
    out.extend_from_slice(&(texts.len() as u32).to_le_bytes());
    for t in texts {
        out.extend_from_slice(&(t.len() as u32).to_le_bytes());
        out.extend_from_slice(t.as_bytes());
    }
    out
}

fn decode_texts(b: &[u8]) -> io::Result<Vec<String>> {
    let n = u32_at(b, 0)? as usize;
    let mut at = 4;
    let mut out = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let len = u32_at(b, at)? as usize;
        at += 4;
        let s = b
            .get(at..at + len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a truncated text"))?;
        out.push(String::from_utf8_lossy(s).into_owned());
        at += len;
    }
    Ok(out)
}

fn encode_vectors(vs: &[Vec<f32>], dims: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + vs.len() * dims as usize * 4);
    out.extend_from_slice(&(vs.len() as u32).to_le_bytes());
    out.extend_from_slice(&dims.to_le_bytes());
    for v in vs {
        for x in v {
            out.extend_from_slice(&x.to_le_bytes());
        }
    }
    out
}

fn decode_vectors(b: &[u8]) -> io::Result<Vec<Vec<f32>>> {
    let n = u32_at(b, 0)? as usize;
    let dims = u32_at(b, 4)? as usize;
    let body = &b[8.min(b.len())..];
    if body.len() != n * dims * 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} vector bytes for {n} x {dims}", body.len()),
        ));
    }
    Ok(body
        .chunks_exact(dims.max(1) * 4)
        .take(n)
        .map(|v| {
            v.chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()
        })
        .collect())
}

// ----------------------------------------------------------------- child ---

/// The worker's entry point: `glia embed-worker` and `glia-embed-worker`
/// call it and exit with what it returns. Speaks the module's protocol on
/// stdin / stdout and writes nothing else to stdout; 0 on stdin EOF, 2 on a
/// protocol error.
pub fn worker_main() -> i32 {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = BufReader::new(stdin.lock());
    let mut output = BufWriter::new(stdout.lock());
    match serve(&mut input, &mut output) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("[embed] worker error: {e}");
            2
        }
    }
}

/// What the test knob asks for.
#[derive(Default)]
struct Knob {
    abort_after: Option<usize>,
    no_adapter: bool,
}

fn knob() -> Knob {
    let v = std::env::var(WORKER_TEST_ENV).unwrap_or_default();
    let mut k = Knob::default();
    for part in v.split(',').map(str::trim) {
        if part == "no_adapter" {
            k.no_adapter = true;
        } else if let Some(n) = part.strip_prefix("abort_after=") {
            k.abort_after = n.parse().ok();
        }
    }
    k
}

fn serve(input: &mut impl Read, output: &mut impl Write) -> io::Result<()> {
    let devices: Vec<&str> = device::built_devices()
        .into_iter()
        .map(Device::as_str)
        .collect();
    write_frame(
        output,
        b'H',
        format!("{PROTOCOL}\n{}", devices.join(",")).as_bytes(),
    )?;
    let Some((b'I', init)) = read_frame(input)? else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected an init frame",
        ));
    };
    let init = String::from_utf8_lossy(&init).into_owned();
    let (model, device) = init.split_once('\n').unwrap_or((init.as_str(), ""));
    let device = Device::parse(device).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown device `{device}`"),
        )
    })?;
    let knob = knob();
    if knob.no_adapter {
        return write_frame(output, b'X', b"no_adapter:simulated");
    }
    let (embedder, adapter, batch) = match open(model, device) {
        Ok(opened) => opened,
        Err(reason) => return write_frame(output, b'X', reason.as_bytes()),
    };
    write_frame(
        output,
        b'R',
        format!("{}\n{}\n{batch}", adapter.name, adapter.driver).as_bytes(),
    )?;
    let mut answered = 0usize;
    while let Some((tag, payload)) = read_frame(input)? {
        if tag != b'E' {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected frame `{}`", tag as char),
            ));
        }
        if knob.abort_after == Some(answered) {
            std::process::abort();
        }
        let texts = decode_texts(&payload)?;
        match embedder.embed(&texts) {
            Ok(vs) => write_frame(output, b'V', &encode_vectors(&vs, embedder.dims()))?,
            Err(e) => {
                let reason = if e.starts_with("oom:") {
                    "oom".to_string()
                } else {
                    format!("error:{}", device::short_reason(&e))
                };
                write_frame(output, b'X', reason.as_bytes())?;
            }
        }
        answered += 1;
    }
    Ok(())
}

/// The child's session: `model` on `device`, the adapter it reports and the
/// batch it takes; Err is the `X` reason.
fn open(model: &str, device: Device) -> Result<(Box<dyn Embedder>, Adapter, usize), String> {
    match model {
        // Device-independent: lets the protocol be tested without a GPU.
        "test-hash" => Ok((
            Box::new(HashEmbedder),
            Adapter::new(MODEL_TEST_HASH, "none"),
            device::batch(device),
        )),
        "bge-small" => open_bge(device),
        other => Err(format!("error:unknown model `{other}`")),
    }
}

#[cfg(all(
    feature = "fastembed",
    not(all(target_os = "macos", target_arch = "x86_64"))
))]
fn open_bge(device: Device) -> Result<(Box<dyn Embedder>, Adapter, usize), String> {
    if !device.built() {
        return Err("not_built".to_string());
    }
    let adapter = if device == Device::Cpu {
        device::cpu_adapter()
    } else {
        device::gpu_adapter()
    };
    match crate::FastEmbedder::new(device) {
        Ok(e) => {
            let batch = e.batch();
            Ok((Box::new(e), adapter, batch))
        }
        Err(e) => Err(format!("no_adapter:{}", device::short_reason(&e))),
    }
}

#[cfg(not(all(
    feature = "fastembed",
    not(all(target_os = "macos", target_arch = "x86_64"))
)))]
fn open_bge(_device: Device) -> Result<(Box<dyn Embedder>, Adapter, usize), String> {
    Err("not_built".to_string())
}

// ---------------------------------------------------------------- parent ---

/// Builds the in-process CPU embedder a [`WorkerEmbedder`] falls back to.
pub type CpuFactory = fn() -> Result<Box<dyn Embedder>, String>;

/// A running child.
struct Live {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: Option<JoinHandle<()>>,
    batch: usize,
}

impl Live {
    /// Close stdin, kill the child if it still runs, reap it, and return its
    /// exit status.
    fn stop(mut self) -> Option<ExitStatus> {
        drop(self.stdin);
        let status = match self.child.try_wait() {
            Ok(Some(s)) => Some(s),
            _ => {
                let _ = self.child.kill();
                self.child.wait().ok()
            }
        };
        if let Some(h) = self.stderr.take() {
            let _ = h.join();
        }
        status
    }

    /// Wait for the child to end (it has failed a read or write) and describe
    /// how it ended.
    fn crashed(mut self) -> String {
        drop(self.stdin);
        let status = self.child.wait().ok();
        if let Some(h) = self.stderr.take() {
            let _ = h.join();
        }
        format!(
            "gpu_crash:{}",
            status.map_or("unknown".to_string(), describe)
        )
    }
}

/// `signal=<n>` (unix) or `status=<n>`.
fn describe(status: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return format!("signal={sig}");
        }
    }
    match status.code() {
        Some(c) => format!("status={c}"),
        None => "status=unknown".to_string(),
    }
}

enum State {
    Idle,
    Live(Live),
    Cpu(Box<dyn Embedder>),
}

/// The parent side: an [`Embedder`] whose vectors come from a worker child on
/// a GPU device, else (module doc) from an in-process CPU embedder.
pub struct WorkerEmbedder {
    spelling: &'static str,
    model_name: &'static str,
    dims: u32,
    device: Device,
    strict: bool,
    program: Option<WorkerProgram>,
    cpu: CpuFactory,
    state: Mutex<State>,
    fallback: Mutex<Option<String>>,
}

impl std::fmt::Debug for WorkerEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerEmbedder")
            .field("model", &self.model_name)
            .field("device", &self.device)
            .field("strict", &self.strict)
            .field("program", &self.program)
            .finish()
    }
}

impl WorkerEmbedder {
    /// A worker embedder of `model` (`bge-small` | `test-hash`) on `device`,
    /// started lazily from `program` (None: there is no worker, so the first
    /// call falls back with `no_worker`), falling back to `cpu`. `strict`
    /// turns a START failure into an Err naming [`DEVICE_ENV`] (an explicit
    /// device); a failure after the start always falls back.
    pub fn new(
        model: &str,
        device: Device,
        strict: bool,
        program: Option<WorkerProgram>,
        cpu: CpuFactory,
    ) -> Result<Self, String> {
        let (spelling, model_name, dims) = match model {
            "bge-small" => ("bge-small", crate::MODEL_BGE_SMALL, crate::DIMS_BGE_SMALL),
            "test-hash" => ("test-hash", MODEL_TEST_HASH, DIMS_TEST_HASH),
            other => {
                return Err(format!(
                    "unknown embedding model `{other}`: expected one of {}",
                    crate::MODEL_NAMES.join(", ")
                ));
            }
        };
        Ok(WorkerEmbedder {
            spelling,
            model_name,
            dims,
            device,
            strict,
            program,
            cpu,
            state: Mutex::new(State::Idle),
            fallback: Mutex::new(None),
        })
    }

    /// The device vectors come from now: the GPU device while the worker
    /// runs (or before it started), the CPU after a fallback.
    pub fn backend(&self) -> Device {
        match self.state.lock().as_deref() {
            Ok(State::Cpu(_)) => Device::Cpu,
            _ => self.device,
        }
    }

    /// The last fallback reason, None while no fallback happened.
    pub fn fallback(&self) -> Option<String> {
        self.fallback.lock().ok().and_then(|f| f.clone())
    }

    /// Start the worker: Ok(Live) on a ready frame, else the fallback reason.
    fn spawn(&self) -> Result<Live, String> {
        let Some(program) = &self.program else {
            return Err("no_worker".to_string());
        };
        if program.builtin && !self.device.built() {
            return Err("not_built".to_string());
        }
        let mut child = Command::new(&program.program)
            .args(&program.args)
            .envs(program.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("no_worker:{} {e}", program.program.display()))?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("no_worker:no pipes".to_string());
        };
        let stderr = std::thread::Builder::new()
            .name("glia-embed-worker-stderr".to_string())
            .spawn(move || forward_stderr(stderr))
            .ok();
        let mut live = Live {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            stderr,
            batch: device::batch(self.device),
        };
        match self.handshake(&mut live) {
            Ok(batch) => {
                live.batch = batch.max(1);
                Ok(live)
            }
            Err(Handshake::Refused(reason)) => {
                live.stop();
                Err(reason)
            }
            Err(Handshake::Died) => Err(live.crashed()),
            Err(Handshake::Unstarted) => {
                let crashed = live.crashed();
                let how = crashed.strip_prefix("gpu_crash:").unwrap_or(&crashed);
                Err(format!("no_worker:{how}"))
            }
        }
    }

    fn handshake(&self, live: &mut Live) -> Result<usize, Handshake> {
        let hello = match read_frame(&mut live.stdout) {
            Ok(Some((b'H', p))) => String::from_utf8_lossy(&p).into_owned(),
            Ok(Some(_)) => return Err(Handshake::Refused("no_worker:no hello".to_string())),
            // Dead before its hello: the worker never ran (a loader error,
            // such as a missing Dawn library or a too-old glibc, exits 127).
            Ok(None) | Err(_) => return Err(Handshake::Unstarted),
        };
        let (proto, devices) = hello.split_once('\n').unwrap_or((hello.as_str(), ""));
        if proto != PROTOCOL {
            return Err(Handshake::Refused(format!("no_worker:version {proto}")));
        }
        // The test model runs on any worker; bge-small needs the device built.
        if self.spelling != "test-hash" && !devices.split(',').any(|d| d == self.device.as_str()) {
            return Err(Handshake::Refused("not_built".to_string()));
        }
        let init = format!("{}\n{}", self.spelling, self.device);
        if write_frame(&mut live.stdin, b'I', init.as_bytes()).is_err() {
            return Err(Handshake::Died);
        }
        match read_frame(&mut live.stdout) {
            Ok(Some((b'R', p))) => {
                let ready = String::from_utf8_lossy(&p).into_owned();
                let mut parts = ready.splitn(3, '\n');
                let name = parts.next().unwrap_or("unknown");
                let driver = parts.next().unwrap_or("unknown");
                let batch = parts
                    .next()
                    .and_then(|b| b.parse().ok())
                    .unwrap_or(device::batch(self.device));
                eprintln!(
                    "{}",
                    device::marker(&Adapter::new(name, driver), self.device, None, batch)
                );
                Ok(batch)
            }
            Ok(Some((b'X', p))) => {
                Err(Handshake::Refused(String::from_utf8_lossy(&p).into_owned()))
            }
            Ok(Some(_)) => Err(Handshake::Refused("no_worker:no ready frame".to_string())),
            Ok(None) | Err(_) => Err(Handshake::Died),
        }
    }

    /// Switch to the CPU embedder for good, recording and printing `reason`.
    fn fall_back(&self, reason: String) -> Result<Box<dyn Embedder>, String> {
        let cpu = (self.cpu)()?;
        let adapter = if cpu.device().is_some() {
            device::cpu_adapter()
        } else {
            Adapter::new(cpu.model().to_string(), "none")
        };
        eprintln!(
            "{}",
            device::marker(
                &adapter,
                Device::Cpu,
                Some(&reason),
                device::batch(Device::Cpu)
            )
        );
        if let Ok(mut f) = self.fallback.lock() {
            *f = Some(reason);
        }
        Ok(cpu)
    }

    /// One sub-batch through the worker: Ok(vectors), or Err(reason) after
    /// which the worker is gone.
    fn round(&self, live: &mut Live, texts: &[String]) -> Result<Vec<Vec<f32>>, Round> {
        if write_frame(&mut live.stdin, b'E', &encode_texts(texts)).is_err() {
            return Err(Round::Died);
        }
        match read_frame(&mut live.stdout) {
            Ok(Some((b'V', p))) => {
                let vs = decode_vectors(&p).map_err(|e| Round::Failed(format!("error:{e}")))?;
                if vs.len() != texts.len() || vs.iter().any(|v| v.len() != self.dims as usize) {
                    return Err(Round::Failed(format!(
                        "error:{} vectors for {} texts",
                        vs.len(),
                        texts.len()
                    )));
                }
                Ok(vs)
            }
            Ok(Some((b'X', p))) => Err(Round::Failed(String::from_utf8_lossy(&p).into_owned())),
            Ok(Some(_)) => Err(Round::Failed("error:unexpected frame".to_string())),
            Ok(None) | Err(_) => Err(Round::Died),
        }
    }
}

enum Handshake {
    Refused(String),
    Unstarted,
    Died,
}

enum Round {
    Failed(String),
    Died,
}

fn forward_stderr(stderr: impl Read) {
    let reader = BufReader::new(stderr);
    for line in reader.split(b'\n') {
        let Ok(line) = line else {
            break;
        };
        let line = String::from_utf8_lossy(&line);
        if line.contains(DAWN_LIMIT_WARNING) {
            continue;
        }
        eprintln!("{}", line.trim_end_matches('\r'));
    }
}

impl Embedder for WorkerEmbedder {
    fn model(&self) -> &str {
        self.model_name
    }

    fn dims(&self) -> u32 {
        self.dims
    }

    fn device(&self) -> Option<Device> {
        Some(self.backend())
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Ok(self
            .embed_tagged(texts)?
            .into_iter()
            .map(|(v, _)| v)
            .collect())
    }

    fn embed_tagged(&self, texts: &[String]) -> Result<Vec<Tagged>, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "the embedding worker was poisoned by an earlier panic".to_string())?;
        if matches!(*state, State::Idle) {
            *state = match self.spawn() {
                Ok(live) => State::Live(live),
                Err(reason) if self.strict => {
                    return Err(format!(
                        "{DEVICE_ENV}={}: the {} device did not start ({reason})",
                        self.device, self.device
                    ));
                }
                Err(reason) => State::Cpu(self.fall_back(reason)?),
            };
        }
        let mut out: Vec<Tagged> = Vec::with_capacity(texts.len());
        if let State::Live(live) = &mut *state {
            let batch = live.batch;
            let mut failure = None;
            for sub in texts.chunks(batch) {
                match self.round(live, sub) {
                    Ok(vs) => out.extend(vs.into_iter().map(|v| (v, Some(self.device)))),
                    Err(r) => {
                        failure = Some(r);
                        break;
                    }
                }
            }
            if let Some(failure) = failure {
                let State::Live(live) = std::mem::replace(&mut *state, State::Idle) else {
                    return Err("the embedding worker state changed underfoot".to_string());
                };
                let reason = match failure {
                    Round::Failed(reason) => {
                        live.stop();
                        reason
                    }
                    Round::Died => live.crashed(),
                };
                *state = State::Cpu(self.fall_back(reason)?);
            }
        }
        if out.len() < texts.len()
            && let State::Cpu(cpu) = &*state
        {
            out.extend(cpu.embed_tagged(&texts[out.len()..])?);
        }
        Ok(out)
    }
}

impl Drop for WorkerEmbedder {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock()
            && let State::Live(live) = std::mem::replace(&mut *state, State::Idle)
        {
            live.stop();
        }
    }
}

/// The directories glia-py looks for [`WORKER_BIN`] in, given its module's
/// `__file__` and the interpreter: the interpreter's directory (a venv's
/// `bin` / `Scripts`), then every ancestor of the module's directory with a
/// `bin` or `Scripts` child (a `--user` or system install), nearest first.
pub fn worker_dirs(module_file: Option<&Path>, interpreter: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = interpreter.and_then(Path::parent) {
        dirs.push(dir.to_path_buf());
    }
    if let Some(module_dir) = module_file.and_then(Path::parent) {
        dirs.push(module_dir.to_path_buf());
        for a in module_dir.ancestors().skip(1).take(6) {
            for sub in ["bin", "Scripts"] {
                let d = a.join(sub);
                if !dirs.contains(&d) {
                    dirs.push(d);
                }
            }
        }
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let texts = vec!["a".to_string(), String::new(), "héllo".to_string()];
        assert_eq!(decode_texts(&encode_texts(&texts)).unwrap(), texts);
        let vs = vec![vec![1.0f32, -2.5], vec![0.0, 3.25]];
        assert_eq!(decode_vectors(&encode_vectors(&vs, 2)).unwrap(), vs);
        let mut buf = Vec::new();
        write_frame(&mut buf, b'E', b"xyz").unwrap();
        let mut r = &buf[..];
        assert_eq!(read_frame(&mut r).unwrap(), Some((b'E', b"xyz".to_vec())));
        assert_eq!(read_frame(&mut r).unwrap(), None);
        assert!(decode_vectors(&[1, 0, 0, 0, 2, 0, 0, 0]).is_err());
    }

    #[test]
    fn serve_speaks_the_protocol() {
        let mut input = Vec::new();
        write_frame(&mut input, b'I', b"test-hash\nwebgpu").unwrap();
        write_frame(&mut input, b'E', &encode_texts(&["get user".to_string()])).unwrap();
        let mut output = Vec::new();
        serve(&mut &input[..], &mut output).unwrap();
        let mut r = &output[..];
        let (t, hello) = read_frame(&mut r).unwrap().unwrap();
        assert_eq!(t, b'H');
        assert!(String::from_utf8_lossy(&hello).starts_with(PROTOCOL));
        let (t, ready) = read_frame(&mut r).unwrap().unwrap();
        assert_eq!(t, b'R');
        assert_eq!(
            String::from_utf8_lossy(&ready),
            format!("{MODEL_TEST_HASH}\nnone\n8")
        );
        let (t, v) = read_frame(&mut r).unwrap().unwrap();
        assert_eq!(t, b'V');
        assert_eq!(
            decode_vectors(&v).unwrap(),
            vec![HashEmbedder::embed_one("get user")]
        );
    }

    #[test]
    fn worker_dirs_order() {
        let dirs = worker_dirs(
            Some(Path::new(
                "/v/lib/python3.11/site-packages/glia_py/__init__.py",
            )),
            Some(Path::new("/v/bin/python")),
        );
        assert_eq!(dirs[0], PathBuf::from("/v/bin"));
        assert_eq!(
            dirs[1],
            PathBuf::from("/v/lib/python3.11/site-packages/glia_py")
        );
        assert!(dirs.contains(&PathBuf::from("/v/Scripts")));
    }
}
