use std::fs::File;
use std::fmt::Display;
use std::collections::HashMap;
use std::net::{IpAddr, UdpSocket};
use std::io::{Read, Write, BufWriter};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::Result;
use multipart::server::Multipart;
use qrcodegen::{QrCode, QrCodeEcc};
use rayon::iter::{ParallelBridge, ParallelIterator};
use tiny_http::{ReadWrite, Header, Method, Request, Response, StatusCode, Server as TinyHttpServer};

mod qr;
use qr::*;
#[allow(unused_imports, unused_parens, non_camel_case_types, unused_mut, dead_code, unused_assignments, unused_variables, static_mut_refs, non_snake_case, non_upper_case_globals)]
mod stb_image_write;

const SIZE_LIMIT: u64 = 1024 * 1024 * 1024;

const HOME_HTML:          &[u8] = include_bytes!("index.html");
const HOME_SCRIPT:        &[u8] = include_bytes!("index.js");
const HOME_MOBILE_HTML:   &[u8] = include_bytes!("index-mobile.html");
const HOME_MOBILE_SCRIPT: &[u8] = include_bytes!("index-mobile.js");
const NOT_FOUND_HTML:     &[u8] = b"<h1>NOT FOUND</h1>";
const HTTP_OK_RESPONSE:   &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";

type Clients = HashMap::<String, Box::<dyn ReadWrite + Send>>;

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

macro_rules! progress_fmt {
    ($lit: literal) => { concat!("data: {{ \"progress\": ", $lit, "}}\n\n") };
    ($expr: expr) => { format!("data: {{ \"progress\": {} }}\n\n", $expr) };
}

macro_rules! anyerr {
    ($lit: literal) => { Err(anyhow::anyhow!($lit)) };
}

define_addr_port!{
    const ADDR: &str = "0.0.0.0";
    const PORT: &str = "6969";
}

struct FilePath(String);

impl FilePath {
    const MAX_LEN: usize = 30;
    const DOTS: &str = "[...]";

    fn new(full_file_path: String) -> Self {
        let s = if full_file_path.len() > Self::MAX_LEN {
            let ext_pos = full_file_path.rfind('.').unwrap_or(full_file_path.len());
            let (name_part, ext_part) = full_file_path.split_at(ext_pos);
            let trim_len = Self::MAX_LEN - ext_part.len() - const { Self::DOTS.len() };
            let trimmed_name = if trim_len > 0 {
                &name_part[..trim_len]
            } else {
                "" // ext too long
            };
            format!("{trimmed_name}{dots}{ext_part}", dots = Self::DOTS)
        } else {
            full_file_path
        };

        Self(s)
    }
}

impl Display for FilePath {
    #[inline(always)]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{fp}", fp = self.0)
    }
}

struct ProgressTracker<'a, W: Write> {
    writer: W,
    written: u64,
    total_size: u64,
    file_path: &'a String,
    clients: AtomicClients
}

impl<'a, W: Write> ProgressTracker::<'a, W> {
    #[inline]
    fn new(writer: W, total_size: u64, file_path: &'a String, clients: AtomicClients) -> Self {
        Self {
            writer,
            total_size,
            written: 0,

            file_path,
            clients
        }
    }

    fn report_progress_if_needed(&mut self) {
        let p = self.progress();
        if p % 5 != 0 { return }
        let mut clients = self.clients.lock().unwrap();
        let Some(writer) = clients.get_mut(self.file_path) else { return };
        if let Err(e) = writer.write_all(progress_fmt!(p).as_bytes()) {
            eprintln!("error: client disconnected from http://{ADDR_PORT}/progress/{fp}, or error occured: {e}", fp = self.file_path)
        }
    }

    #[inline(always)]
    fn progress(&self) -> u64 {
        (self.written * 100 / self.total_size).min(100)
    }
}

impl<W: Write> Write for ProgressTracker::<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result::<usize> {
        let written = self.writer.write(buf)?;
        self.written += written as u64;
        self.report_progress_if_needed();
        Ok(written)
    }

    #[inline(always)]
    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

#[inline(always)]
fn get_header<'a>(rq: &'a Request, field: &str) -> Option::<&'a Header> {
    rq.headers()
        .iter()
        .find(|header| header.field.as_str().as_str().eq_ignore_ascii_case(field))
}

#[inline]
fn user_agent_is_mobile(user_agent: &str) -> bool {
    [
        "Mobile",        // General mobile indicator
        "Android",       // Android devices
        "iPhone",        // iPhones
        "iPod",          // iPods
        "BlackBerry",    // BlackBerry devices
        "Windows Phone", // Windows Phones
        "Opera Mini",    // Opera Mini browser
        "IEMobile",      // Internet Explorer Mobile
    ].iter().any(|keyword| user_agent.contains(keyword))
}

struct Server {
    qr_png_bytes: Vec::<u8>,
    server: TinyHttpServer,
    clients: AtomicClients,
    sse_response: Response::<std::io::Empty>
}

impl Server {
    fn new(qr_png_bytes: Vec::<u8>) -> Self {
        Self {
            qr_png_bytes,
            server: TinyHttpServer::http(ADDR_PORT).unwrap(),
            clients: Arc::new(Mutex::new(Clients::new())),
            sse_response: Response::empty(200)
                .with_header(Header::from_bytes("Content-Type", "text/event-stream").unwrap())
                .with_header(Header::from_bytes("Cache-Control", "no-cache").unwrap())
                .with_header(Header::from_bytes("Connection", "keep-alive").unwrap()),
        }
    }

    #[track_caller]
    #[inline(always)]
    fn clients(&self) -> MutexGuard::<Clients> {
        self.clients.lock().unwrap()
    }

