use std::fs::File;
use std::fmt::Display;
use std::time::Instant;
use std::mem::MaybeUninit;
use std::collections::HashMap;
use std::io::{Read, Write, BufWriter};
use std::sync::{Arc, Mutex, MutexGuard};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use anyhow::Result;
use multipart::server::Multipart;
use qrcodegen::{QrCode, QrCodeEcc};
use get_if_addrs::{IfAddr, get_if_addrs};
use socket2::{Domain, Protocol, Socket, Type};
use rayon::iter::{ParallelBridge, ParallelIterator};
use tiny_http::{ReadWrite, Header, Method, Request, Response, StatusCode, Server as TinyHttpServer};

mod qr;
use qr::*;
#[allow(unused_imports, unused_parens, non_camel_case_types, unused_mut, dead_code, unused_assignments, unused_variables, static_mut_refs, non_snake_case, non_upper_case_globals)]
mod stb_image_write;

const ICMP_ECHO_REPLY: u8 = 0;
const ICMP_ECHO_REQUEST: u8 = 8;
const TIMEOUT_DURATION: u64 = 3;
const SIZE_LIMIT: u64 = 1024 * 1024 * 1024;
const GOOGLE_IPV4: Ipv4Addr = Ipv4Addr::from_bits(0x08080808);

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

fn ping_interface(interface_name: &String, src_ip: Ipv4Addr, dest_ip: Ipv4Addr) -> std::io::Result<bool> {
    fn compute_checksum(data: &[u8]) -> u16 {
        let mut sum = 0u32;
        let chunks = data.chunks_exact(2);
        if let Some(&last_byte) = chunks.remainder().first() {
            sum += (last_byte as u32) << 8;
        }

        sum += chunks.into_iter()
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]) as u32)
            .sum::<u32>();

        while (sum >> 16) > 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }

        !(sum as u16)
    }

    println!("pinging from interface {interface_name} (IP: {src_ip}) to {dest_ip}");

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::ICMPV4))?;
    let source_addr = SocketAddr::V4(SocketAddrV4::new(src_ip, 0));
    socket.bind(&source_addr.into())?;

    let mut packet = [0u8; 64];
    packet[0] = ICMP_ECHO_REQUEST;
    packet[1] = 0;
    packet[2..4].copy_from_slice(&[0, 0]);

    let identifier = 0x1234u16;
    let sequence = 0x1u16;
    packet[4..6].copy_from_slice(&identifier.to_be_bytes());
    packet[6..8].copy_from_slice(&sequence.to_be_bytes());

    let checksum = compute_checksum(&packet);
    packet[2..4].copy_from_slice(&checksum.to_be_bytes());

    let dst = SocketAddr::V4(SocketAddrV4::new(dest_ip, 0)).into();
    socket.send_to(&packet, &dst)?;

    let start_time = Instant::now();

    let mut buffer: [MaybeUninit::<u8>; 1024] = unsafe { MaybeUninit::uninit().assume_init() };
    loop {
        if start_time.elapsed().as_secs() >= TIMEOUT_DURATION {
            #[cfg(debug_assertions)]
            println!("timeout: no reply within 1 second.");
            return Ok(false)
        }

        let sender = socket.recv_from(&mut buffer)?.1;
        let buffer = unsafe { &*(buffer.as_mut_ptr() as *mut [u8; 1024]) };

        let sockv4 = sender.as_socket_ipv4().unwrap();
        let sender_ip = sockv4.ip();

        if sender_ip != &dest_ip { continue }

        let reply_type = buffer[20];
        if reply_type != ICMP_ECHO_REPLY { continue }

        println!("received ICMP echo reply from {sender_ip} in {:.3} ms", start_time.elapsed().as_millis());
        println!("found fitting interface, generating qr-code..");

        return Ok(true)
    }
}

fn find_loopback_addr() -> Option::<Ipv4Addr> {
    get_if_addrs().ok()?.into_iter().find_map(|iface| {
        if iface.is_loopback() { return None }
        match &iface.addr {
            IfAddr::V4(v4) if ping_interface(&iface.name, v4.ip, GOOGLE_IPV4).is_ok_and(|b| b) => {
                Some(v4.ip)
            }
            _ => None // TODO: check ipv6 interfaces
        }
    })
}

fn main() -> Result::<()> {
    println!("looking for non-loopback ipv4 interface with an internet access..");
    let local_ip = find_loopback_addr().unwrap_or_else(|| {
        panic!("could not find it..")
    });

    let local_addr = format!("http://{local_ip:?}:{PORT}");
    let qr = QrCode::encode_text(&local_addr, QrCodeEcc::Low).expect("could not encode url to qr code");
    let qr_png_bytes = gen_qr_png_bytes(&qr).expect("could not generate a qr-code image");

    Server::new(qr_png_bytes).serve();

    Ok(())
}
