use std::fs::File;
use std::fmt::Display;
use std::thread::sleep;
use std::time::Duration;
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::io::{Read, Write, BufWriter};
use std::sync::atomic::{Ordering, AtomicBool, AtomicU64};

use anyhow::Result;
use raylib_light::CloseWindow;
use multipart::server::Multipart;
use qrcodegen::{QrCode, QrCodeEcc};
use get_if_addrs::{IfAddr, get_if_addrs};
use rayon::iter::{ParallelBridge, ParallelIterator};
use tiny_http::{ReadWrite, Header, Server, Method, Request, Response, StatusCode};

mod qr;
use qr::*;

const SIZE_LIMIT: u64 = 1024 * 1024 * 1024;
const PROGRESS_CONNECTION_PRECLOSE_DELAY: u64 = 250; // millis
const DUMMY_HTTP_RQ: &[u8] = b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

const NOT_FOUND_HTML: &[u8] = b"<h1>NOT FOUND</h1>";
const HOME_HTML: &[u8] = include_bytes!("index.html");
const HOME_SCRIPT: &[u8] = include_bytes!("index.js");

type Clients = HashMap::<String, Box::<dyn ReadWrite + Send>>;

type AtomicStop = Arc::<AtomicBool>;
type AtomicClients = Arc::<Mutex::<Clients>>;

macro_rules! define_addr_port {
    (const ADDR: &str = $addr: literal; const PORT: &str = $port: literal;) => {
        #[allow(unused)]
        const ADDR: &str = $addr;
        #[allow(unused)]
        const PORT: &str = $port;
        #[allow(unused)]
        const ADDR_PORT: &str = concat!($addr, ':', $port);
    };
}

define_addr_port!{
    const ADDR: &str = "0.0.0.0";
    const PORT: &str = "6969";
}

#[derive(Eq, Hash, Clone, PartialEq)]
struct FilePath(String);

impl FilePath { const MAX_LEN: usize = 30; }

impl Display for FilePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let FilePath(ref full_file_path) = self;

        let ext_pos = full_file_path.rfind('.').unwrap_or(full_file_path.len());
        let (name_part, ext_part) = full_file_path.split_at(ext_pos);

        if full_file_path.len() > Self::MAX_LEN {
            const DOTS: &str = "[...]";
            let trim_len = Self::MAX_LEN - ext_part.len() - const { DOTS.len() };
            let trimmed_name = if trim_len > 0 {
                &name_part[..trim_len]
            } else {
                "" // ext too long
            };
            write!(f, "{trimmed_name}{DOTS}{ext_part}")
        } else {
            write!(f, "{full_file_path}")
        }
    }
}

struct ProgressTracker<'a, W: Write> {
    writer: W,
    total_size: u64,
    written: Arc::<AtomicU64>,
    file_path: &'a String,
    clients: AtomicClients
}

impl<'a, W: Write> ProgressTracker::<'a, W> {
    #[inline]
    pub fn new(writer: W, total_size: u64, file_path: &'a String, clients: AtomicClients) -> Self {
        Self {
            writer,
            total_size,
            written: Arc::new(AtomicU64::new(0)),

            file_path,
            clients
        }
    }

    #[inline]
    pub fn progress(&self) -> u64 {
        let written = self.written.load(Ordering::Relaxed);
        if written >= self.total_size - 1 {
            100
        } else {
            (written * 100 / self.total_size).min(100)
        }
    }
}

impl<W: Write> Write for ProgressTracker::<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result::<usize> {
        let written = self.writer.write(buf)?;
        self.written.fetch_add(written as u64, Ordering::Relaxed);

        let p = self.progress();
        if p % 5 == 0 {
            let msg = format!("data: {{ \"progress\": {p} }}\n\n");
            let file_path = self.file_path;
            let mut clients = self.clients.lock().unwrap();
            let writer = clients.get_mut(file_path).unwrap();
            if writer.write_all(msg.as_bytes()).is_err() {
                eprintln!("error: client disconnected from /{file_path}/progress");
            }
        }

        Ok(written)
    }

    #[inline(always)]
    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

#[inline]
fn serve_bytes(request: Request, bytes: &[u8], content_type: &str) -> Result::<()> {
    let content_type_header = unsafe {
        Header::from_bytes("Content-Type", content_type).unwrap_unchecked()
    };
    request.respond(Response::from_data(bytes).with_header(content_type_header))?;
    Ok(())
}

