use std::fs;
use std::borrow::BorrowMut;
use std::net::{IpAddr, UdpSocket};
use std::sync::{Arc, Mutex, MutexGuard};
use std::io::{Cursor, Write, BufWriter};

use dashmap::DashMap;
use tokio::sync::watch;
use actix_multipart::Multipart;
use qrcodegen::{QrCode, QrCodeEcc};
use actix_web::{get, post, HttpRequest};
use tokio_stream::wrappers::WatchStream;
use futures_util::{StreamExt, TryStreamExt};
use zip::{ZipWriter, write::SimpleFileOptions};
use actix_web::{App, HttpServer, HttpResponse, Responder, middleware::Logger, web::{self, Data, Bytes}};

#[allow(unused_imports, unused_parens, non_camel_case_types, unused_mut, dead_code, unused_assignments, unused_variables, static_mut_refs, non_snake_case, non_upper_case_globals)]
mod stb_image_write;

mod qr;
use qr::*;

macro_rules! atomic_type {
    ($(type $name: ident = $ty: ty;)*) => {$(paste::paste! {
        #[allow(unused)] type $name = $ty;
        #[allow(unused)] type [<Atomic $name>] = Arc::<Mutex::<$ty>>;
    })*};
}

macro_rules! lock_fn {
    ($($field: tt), *) => { $(paste::paste! {
        #[inline]
        #[track_caller]
        fn [<lock_ $field>](&self) -> MutexGuard::<[<$field:camel>]> {
            self.$field.lock().unwrap()
        }
    })*};
}

macro_rules! define_addr_port {
    (const ADDR: $addr_ty: ty = $addr: literal; const PORT: $port_ty: ty = $port: literal;) => {
        #[allow(unused)] const ADDR: $addr_ty = $addr;
        #[allow(unused)] const PORT: $port_ty = $port;
        #[allow(unused)] const ADDR_PORT: &str = concat!($addr, ':', $port);
    };
}

define_addr_port! {
    const ADDR: &str = "0.0.0.0";
    const PORT: u16 = 6969;
}

const SIZE_LIMIT: usize = 1024 * 1024 * 1024;

const HOME_DESKTOP_HTML:   &[u8] = include_bytes!("index-desktop.html");
const HOME_DESKTOP_SCRIPT: &[u8] = include_bytes!("index-desktop.js");
const HOME_MOBILE_HTML:    &[u8] = include_bytes!("index-mobile.html");
const HOME_MOBILE_SCRIPT:  &[u8] = include_bytes!("index-mobile.js");

atomic_type! {
    type Files = Vec::<File>;
}

type Clients = DashMap::<String, watch::Sender::<u8>>;

pub struct File {
    pub name: String,
    pub bytes: Vec::<u8>
}

impl File {
    async fn from_multipart(multipart: &mut Multipart, mut clients: Arc::<Clients>) -> Result::<File, &'static str> {
        let mut size = None;
        let mut bytes = Vec::new();
        let mut name = String::new();
        while let Some(Ok(field)) = multipart.next().await {
            if field.name() == "size" {
                #[cfg(debug_assertions)]
                println!("processing `size` field...");
                let buf = field.try_fold(String::new(), |mut acc, chunk| async move {
                    acc.push_str(std::str::from_utf8(&chunk).unwrap());
                    Ok(acc)
                }).await.map_err(|_| "error reading size field")?;
                size = buf.parse::<usize>().ok();
                if size.is_none() {
                    return Err("invalid size field")
                }
                if let Err(..) = bytes.try_reserve_exact(size.unwrap() / 8) {
                    return Err("could not reserve memory")
                }
                #[cfg(debug_assertions)]
                println!("file size: {fs}", fs = size.unwrap());

                if size.unwrap() > SIZE_LIMIT {
                    #[cfg(debug_assertions)]
                    println!("file size exceeds limit, returning bad request..");
                    return Err("file size exceeds limit, returning bad request..")
                }
            } else {
                #[cfg(debug_assertions)]
                println!("processing `file` field...");

                if let Some(name_) = field.content_disposition().get_filename() {
                    name = name_.to_owned()
                }

                let Some(size) = size else {
                    return Err("invalid size field")
                };

                bytes = field.try_fold((bytes, name.to_owned(), clients.borrow_mut()), |(mut bytes, name, clients), chunk| async move {
                    bytes.extend_from_slice(&chunk);
                    let progress = (bytes.len() * 100 / size).min(100);
                    if progress % 5 == 0 {
                    	let Some(tx) = clients.get_mut(&name) else {
                    	    println!("no: {name} in the clients hashmap, returning an error..");
                    	    return Err(actix_multipart::MultipartError::Incomplete)
                    	};
                    	if let Err(e) = tx.send(progress as u8) {
                    	    eprintln!("failed to send progress: {e}");
                    	}
                    }
                    Ok((bytes, name, clients))
                }).await.map_err(|_| "error reading file field")?.0;
            }
        }

