//! A std-only HTTP/1.1 stub on loopback that plays a Confluence server in tests.
//!
//! It binds `127.0.0.1:0`, answers each connection with the next [`Canned`]
//! response (then closes it), and records every request it saw. It is what lets
//! the `confluence_rest` transport and `glia docs sync` / `glia docs push` run
//! end to end without a live Atlassian site.
//!
//! Test support only. The module is compiled always (not behind a cargo feature,
//! so `cargo test -p <doc-sources>` can never silently skip a test that needs
//! it) and hidden from the docs; `doc-sources` is `publish = false` and not a
//! dependency of `py/`, so none of this reaches the wheel.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How long the stub waits for each connection, and for each read on one,
/// before giving up. A client that never calls cannot hang a test past this.
const DEADLINE: Duration = Duration::from_secs(10);

/// Largest request head (request line + headers) the stub will buffer.
const MAX_HEAD: usize = 64 * 1024;

/// One response the stub sends, in order, one per connection.
pub struct Canned {
    pub status: u16,
    pub body: String,
    /// Extra response headers (e.g. `Retry-After`), sent in order before
    /// `Content-Length`. A `Content-Type` here replaces the default
    /// `application/json`.
    pub headers: Vec<(String, String)>,
}

impl Canned {
    /// A `200` with `body` (a JSON document in every Confluence exchange).
    pub fn ok(body: impl Into<String>) -> Canned {
        Canned::status(200, body)
    }

    /// Any status with `body` and no extra headers (a 404 page, a 429 throttle).
    pub fn status(code: u16, body: impl Into<String>) -> Canned {
        Canned {
            status: code,
            body: body.into(),
            headers: Vec::new(),
        }
    }

    /// Add one response header.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// One request the stub received.
pub struct Recorded {
    pub method: String,
    /// The request target as sent: path plus query (`/wiki/rest/api/...?...`).
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Recorded {
    /// The value of header `name`, matched case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A running stub. [`StubServer::finish`] joins it and returns what it saw.
pub struct StubServer {
    origin: String,
    handle: JoinHandle<Vec<Recorded>>,
}

impl StubServer {
    /// Bind `127.0.0.1:0` and serve `responses` in order, one per connection,
    /// then stop. Waiting for a connection gives up after 10 s, so a test whose
    /// client makes fewer calls than it queued still finishes.
    pub fn start(responses: Vec<Canned>) -> io::Result<StubServer> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let handle = thread::Builder::new()
            .name("confluence-stub".into())
            .spawn(move || serve(&listener, responses))?;
        Ok(StubServer {
            origin: format!("http://127.0.0.1:{port}"),
            handle,
        })
    }

    /// `http://127.0.0.1:<port>` — pass it as the Confluence site.
    pub fn origin(&self) -> String {
        self.origin.clone()
    }

    /// Wait for the stub to serve its queue (or hit the deadline) and return
    /// every request it recorded, in arrival order.
    pub fn finish(self) -> Vec<Recorded> {
        self.handle.join().unwrap_or_default()
    }
}

fn serve(listener: &TcpListener, responses: Vec<Canned>) -> Vec<Recorded> {
    let mut seen = Vec::new();
    for canned in responses {
        let Some(mut stream) = accept_before(listener, Instant::now() + DEADLINE) else {
            break;
        };
        match read_request(&mut stream) {
            Ok(req) => seen.push(req),
            Err(_) => break,
        }
        if write_response(&mut stream, &canned).is_err() {
            break;
        }
    }
    seen
}

/// Poll the non-blocking listener until a client connects or `deadline` passes.
fn accept_before(listener: &TcpListener, deadline: Instant) -> Option<TcpStream> {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                // An accepted socket inherits O_NONBLOCK on some platforms.
                stream.set_nonblocking(false).ok()?;
                stream.set_read_timeout(Some(DEADLINE)).ok()?;
                stream.set_write_timeout(Some(DEADLINE)).ok()?;
                return Some(stream);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return None;
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return None,
        }
    }
}

/// Read one request: the head up to CRLFCRLF, then `Content-Length` body bytes.
fn read_request(stream: &mut TcpStream) -> io::Result<Recorded> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request head too large",
            ));
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "closed before end of head",
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap_or("").split(' ');
    let method = request_line.next().unwrap_or("").to_string();
    let target = request_line.next().unwrap_or("").to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let len = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "closed before end of body",
            ));
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(len);
    Ok(Recorded {
        method,
        target,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

/// `Connection: close` so the client opens a fresh connection per request —
/// the stub serves exactly one request per accepted connection.
fn write_response(stream: &mut TcpStream, canned: &Canned) -> io::Result<()> {
    let mut head = format!("HTTP/1.1 {} X\r\n", canned.status);
    if !canned.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
        head.push_str("Content-Type: application/json\r\n");
    }
    for (name, value) in &canned.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        canned.body.len()
    ));
    stream.write_all(head.as_bytes())?;
    stream.write_all(canned.body.as_bytes())?;
    stream.flush()
}
