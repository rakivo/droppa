use std::fs;
use std::future::Future;
use std::net::{IpAddr, UdpSocket};
use std::sync::{Arc, Mutex, MutexGuard};
use std::io::{Cursor, Write, BufWriter};

use serde::Serialize;
use dashmap::DashMap;
use actix_web::rt as actix_rt;
use qrcodegen::{QrCode, QrCodeEcc};
use actix_files::Files as ActixFiles;
use actix_web::{get, post, HttpRequest};
use tokio_stream::wrappers::WatchStream;
use futures_util::{StreamExt, TryStreamExt};
use actix_multipart::{Multipart, MultipartError};
use zip::{ZipWriter, CompressionMethod, write::SimpleFileOptions};
use tokio::time::{sleep as tokio_sleep, Duration as TokioDuration};
use tokio::sync::{mpsc, watch, Mutex as TokioMutex, MutexGuard as TokioMutexGuard};
use actix_web::{App, HttpServer, HttpResponse, Responder, middleware::Logger, web::{self, Path, Data}};

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
        #[track_caller]
        #[inline(always)]
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

const GIG: usize = 1024 * 1024 * 1024;
const SIZE_LIMIT: usize = GIG * 1;

const HOME_MOBILE_HTML:    &[u8] = include_bytes!("../front/index-mobile.html");
const HOME_DESKTOP_HTML:   &[u8] = include_bytes!("../front/index-desktop.html");

atomic_type! {
    type Files = Vec::<File>;
}

#[derive(Debug, Serialize)]
pub struct TrackFile {
    pub size: usize,
    pub name: String,
    pub progress: u8
}

pub struct Client {
    sender: watch::Sender::<u8>,
    progress: u8,
    mobile: bool,
    size: usize,
}

type ProgressPinger = Arc::<TokioMutex::<Option::<mpsc::Sender::<()>>>>;
type ProgressSender = Arc::<TokioMutex::<Option::<watch::Sender::<String>>>>;

type Clients = DashMap::<String, Client>;

pub struct File {
    pub size: usize,
    pub name: String,
    pub bytes: Vec::<u8>
}

impl File {
    async fn from_multipart(multipart: &mut Multipart, clients: Arc::<Clients>, pp: ProgressPinger) -> Result::<File, &'static str> {
        let mut size = None;
        let mut bytes = Vec::new();
        let mut name = String::new();
        while let Some(Ok(field)) = multipart.next().await {
            if field.name() == "size" {
                println!("[INFO] processing `size` field...");

                let buf = field.try_fold(String::new(), |mut acc, chunk| async move {
                    acc.push_str(std::str::from_utf8(&chunk).unwrap());
                    Ok(acc)
                }).await.map_err(|_| "error reading size field")?;

                size = buf.parse::<usize>().ok();
                if size.is_none() {
                    println!("[FATAL] invalid size field: {buf}");
                    return Err("invalid size field")
                }

                let size = unsafe { size.unwrap_unchecked() };
                if bytes.try_reserve_exact(size / std::mem::size_of::<u8>()).is_err() {
                    println!("[FATAL] could not reserve memory: {}", size / std::mem::size_of::<u8>());
                    return Err("could not reserve memory")
                }

                println!("[INFO] parsed file size: {size}");

                if size > SIZE_LIMIT {
                    #[cfg(feature = "dbg")] println!("file size exceeds limit, returning bad request..");
                    return Err("file size exceeds limit, returning bad request..")
                }
            } else {
                println!("[INFO] processing `file` field...");

                let Some(size) = size else {
                    println!("`size` field must go first, not the `file` one");
                    return Err("`size` field must go first, not the `file` one")
                };

                _ = field.content_disposition()
                    .get_filename()
                    .map(|name_| name = name_.to_owned());

                println!("[INFO {name}] size: {size}");

                bytes = field.try_fold((bytes, &name, &clients, &pp), |(mut bytes, name, clients, pp), chunk| async move {
                    bytes.extend_from_slice(&chunk);
                    let progress = (bytes.len() * 100 / size).min(100) as u8;
                    if progress % 5 == 0 {
                        #[cfg(feature = "dbg")] println!("[INFO {name}] copying chunk..");
                        {
                            let Some(mut ps) = clients.get_mut(name) else {
                                println!("[ERROR] no: {name} in the clients hashmap, returning an error..");
                                return Err(MultipartError::Incomplete)
                            };
                            ps.size = size;
                            ps.progress = progress;
                            if let Err(e) = ps.sender.send(progress) {
                                eprintln!("[ERROR] failed to send progress: {e}");
                            }
                        }
                        #[cfg(feature = "dbg")] println!("[INFO {name}] copied chunk, trying to lock the pinger..");
                        if let Ok(pp) = pp.try_lock() {
                            if let Some(pp) = pp.as_ref() {
                                if pp.try_send(()).is_ok() {
                                    #[cfg(feature = "dbg")] println!("[INFO {name}] pinged successfully..");
                                }
                            }
                        } else {
                            #[cfg(feature = "dbg")] println!("[INFO {name}] lock failed..");
                        }
                    }
                    Ok((bytes, name, clients, pp))
                }).await.map_err(|_| "error reading file field")?.0;
            }
        }