        Ok(File {bytes, name})
    }
}

#[derive(Clone)]
struct Server {
    qr_bytes: Bytes,
    files: AtomicFiles,
    clients: Arc::<Clients>
}

impl Server {
    lock_fn! { files }
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

#[get("/progress/{file_name}")]
async fn track_progress(path: web::Path::<String>, state: Data::<Server>) -> impl Responder {
    let (tx, ..) = watch::channel(0);
    
    let file_name = path.into_inner();

    println!("[INFO] client connected to <http://localhost:8080/progress/{file_name}>");
    
    let rx = WatchStream::new(tx.subscribe());
    tx.send(0).unwrap();

    {
        println!("[INFO] inserted: {file_name} into the clients hashmap");
        state.clients.insert(file_name, tx);
    }

    HttpResponse::Ok()
        .append_header(("Content-Type", "text/event-stream"))
        .append_header(("Cache-Control", "no-cache"))
        .append_header(("Connection", "keep-alive"))
        .streaming(rx.map(|data| {
            Ok::<_, actix_web::Error>(format!("data: {{ \"progress\": {data} }}\n\n").into())
        }))
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
async fn qr_code(state: Data::<Server>) -> impl Responder {
    HttpResponse::Ok()
        .content_type("image/png")
        .body(state.qr_bytes.clone())
}

#[post("/upload-desktop")]
async fn upload_desktop(mut multipart: Multipart, state: Data::<Server>) -> impl Responder {
    println!("[INFO] upload-desktop requested, parsing multipart..");

    let File { bytes, name } = match File::from_multipart(&mut multipart, Arc::clone(&state.clients)).await {
        Ok(f) => f,
        Err(e) => return HttpResponse::BadRequest().body(e)
    };

    println!("[INFO] uploaded: {name}");

    {
        state.lock_files().push(File { bytes, name });
    }

    HttpResponse::Ok().finish()
}

#[post("/upload-mobile")]
async fn upload_mobile(mut multipart: Multipart, state: Data::<Server>) -> impl Responder {
    println!("[INFO] upload-mobile requested, parsing multipart..");

    let mut size = None;
    while let Some(Ok(field)) = multipart.next().await {
        if field.name() == "size" {
            #[cfg(debug_assertions)]
            println!("processing `size` field...");
            let Ok(buf) = field.try_fold(String::new(), |mut acc, chunk| async move {
                acc.push_str(std::str::from_utf8(&chunk).unwrap());
                Ok(acc)
            }).await else {
                return HttpResponse::BadRequest().body("error reading `size` field")
            };
            size = buf.parse::<usize>().ok();
            if size.is_none() {
                return HttpResponse::BadRequest().body("invalid size field")
            }
            #[cfg(debug_assertions)]
            println!("file size: {fs}", fs = size.unwrap());
            if size.unwrap() > SIZE_LIMIT {
                #[cfg(debug_assertions)]
                println!("file size exceeds limit, returning bad request..");
                return HttpResponse::BadRequest().body("file size exceeds limit")
            }
        } else {
            #[cfg(debug_assertions)]
            println!("processing `file` field...");

            let Some(name) = field.content_disposition().get_filename().map(ToOwned::to_owned) else {
                return HttpResponse::BadRequest().body("invalid `file` field, no name")
            };

            #[cfg(debug_assertions)] let mut name = name;
            #[cfg(debug_assertions)] { name = name + ".test" }

            let Some(size) = size else {
                return HttpResponse::BadRequest().body("`file` field should go after `size` field")
            };

            let Ok(file) = fs::File::create(&name) else {
                return HttpResponse::BadRequest().body(format!("could not create file: {name}"))
            };

            let mut clients = Arc::clone(&state.clients);

            println!("[INFO] copying bytes to: {name}..");

            if let Err(e) = field.try_fold((BufWriter::new(file), 0, &name, clients.borrow_mut()), |(mut wbuf, mut written, name, clients), chunk| async move {
                match wbuf.write(&chunk) {
                    Ok(count) => written += count,
                    Err(e) => {
                  	    eprintln!("failed to write bytes to: {name}: {e}");
                 	    return Err(actix_multipart::MultipartError::Incomplete)
                    }
                }

                let progress = (written * 100 / size).min(100);
                if progress % 5 == 0 {
                    let Some(tx) = clients.get_mut(name) else {
                    	eprintln!("no: {name} in the clients hashmap, returning an error..");
                    	return Err(actix_multipart::MultipartError::Incomplete)
                    };
                    if let Err(e) = tx.send(progress as u8) {
                    	eprintln!("failed to send progress: {e}");
                    }
                }

                Ok((wbuf, written, name, clients))
            }).await {
                return HttpResponse::BadRequest().body(format!("could not copy bytes to: {name}: {e}"))
            }
        }
    }

    HttpResponse::Ok().finish()
}

#[get("/download-files")]
async fn download_files(state: web::Data::<Server>) -> impl Responder {
    println!("[INFO] download files requested, zipping them up..");

    let files = Arc::clone(&state.files);
    let Ok(Ok(zip_bytes)) = tokio::task::spawn_blocking(move || {
        let mut zip_bytes = Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(&mut zip_bytes);
        let opts = SimpleFileOptions::default();

        for file in files.lock().unwrap().iter() {
            zip.start_file(&file.name, opts)?;
            zip.write_all(&file.bytes)?;
        }

        zip.finish().map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;

        Ok::<Vec::<u8>, std::io::Error>(zip_bytes.into_inner())
    }).await else {
        return HttpResponse::SeeOther().body("error zipping up your files")
    };

    println!("[INFO] finished zipping up the files, sending to your phone..");

    HttpResponse::Ok()
        .content_type("application/zip")
        .body(zip_bytes)
}

fn get_default_local_ip_addr() -> Option::<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("1.1.1.1:80").ok()?;
    sock.local_addr().ok().map(|addr| addr.ip())
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    println!("looking for default local IP address...");
    let local_ip = get_default_local_ip_addr().unwrap_or_else(|| panic!("could not find local IP address"));

    println!("found: {local_ip}, using it to generate QR code...");
    let local_addr = format!("http://{local_ip}:{PORT}");
    let qr = QrCode::encode_text(&local_addr, QrCodeEcc::Low).expect("could not encode URL to QR code");
    println!("serving at: <http://{ADDR_PORT}>");

    let server = Data::new(Server {
        clients: Arc::new(DashMap::new()),
        files: Arc::new(Mutex::new(Vec::new())),
        qr_bytes: gen_qr_png_bytes(&qr).expect("Could not generate QR code image").into()
    });

    HttpServer::new(move || {
        App::new().app_data(server.clone())
            .wrap(Logger::default())
            .service(index)
            .service(qr_code)
            .service(track_progress)
            .service(download_files)
            .service(upload_mobile)
            .service(upload_desktop)
            .service(index_mobile_js)
            .service(index_desktop_js)
    }).bind((ADDR, PORT))?.run().await
}
