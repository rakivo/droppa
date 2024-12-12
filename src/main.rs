use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use std::io::{Cursor, Write};
use std::collections::HashMap;

use actix_web::{get, post};
use actix_web::HttpRequest;
use actix_multipart::Multipart;
use futures_util::StreamExt;
use qrcodegen::{QrCode, QrCodeEcc};
use futures_channel::mpsc::{channel, Sender};
use zip::{ZipWriter, write::SimpleFileOptions};
use actix_web::{App, HttpServer, HttpResponse, Responder, web::{self, Data}, middleware::Logger};

#[allow(unused_imports, unused_parens, non_camel_case_types, unused_mut, dead_code, unused_assignments, unused_variables, static_mut_refs, non_snake_case, non_upper_case_globals)]
mod stb_image_write;

mod qr;
use qr::*;

macro_rules! progress_fmt {
    ($lit: literal) => { concat!("data: { \"progress\": ", $lit, "}\n\n") };
    ($expr: expr) => { format!("data: {{ \"progress\": {} }}\n\n", $expr) };
}

macro_rules! atomic_type {
    ($(type $name: ident = $ty: ty;)*) => {$(paste::paste! {
        #[allow(unused)] type $name = $ty;
        #[allow(unused)] type [<Atomic $name>] = Arc::<Mutex::<$ty>>;
    })*};
}

macro_rules! define_addr_port {
    (const ADDR: $addr_ty: ty = $addr: literal; const PORT: $port_ty: ty = $port: literal;) => {
        #[allow(unused)] const ADDR: $addr_ty = $addr;
        #[allow(unused)] const PORT: $port_ty = $port;
        #[allow(unused)] const ADDR_PORT: &str = concat!($addr, ':', $port);
    };
}

atomic_type! {
    type Files = Vec::<File>;
    type Clients = HashMap::<String, Sender::<String>>;
}

define_addr_port! {
    const ADDR: &str = "0.0.0.0";
    const PORT: u16 = 6969;
}

pub struct File {
    pub bytes: Vec::<u8>,
    pub file_name: String
}

#[derive(Clone)]
struct AppState {
    qr_bytes: web::Bytes,
    clients: AtomicClients,
    uploaded_files: AtomicFiles,
}

const SIZE_LIMIT: usize = 1024 * 1024 * 1024;

const HOME_DESKTOP_HTML:   &[u8] = include_bytes!("index-desktop.html");
const HOME_DESKTOP_SCRIPT: &[u8] = include_bytes!("index-desktop.js");
const HOME_MOBILE_HTML:    &[u8] = include_bytes!("index-mobile.html");
const HOME_MOBILE_SCRIPT:  &[u8] = include_bytes!("index-mobile.js");

const HTTP_OK_RESPONSE: &str = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";

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

#[get("/progress/{file_name}")]
async fn track_progress(path: web::Path::<String>, state: web::Data<AppState>) -> impl Responder {
    let file_name = path.into_inner();
    
    let (mut tx, rx) = channel::<String>(32);

    tx.try_send(HTTP_OK_RESPONSE.to_owned()).unwrap();
    
    println!("[INFO] client connected to <http://localhost:8080/progress/{file_name}>");
    
    let mut clients = state.clients.lock().unwrap();
    clients.insert(file_name, tx);

    let stream = rx.map(|data| Ok::<_, actix_web::Error>(data.into()));
    HttpResponse::Ok()
        .append_header(("Content-Type", "text/event-stream"))
        .append_header(("Cache-Control", "no-cache"))
        .append_header(("Connection", "keep-alive"))
        .streaming(stream)
}

#[get("/")]
async fn index(rq: HttpRequest) -> impl Responder {
    let Some(user_agent) = rq.headers().get("User-Agent").and_then(|header| header.to_str().ok()) else {
        return HttpResponse::BadRequest().body("Request to `/` that does not contain user agent")
    };

    if user_agent_is_mobile(user_agent) {
        HttpResponse::Ok()
            .append_header(("Content-Type", "text/html"))
            .body(HOME_MOBILE_HTML)
    } else {
        HttpResponse::Ok()
            .append_header(("Content-Type", "text/html"))
            .body(HOME_DESKTOP_HTML)
    }
}

#[get("/index-mobile.js")]
async fn index_mobile_js() -> impl Responder {
    HttpResponse::Ok()
        .append_header(("Content-Type", "application/javascript; charset=UTF-8"))
        .body(HOME_MOBILE_SCRIPT)
}

#[get("/index-desktop.js")]
async fn index_desktop_js() -> impl Responder {
    HttpResponse::Ok()
        .append_header(("Content-Type", "application/javascript; charset=UTF-8"))
        .body(HOME_DESKTOP_SCRIPT)
}