        Ok(File { bytes, name, size: unsafe { size.unwrap_unchecked() } })
    }
}

struct Server {
    files: AtomicFiles,
    qr_bytes: web::Bytes,
    clients: Arc::<Clients>,
    progress_tx: ProgressPinger,
    mobile_files_progress_sender: ProgressSender,
    desktop_files_progress_sender: ProgressSender
}

impl Server {
    #[inline(always)]
    fn lock_sender(&self, mobile: bool) -> impl Future::<Output = TokioMutexGuard::<Option::<watch::Sender::<String>>>> {
        if mobile {
            self.mobile_files_progress_sender.lock()
        } else {
            self.desktop_files_progress_sender.lock()
        }
    }

    #[inline(always)]
    async fn sender_send(&self, json: String, mobile: bool) {
        if let Err(e) = self.lock_sender(mobile).await.as_ref().expect("SENDER IS NOT INITIALIZED").send(json) {
            eprintln!("[FATAL] could not send JSON: {e}")
        }
    }

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
async fn track_progress(rq: HttpRequest, path: Path::<String>, state: Data::<Server>) -> impl Responder {
    let Some(user_agent) = rq.headers().get("User-Agent").and_then(|header| header.to_str().ok()) else {
        return HttpResponse::BadRequest().body("Request to `/` that does not contain user agent")
    };

    let file_name = path.into_inner();
    println!("[INFO] client connected to <http://localhost:8080/progress/{file_name}>");
    
    let tx = watch::channel(0).0;
    let rx = WatchStream::new(tx.subscribe());

    println!("[INFO] inserted: {file_name} into the clients hashmap");
    state.clients.insert(file_name, Client {
        sender: tx,
        progress: 0,
        size: 0,
        mobile: user_agent_is_mobile(user_agent)
    });

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

#[get("/qr.png")]
async fn qr_code(state: Data::<Server>) -> impl Responder {
    HttpResponse::Ok()
        .content_type("image/png")
        .body(web::Bytes::clone(&state.qr_bytes))
}

#[post("/upload-desktop")]
async fn upload_desktop(mut multipart: Multipart, state: Data::<Server>) -> impl Responder {
    println!("[INFO] upload-desktop requested, parsing multipart..");

    let File { bytes, name, size } = match File::from_multipart(&mut multipart, Arc::clone(&state.clients), Arc::clone(&state.progress_tx)).await {
        Ok(f) => f,
        Err(e) => return HttpResponse::BadRequest().body(e)
    };

    println!("[INFO] uploaded: {name}");

    {
        state.lock_files().push(File { bytes, name, size });
    }

    HttpResponse::Ok().finish()
}

#[post("/upload-mobile")]
async fn upload_mobile(mut multipart: Multipart, state: Data::<Server>) -> impl Responder {
    println!("[INFO] upload-mobile requested, parsing multipart..");

    let File { bytes, name, size } = match File::from_multipart(&mut multipart, Arc::clone(&state.clients), Arc::clone(&state.progress_tx)).await {
        Ok(f) => f,
        Err(e) => return HttpResponse::BadRequest().body(e)
    };

    #[cfg(feature = "dbg")] let mut name = name;
    #[cfg(feature = "dbg")] { name = name + ".test" }

    if let Err(e) = actix_rt::task::spawn_blocking(move || {
        let file = match fs::File::create(&name) {
            Ok(f) => f,
            Err(e) => return Err(format!("could not create file: {name}: {e}"))
        };

        println!("[INFO] copying bytes to: {name}..");

        let mut wbuf = BufWriter::with_capacity(size, file);
        _ = wbuf.write_all(&bytes).map_err(|e| {
            return format!("could not copy bytes: {name}: {e}")
        });

        println!("[INFO] uploaded: {name}");

        Ok(())
    }).await {
        return HttpResponse::SeeOther().body(format!("error copying bytes: {e}"))
    }

    HttpResponse::Ok().finish()
}

#[get("/download-files-mobile")]
async fn download_files(state: Data::<Server>) -> impl Responder {
    println!("[INFO] download files requested, zipping them up..");

    let files = Arc::clone(&state.files);
    let Ok(Ok(zip_bytes)) = actix_rt::task::spawn_blocking(move || {
        let (size, len) = {
            let files = files.lock().unwrap();
            let size = files.iter().map(|f| f.size).sum::<usize>();
            (size, files.len())
        };

        let mut zip_bytes = Cursor::new(Vec::with_capacity(size));

        let mut zip = ZipWriter::new(&mut zip_bytes);
        let mut opts = SimpleFileOptions::default()
            .compression_level(Some(8))
            .compression_method(CompressionMethod::Deflated);

        if size > const { GIG * 4 } || len > 65536 {
            opts = opts.large_file(true)
        }

        {
            let files = files.lock().unwrap();
            for File { name, bytes, .. } in files.iter() {
                zip.start_file(&name, opts)?;
                zip.write_all(&bytes)?;
            }
        }

        zip.finish().map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;

        Ok::<_, std::io::Error>(zip_bytes.into_inner())
    }).await else {
        return HttpResponse::SeeOther().body("error zipping up your files")
    };

    println!("[INFO] finished zipping up the files, sending to your phone..");
    HttpResponse::Ok()
        .content_type("application/zip")
        .body(zip_bytes)
}

async fn stream_progress(state: Data::<Server>, mobile: bool) -> impl Responder {
    let ptx = watch::channel("[]".to_owned()).0;
    let prx = WatchStream::new(ptx.subscribe());

    println!{
        "[INFO] client connected to <http://localhost:8080/download-files-progress-{device}>",
        device = if mobile { "mobile" } else { "desktop" }
    };

    {
        let files_progress_sender = &mut state.lock_sender(mobile).await;
        if files_progress_sender.is_some() {
            state.sender_send("CONNECTION_REPLACED".to_owned(), mobile).await;
            **files_progress_sender = Some(ptx);
            return HttpResponse::Ok()
                .append_header(("Content-Type", "text/event-stream"))
                .append_header(("Cache-Control", "no-cache"))
                .append_header(("Connection", "keep-alive"))
                .streaming(prx.map(|data| {
                    Ok::<_, actix_web::Error>(format!("data: {data}\n\n").into())
                }))
        }

        **files_progress_sender = Some(ptx)
    }

    let (tx, mut rx) = mpsc::channel(8);
    *state.progress_tx.lock().await = Some(tx);

    let state = Data::clone(&state);
    actix_rt::spawn(async move {
        loop {
            if rx.try_recv().is_err() {
                tokio_sleep(TokioDuration::from_millis(150)).await;
                continue
            }

            let data = state.clients.iter().filter(|p| p.mobile != mobile).map(|p| {
                TrackFile { name: p.key().to_owned(), progress: p.progress, size: p.size }
            }).collect::<Vec::<_>>();

            let json = serde_json::to_string(&data).unwrap();
            state.sender_send(json, mobile).await;
            tokio_sleep(TokioDuration::from_millis(100)).await;
        }
    });

    HttpResponse::Ok()
        .append_header(("Content-Type", "text/event-stream"))
        .append_header(("Cache-Control", "no-cache"))
        .append_header(("Connection", "keep-alive"))
        .streaming(prx.map(|data| {
            Ok::<_, actix_web::Error>(format!("data: {data}\n\n").into())
        }))
}

#[get("/download-files-progress-mobile")]
async fn download_files_progress_mobile(state: Data::<Server>) -> impl Responder {
    stream_progress(state, true).await
}

#[get("/download-files-progress-desktop")]
async fn download_files_progress_desktop(state: Data::<Server>) -> impl Responder {
    stream_progress(state, false).await
}

fn get_default_local_ip_addr() -> Option::<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("1.1.1.1:80").ok()?;
    sock.local_addr().ok().map(|addr| addr.ip())
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    println!("[INFO] looking for default local IP address...");
    let local_ip = get_default_local_ip_addr().unwrap_or_else(|| panic!("could not find local IP address"));

    println!("[INFO] found: {local_ip}, using it to generate QR code...");
    let local_addr = format!("http://{local_ip}:{PORT}");
    let qr = QrCode::encode_text(&local_addr, QrCodeEcc::Low).expect("could not encode URL to QR code");

    let server = Data::new(Server {
        clients: Arc::new(DashMap::new()),
        files: Arc::new(Mutex::new(Vec::new())),
        progress_tx: Arc::new(TokioMutex::new(None)),
        mobile_files_progress_sender: Arc::new(TokioMutex::new(None)),
        desktop_files_progress_sender: Arc::new(TokioMutex::new(None)),
        qr_bytes: gen_qr_png_bytes(&qr).expect("could not generate QR code image").into()
    });

    println!("[INFO] serving at: <http://{local_ip}:{PORT}>");

    HttpServer::new(move || {
        App::new()
            .app_data(Data::clone(&server))
            .wrap(Logger::default())
            .service(index)
            .service(qr_code)
            .service(upload_mobile)
            .service(track_progress)
            .service(download_files)
            .service(upload_desktop)
            .service(download_files_progress_mobile)
            .service(download_files_progress_desktop)
            .service(ActixFiles::new("/", "./front"))
    }).bind((local_ip.to_string(), PORT))?.run().await
}
