use std::fs::File;
use std::sync::Arc;
use std::fmt::Display;
use std::net::TcpStream;
use std::io::{Write, BufWriter};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use raylib_light::CloseWindow;
use multipart::server::Multipart;
use qrcodegen::{QrCode, QrCodeEcc};
use get_if_addrs::{IfAddr, get_if_addrs};
use rayon::iter::{ParallelBridge, ParallelIterator};
use tiny_http::{Response, Request, Header, Server, Method, StatusCode};

mod qr;
use qr::*;

const SIZE_LIMIT: u64 = 1024 * 1024 * 1024;
const DUMMY_HTTP_RQ: &[u8] = b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

const NOT_FOUND_HTML: &[u8] = b"<h1>NOT FOUND</h1>";
const HOME_HTML: &[u8] = include_bytes!("index.html");

type AtomicStop = Arc::<AtomicBool>;

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

struct FilePath(String);

impl FilePath { const MAX_LEN: usize = 25; }

impl Display for FilePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let split = self.0.len().min(Self::MAX_LEN);
        write!{
            f,
            "{s}{dots}",
            s = &self.0[..split],
            dots = if self.0.len() > Self::MAX_LEN { "..." } else { "" }
        }
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

fn handle_upload(rq: &mut Request) -> Result::<()> {
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
    
    while let Some(mut field) = multipart.read_entry()? {
        let Some(file_path) = field.headers.filename.map(FilePath) else { continue };
        println!("[{file_path}] creating file");
        let file = File::create(&file_path.0)?;
        let wbuf = BufWriter::new(file);
        println!("[{file_path}] copying data..");
        field.data.save().size_limit(SIZE_LIMIT).write_to(wbuf);
        println!("[{file_path}] done!");
    }

    Ok(())
}

fn server_serve(server: &Server, stop: &AtomicStop) {
    println!("serving at: <http://{ADDR_PORT}>");
    server.incoming_requests().par_bridge().for_each(|mut rq| {
        if stop.load(Ordering::SeqCst) {
            println!("[INFO] Shutting down the server.");
            server.unblock()
        }

        if let Err(err) = match (rq.method(), rq.url()) {
            (&Method::Get, "/") => serve_bytes(rq, HOME_HTML, "text/html; charset=UTF-8"),
            (&Method::Post, "/upload") => {
                match handle_upload(&mut rq) {
                    Err(e) => rq.respond(Response::from_string(e.to_string()).with_status_code(StatusCode(500))),
                    _ => rq.respond(Response::from_string("OK").with_status_code(StatusCode(200))),
                }.map_err(Into::into)
            },
            _ => serve_bytes(rq, NOT_FOUND_HTML, "text/html; charset=UTF-8")
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