#[get("/qr.png")]
async fn qr_code(state: web::Data<AppState>) -> impl Responder {
    HttpResponse::Ok()
        .content_type("image/png")
        .body(state.qr_bytes.clone())
}

#[post("/upload-desktop")]
async fn upload_desktop(mut payload: Multipart, state: web::Data<AppState>) -> impl Responder {
    println!("upload-desktop requested, getting file size..");

    let mut file_size = None;
    let mut file_name = String::new();
    let mut file_bytes = Vec::new();
    let mut clients = state.clients.lock().unwrap();

    while let Some(Ok(mut field)) = payload.next().await {
        if matches!(field.name(), Some(name) if name == "size") {
            #[cfg(debug_assertions)]
            println!("processing `size` field...");
            let mut buf = String::new();
            while let Some(Ok(chunk)) = field.next().await {
                buf.push_str(std::str::from_utf8(&chunk).unwrap());
            }
            file_size = buf.parse::<usize>().ok();
            if file_size.is_none() {
                return HttpResponse::BadRequest().body("invalid size field")
            }
            file_bytes.try_reserve_exact(file_size.unwrap() / 8).unwrap();
            #[cfg(debug_assertions)]
            println!("file size: {fs}", fs = file_size.unwrap());
        } else {
            #[cfg(debug_assertions)]
            println!("processing `file` field...");
            if let Some(content_disposition) = field.content_disposition() {
                if let Some(name) = content_disposition.get_filename() {
                    file_name = name.to_owned();
                }
            }

            let Some(size) = file_size else {
                return HttpResponse::BadRequest().body("invalid size field")
            };

            while let Some(Ok(chunk)) = field.next().await {
                file_bytes.extend_from_slice(&chunk);
                let prgrs = (file_bytes.len() * 100 / size).min(100);
                if prgrs % 5 != 0 { continue }
                let Some(tx) = clients.get_mut(&file_name) else { continue };
                if let Err(e) = tx.try_send(progress_fmt!(prgrs)) {
                    eprintln!("failed to send progress: {e}")
                }
            }
        }
    }

    if file_bytes.len() > SIZE_LIMIT {
        #[cfg(debug_assertions)]
        println!("file size exceeds limit, returning bad request..");
        return HttpResponse::BadRequest().body("file size exceeds limit")
    }

    let file = File {
        bytes: file_bytes,
        file_name: file_name.clone(),
    };

    println!("uploaded: {file_name}");

    state.uploaded_files.lock().unwrap().push(file);
    HttpResponse::Ok().body(format!("Uploaded: {file_name}"))
}

#[get("/download-files")]
async fn download_files(state: web::Data<AppState>) -> impl Responder {
    #[cfg(debug_assertions)]
    println!("download files requested, zipping them up..");

    let files = state.uploaded_files.lock().unwrap();

    let mut zip_bytes = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut zip_bytes);
    let opts = SimpleFileOptions::default();

    for file in files.iter() {
        zip.start_file(&file.file_name, opts).unwrap();
        zip.write_all(&file.bytes).unwrap();
    }

    zip.finish().unwrap();

    #[cfg(debug_assertions)]
    println!("finished zipping up the files, sending to your phone..");

    HttpResponse::Ok()
        .content_type("application/zip")
        .body(zip_bytes.into_inner())
}

fn get_default_local_ip_addr() -> Option::<std::net::IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("1.1.1.1:80").ok()?;
    sock.local_addr().ok().map(|addr| addr.ip())
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    println!("looking for default local IP address...");
    let local_ip = get_default_local_ip_addr().unwrap_or_else(|| panic!("could not find local IP address"));

    println!("found: {local_ip}, using it to generate QR code...");
    let local_addr = format!("http://{local_ip}:{PORT}");
    let qr = QrCode::encode_text(&local_addr, QrCodeEcc::Low).expect("could not encode URL to QR code");

    let state = AppState {
        clients: Arc::new(Mutex::new(HashMap::new())),
        uploaded_files: Arc::new(Mutex::new(Vec::new())),
        qr_bytes: gen_qr_png_bytes(&qr).expect("Could not generate QR code image").into(),
    };

    println!("serving at: <http://{ADDR_PORT}>");

    HttpServer::new(move || {
        App::new()
            .app_data(Data::new(state.clone()))
            .wrap(Logger::default())
            .service(index)
            .service(qr_code)
            .service(track_progress)
            .service(download_files)
            .service(upload_desktop)
            .service(index_mobile_js)
            .service(index_desktop_js)
    }).bind((ADDR, PORT))?.run().await
}