    fn handle_upload(&self, rq: &mut Request) -> Result::<FilePath> {
        let Some(content_type) = get_header(&*rq, "Content-Type") else {
            return anyerr!("request to `/upload` with no user-agent")
        };

        let content_type_value = content_type.value.as_str();
        if !content_type_value.starts_with("multipart/form-data; boundary=") {
            return anyerr!("invalid Content-Type")
        }

        let boundary = content_type_value["multipart/form-data; boundary=".len()..].to_owned();
        let mut multipart = Multipart::with_body(rq.as_reader(), boundary);

        let Ok(file_size) = multipart.read_entry() else { return anyerr!("Invalid Multipart data") };
        let Some(Some(file_size)) = file_size.map(|mut field| {
            if field.headers.name == "size".into() {
                let mut buf = String::with_capacity(20);
                field.data.read_to_string(&mut buf).unwrap();
                Some(buf.trim().parse().unwrap())
            } else {
                None
            }
        }) else {
            return anyerr!("invalid Multipart data")
        };

        let Ok(Some(mut field)) = multipart.read_entry() else {
            return anyerr!("invalid Multipart data")
        };

        let Some(file_path_string) = field.headers.filename else {
            return anyerr!("invalid Multipart data")
        };

        #[cfg(debug_assertions)]
        let file_path_string = file_path_string + ".test";

        let file_path = FilePath::new(file_path_string.to_owned());

        println!("[{file_path}] creating file");
        let file = File::create(&file_path_string)?;
        let wbuf = BufWriter::new(file);
        let writer = ProgressTracker::new(wbuf, file_size, &file_path_string, Arc::clone(&self.clients));

        println!("[{file_path}] copying data..");
        field.data.save().size_limit(SIZE_LIMIT).write_to(writer);
        println!("[{file_path}] done!");

        {
            let mut clients = self.clients();
            if let Some(writer) = clients.get_mut(&file_path.0) {
                #[allow(unused)]
                if let Err(e) = writer.write_all(const { progress_fmt!("100").as_bytes() }) {
                    #[cfg(debug_assertions)]
                    eprintln!("error: client disconnected from http://{ADDR_PORT}/progress/{file_path}, or error occured: {e}")
                }
                _ = writer.flush()
            }
        }

        Ok(file_path)
    }

    fn handle_rq(&self, mut rq: Request) -> Result::<()> {
        match (rq.method(), rq.url()) {
            (&Method::Get, "/") => {
                let Some(user_agent) = get_header(&rq, "User-Agent").map(|ua| ua.value.as_str()) else {
                    return anyerr!("request to `/` with no user-agent")
                };

                if user_agent_is_mobile(&user_agent) {
                    serve_bytes(rq, HOME_MOBILE_HTML, "text/html; charset=UTF-8")
                } else {
                    serve_bytes(rq, HOME_HTML, "text/html; charset=UTF-8")
                }
            }
            (&Method::Get, "/qr.png") => serve_bytes(rq, &self.qr_png_bytes, "image/png; charset=UTF-8"),
            (&Method::Get, "/index.js") => serve_bytes(rq, HOME_SCRIPT, "application/js; charset=UTF-8"),
            (&Method::Get, "/index-mobile.js") => serve_bytes(rq, HOME_MOBILE_SCRIPT, "application/js; charset=UTF-8"),
            (&Method::Post, "/upload") => {
                match self.handle_upload(&mut rq) {
                    Ok(fp) => {
                        _ = self.clients().remove_entry(&fp.0);
                        rq.respond(Response::empty(200))
                    }
                    Err(e) => rq.respond(Response::from_string(e.to_string()).with_status_code(StatusCode(500))),
                }.map_err(Into::into)
            },
            (&Method::Get, path) => if path.starts_with("/progress") {
                let file_path = path[const { "/progress".len() + 1 }..].to_owned();
                let mut stream = rq.upgrade("SSE", self.sse_response.clone());
                stream.write_all(HTTP_OK_RESPONSE)?;
                stream.flush()?;
                println!("[INFO] client connected to <http://{ADDR_PORT}/progress/{file_path}>");
                self.clients().insert(file_path, stream);
                Ok(())
            } else {
                serve_bytes(rq, NOT_FOUND_HTML, "text/html; charset=UTF-8")
            }
            _ => Ok(())
        }
    }

    fn serve(&mut self) {
        println!("serving at: <http://{ADDR_PORT}>");
        self.server.incoming_requests().par_bridge().for_each(|rq| {
            _ = self.handle_rq(rq).inspect_err(|e| eprintln!("{e}"))
        });
    }
}

#[inline]
fn serve_bytes(request: Request, bytes: &[u8], content_type: &str) -> Result::<()> {
    let content_type_header = Header::from_bytes("Content-Type", content_type).expect("invalid header string");
    request.respond(Response::from_data(bytes).with_header(content_type_header))?;
    Ok(())
}

fn get_default_local_ip_addr() -> Option::<IpAddr> {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return None };
    if sock.connect("1.1.1.1:80").is_err() { return None }
    match sock.local_addr() {
        Ok(addr) => Some(addr.ip()),
        Err(_) => None
    }
}

fn main() -> Result::<()> {
    #[cfg(debug_assertions)]
    println!("[running in debug mode]");

    println!("looking for default local ip address..");
    let local_ip = get_default_local_ip_addr().unwrap_or_else(|| {
        panic!("could not find it..")
    });

    println!("found: {local_ip}, using it to generate qr-code..");
    let local_addr = format!("http://{local_ip}:{PORT}");
    let qr = QrCode::encode_text(&local_addr, QrCodeEcc::Low).expect("could not encode url to qr code");
    let qr_png_bytes = gen_qr_png_bytes(&qr).expect("could not generate a qr-code image");

    Server::new(qr_png_bytes).serve();

    Ok(())
}