fn handle_upload(rq: &mut Request, clients: AtomicClients) -> Result::<()> {
    let content_type = rq.headers()
        .iter()
        .find(|header| header.field.equiv("Content-Type"))
        .ok_or_else(|| anyhow::anyhow!("Missing Content-Type header"))?;

    let content_type_value = content_type.value.as_str();
    if !content_type_value.starts_with("multipart/form-data; boundary=") {
        return Err(anyhow::anyhow!("Invalid Content-Type"))
    }

    let boundary = content_type_value["multipart/form-data; boundary=".len()..].to_owned();
    let mut multipart = Multipart::with_body(rq.as_reader(), boundary);

    let file_size = multipart.read_entry()?.map(|mut field| {
        if field.headers.name == "size".into() {
            let mut buf = String::with_capacity(20);
            field.data.read_to_string(&mut buf).unwrap();
            Some(buf.trim().parse().unwrap())
        } else {
            None
        }
    }).unwrap().unwrap();

    let mut field = multipart.read_entry()?.unwrap();

    let file_path = field.headers.filename.map(|f| FilePath(f.into())).unwrap();
    println!("[{file_path}] creating file");

    let file = File::create(file_path.0.as_str())?;
    let wbuf = BufWriter::new(file);
    let writer = ProgressTracker::new(wbuf, file_size, &file_path.0, Arc::clone(&clients));

    println!("[{file_path}] copying data..");
    field.data.save().size_limit(SIZE_LIMIT).write_to(writer);
    println!("[{file_path}] done!");

    {
        let mut clients = clients.lock().unwrap();
        let writer = clients.get_mut(&file_path.0).unwrap();
        if writer.write_all(b"data: {{ \"progress\": 100 }}\n\n").is_err() {
            eprintln!("error: client disconnected from /{file_path}/progress");
        }
    }

    sleep(Duration::from_millis(PROGRESS_CONNECTION_PRECLOSE_DELAY));
    _ = clients.lock().unwrap().remove_entry(&file_path.0);

    Ok(())
}

fn server_serve(server: &Server, stop: &AtomicStop) {
    let clients = Arc::new(Mutex::new(Clients::new()));

    println!("serving at: <http://{ADDR_PORT}>");
    server.incoming_requests().par_bridge().for_each(|mut rq| {
        if stop.load(Ordering::SeqCst) {
            println!("[INFO] shutting down the server.");
            server.unblock()
        }

        if let Err(err) = match (rq.method(), rq.url()) {
            (&Method::Get, "/") => serve_bytes(rq, HOME_HTML, "text/html; charset=UTF-8"),
            (&Method::Get, "/index.js") => serve_bytes(rq, HOME_SCRIPT, "application/js; charset=UTF-8"),
            (&Method::Post, "/upload") => {
                match handle_upload(&mut rq, Arc::clone(&clients)) {
                    Err(e) => rq.respond(Response::from_string(e.to_string()).with_status_code(StatusCode(500))),
                    _ => rq.respond(Response::empty(StatusCode(200))),
                }.map_err(Into::into)
            },
            (&Method::Get, path) => if path.starts_with("/progress") {
                let file_path = &path["/progress".len() + 1..];
                let response = Response::empty(StatusCode(200))
                    .with_header(Header::from_bytes("Content-Type", "text/event-stream").unwrap())
                    .with_header(Header::from_bytes("Cache-Control", "no-cache").unwrap())
                    .with_header(Header::from_bytes("Connection", "keep-alive").unwrap());

                let file_path = file_path.to_owned();
                let stream = rq.upgrade("SSE", response);

                println!("[INFO] client connected to <http://{ADDR_PORT}/{file_path}/progress>");
                clients.lock().unwrap().insert(file_path, stream);

                Ok(())
            } else {
                serve_bytes(rq, NOT_FOUND_HTML, "text/html; charset=UTF-8")
            }
            _ => Ok(())
        } {
            eprintln!("{err}")
        }
    });
}

fn dummy_http_rq(addr: &str) -> std::io::Result<()> {
    let mut stream = TcpStream::connect(addr)?;
    stream.write_all(DUMMY_HTTP_RQ)?;
    Ok(())
}

fn main() -> Result::<()> {
    let Some(local_ip) = get_if_addrs()?.into_iter().find_map(|iface| {
        if iface.is_loopback() { return None }
        if let IfAddr::V4(addr) = iface.addr {
            Some(addr.ip)
        } else {
            None
        }
    }) else {
        panic!("could not local ipv4 address of the network interface")
    };

    let stop = Arc::new(AtomicBool::new(false));
    let stopc = Arc::clone(&stop);

    let server_thread = std::thread::spawn(move || {
        let server = Server::http(ADDR_PORT).unwrap();
        server_serve(&server, &stopc)
    });

    let local_addr = format!("http://{local_ip:?}:{PORT}");
    let qr = QrCode::encode_text(&local_addr, QrCodeEcc::Low).expect("could not encode url to qr code");
    unsafe {
        init_raylib();
        draw_qr_code(gen_qr_canvas(&qr));
        CloseWindow()
    }

    stop.store(true, Ordering::SeqCst);
    // send a dummy request to unblock `incoming_requests`
    if let Err(e) = dummy_http_rq(ADDR_PORT) {
        eprintln!("error: could not send a dummy request to {ADDR_PORT} to `incoming_requests` to shut down the server: {e}")
    }

    server_thread.join().unwrap();

    Ok(())
}
