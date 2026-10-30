//! A scripted HTTP/1.1 server on 127.0.0.1 for the transport tests. Standard library only (the crate has no
//! dev-dependencies). One thread per connection; keep-alive; every request is recorded.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// One request as the server saw it.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub target: String,
    /// Lower-case names.
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// Which accepted connection carried it (0, 1, ...): equal numbers mean a reused connection.
    pub conn: usize,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// What the server does with a request.
pub enum Action {
    /// A complete answer; the connection stays open (keep-alive).
    Respond {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    },
    /// Reads the request, then closes the connection without sending anything.
    Drop,
    /// Reads the request, then says nothing for this long (and closes).
    Hang(Duration),
    /// Answers at once with the headers of a `declared`-byte body, then sends `interval`-spaced single bytes.
    DripBody { declared: usize, interval: Duration },
    /// Sends part of the status line, then one byte per interval: a header drip.
    DripHeaders { interval: Duration },
    /// Headers of a `declared`-byte body, `send` bytes of it, then closes.
    Truncated { declared: usize, send: usize },
    /// 200 with no length (close-delimited), writing up to `total` bytes in 64 KiB blocks; counts what it wrote.
    Stream { total: usize },
    /// Headers announcing `declared` bytes of a body with this status, then writes as much as the client takes.
    BigBody { status: u16, declared: usize },
}

impl Action {
    pub fn json(status: u16, body: &str) -> Action {
        Action::Respond {
            status,
            headers: vec![],
            body: body.to_owned(),
        }
    }
    pub fn json_with(status: u16, headers: &[(&str, &str)], body: &str) -> Action {
        Action::Respond {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            body: body.to_owned(),
        }
    }
    pub fn error(status: u16, code: &str, message: &str) -> Action {
        Action::json(
            status,
            &format!(r#"{{"error":{{"code":"{code}","message":"{message}","requestId":"req_test"}}}}"#),
        )
    }
}

type Handler = dyn Fn(usize, &Seen) -> Action + Send + Sync;

pub struct Server {
    pub url: String,
    addr: std::net::SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    /// Bytes written by `Stream` and `BigBody` actions.
    pub written: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl Server {
    pub fn start(handler: impl Fn(usize, &Seen) -> Action + Send + Sync + 'static) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let written = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let handler: Arc<Handler> = Arc::new(handler);
        let counter = Arc::new(AtomicUsize::new(0));
        let conns = Arc::new(AtomicUsize::new(0));
        {
            let (seen, written, stop) = (seen.clone(), written.clone(), stop.clone());
            thread::spawn(move || {
                for conn in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(conn) = conn else { continue };
                    let (handler, seen, written, counter, stop) = (
                        handler.clone(),
                        seen.clone(),
                        written.clone(),
                        counter.clone(),
                        stop.clone(),
                    );
                    let id = conns.fetch_add(1, Ordering::SeqCst);
                    thread::spawn(move || serve(conn, id, handler, seen, written, counter, stop));
                }
            });
        }
        Server {
            url: format!("http://{addr}"),
            addr,
            seen,
            written,
            stop,
        }
    }

    /// Everything the server received so far.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    /// The number of requests for a given `METHOD target-prefix` after waiting a moment for stragglers: a replay that
    /// the HTTP library sent on its own would show up as an extra request.
    pub fn settled_count(&self) -> usize {
        thread::sleep(Duration::from_millis(300));
        self.count()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr); // wakes the accept loop
    }
}

fn read_request(conn: &mut TcpStream) -> Option<Seen> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        match conn.read(&mut byte) {
            Ok(1) => buf.push(byte[0]),
            _ => return None,
        }
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, target) = (first.next()?.to_owned(), first.next()?.to_owned());
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let len = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    conn.read_exact(&mut body).ok()?;
    Some(Seen {
        method,
        target,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
        conn: 0,
    })
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

fn serve(
    mut conn: TcpStream,
    conn_id: usize,
    handler: Arc<Handler>,
    seen: Arc<Mutex<Vec<Seen>>>,
    written: Arc<AtomicUsize>,
    counter: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
) {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(30)));
    while let Some(mut req) = read_request(&mut conn) {
        req.conn = conn_id;
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let idx = counter.fetch_add(1, Ordering::SeqCst);
        seen.lock().unwrap().push(req.clone());
        match handler(idx, &req) {
            Action::Respond { status, headers, body } => {
                let mut out = format!("HTTP/1.1 {status} {}\r\n", status_text(status));
                out.push_str("Content-Type: application/json\r\n");
                for (k, v) in headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                out.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
                if conn.write_all(out.as_bytes()).is_err() {
                    return;
                }
            }
            Action::Drop => {
                let _ = conn.shutdown(Shutdown::Both);
                return;
            }
            Action::Hang(d) => {
                thread::sleep(d);
                return;
            }
            Action::DripBody { declared, interval } => {
                let head =
                    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {declared}\r\n\r\n");
                if conn.write_all(head.as_bytes()).is_err() {
                    return;
                }
                for _ in 0..declared {
                    thread::sleep(interval);
                    if conn.write_all(b" ").is_err() {
                        return;
                    }
                }
                return;
            }
            Action::DripHeaders { interval } => {
                for b in b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}" {
                    thread::sleep(interval);
                    if conn.write_all(&[*b]).is_err() {
                        return;
                    }
                }
                return;
            }
            Action::Truncated { declared, send } => {
                let head =
                    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {declared}\r\n\r\n");
                let _ = conn.write_all(head.as_bytes());
                let _ = conn.write_all(&vec![b' '; send]);
                let _ = conn.shutdown(Shutdown::Both);
                return;
            }
            Action::Stream { total } => {
                if conn
                    .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: application/json\r\n\r\n")
                    .is_err()
                {
                    return;
                }
                let block = vec![b' '; 64 * 1024];
                let mut sent = 0;
                while sent < total {
                    if conn.write_all(&block).is_err() {
                        break;
                    }
                    sent += block.len();
                    written.store(sent, Ordering::SeqCst);
                }
                return;
            }
            Action::BigBody { status, declared } => {
                let head = format!(
                    "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {declared}\r\n\r\n",
                    status_text(status)
                );
                if conn.write_all(head.as_bytes()).is_err() {
                    return;
                }
                let block = vec![b' '; 64 * 1024];
                let mut sent = 0;
                while sent < declared {
                    if conn.write_all(&block[..block.len().min(declared - sent)]).is_err() {
                        break;
                    }
                    sent += block.len().min(declared - sent);
                    written.store(sent, Ordering::SeqCst);
                }
                return;
            }
        }
    }
}

/// An address where nothing listens (connection refused).
pub fn closed_port_url() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    format!("http://{addr}")
}

pub const KEY: &str = "vk_test_synthetic_key_for_unit_tests_000";
