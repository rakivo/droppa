use std::fs;
use std::path::PathBuf;
use std::future::Future;
use std::net::{IpAddr, UdpSocket};
use std::io::{Cursor, Write, BufWriter};
use std::sync::{Arc, Mutex, MutexGuard};

use dashmap::DashMap;
use actix_web::rt as actix_rt;
use qrcodegen::{QrCode, QrCodeEcc};
use serde::{Deserialize, Serialize};
use actix_files::Files as ActixFiles;
use actix_web::{get, post, HttpRequest};
use futures_util::{StreamExt, TryStreamExt};
use actix_multipart::{Multipart, MultipartError};
use tokio_stream::wrappers::{WatchStream, BroadcastStream};
use zip::{ZipWriter, CompressionMethod, write::SimpleFileOptions};
use tokio::time::{sleep as tokio_sleep, Duration as TokioDuration};
use tokio::sync::{mpsc, watch, broadcast, Mutex as TokioMutex, MutexGuard as TokioMutexGuard};
use actix_web::{App, HttpServer, HttpResponse, Responder, middleware::Logger, web::{self, Path, Data, Query}};

#[allow(unused_imports, unused_parens, non_camel_case_types, unused_mut, dead_code, unused_assignments, unused_variables, static_mut_refs, non_snake_case, non_upper_case_globals)]
mod stb_image_write;

mod qr;
use qr::*;

macro_rules! atomic_type {
    ($(type $name: ident = $ty: ty;)*) => {$(paste::paste! {
        #[allow(unused)] type $name = $ty;
        #[allow(unused)] type [<Atomic $name>] = Arc::<Mutex::<$ty>>;
    })*};
    ($(tokio.type $name: ident = $ty: ty;)*) => {$(paste::paste! {
        #[allow(unused)] type $name = $ty;
        #[allow(unused)] type [<Atomic $name>] = Arc::<TokioMutex::<$ty>>;
    })*};
    ($(arc.type $name: ident = $ty: ty;)*) => {$(paste::paste! {
        #[allow(unused)] type $name = $ty;
        #[allow(unused)] type [<Atomic $name>] = Arc::<$ty>;
    })*};
}

macro_rules! lock_fn {
    ($($field: tt), *) => { $(paste::paste! {
        #[track_caller] #[inline(always)]
        fn [<lock_ $field>](&self) -> MutexGuard::<[<$field:camel>]> {
            self.$field.lock().unwrap()
        }
    })*};
}

const PORT: u16 = 6969;

const GIG: usize = 1024 * 1024 * 1024;
const SIZE_LIMIT: usize = GIG * 1;

const DELIM: &str = if cfg!(windows) { "\\" } else { "/" };

const DROPPA_DOWNLOADS_DIR: &str = "droppa_files";

const HOME_MOBILE_HTML:  &[u8] = include_bytes!("../front/index-mobile.html");
const HOME_DESKTOP_HTML: &[u8] = include_bytes!("../front/index-desktop.html");

#[derive(Debug, Serialize)]
pub struct TrackFile {
    size: usize,
    name: String,
    progress: u8
}

#[derive(Debug)]
pub struct Client {
    size: usize,
    progress: u8,
    mobile: bool,
    sender: watch::Sender::<u8>,
    ip: Box::<str>,
    uuid: Box::<str>,
}

atomic_type! {
    type Files = Vec::<File>;
    type SyncProgressSender = Option::<mpsc::Sender::<u8>>;
}

atomic_type! {
    tokio.type ProgressPinger = Option::<mpsc::Sender::<()>>;
    tokio.type ProgressStreamer = Option::<watch::Sender::<String>>;
    tokio.type DevicesBroadcaster = broadcast::Sender::<String>;
}

atomic_type! {
    arc.type Clients = DashMap::<String, Client>;
}

pub struct File {
    size: usize,
    name: String,
    bytes: Vec::<u8>
}

impl File {
    async fn from_multipart(multipart: &mut Multipart, clients: AtomicClients, pp: AtomicProgressPinger) -> Result::<File, &'static str> {
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
                let u8_reserve = size / std::mem::size_of::<u8>();
                if bytes.try_reserve_exact(size / u8_reserve).is_err() {
                    println!("[FATAL] could not reserve memory: {u8_reserve}");
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

                match field.content_disposition().get_filename() {
                    Some(name_) => name = name_.to_owned(),
                    _ => return Err("`file` field does not have a filename")
                }

                println!("[INFO {name}] size: {size}");

                bytes = field.try_fold((bytes, &name, &clients, &pp), |(mut bytes, name, clients, pp), chunk| async move {
                    bytes.extend_from_slice(&chunk);
                    let progress = (bytes.len() * 100 / size).min(100) as u8;
                    if progress % 5 == 0 {
                        let Some(mut ps) = clients.get_mut(name) else {
                            println!("[ERROR] no: {name} in the clients hashmap, returning an error..");
                            return Err(MultipartError::Incomplete)
                        };

                        ps.size = size;
                        ps.progress = progress;

                        if let Err(e) = ps.sender.send(progress) {
                            eprintln!("[ERROR] failed to send progress: {e}");
                        }

                        if let Ok(pp) = pp.try_lock() {
                            if let Some(pp) = pp.as_ref() {
                                _ = pp.try_send(()).ok()
                            }
                        }
                    }
                    Ok((bytes, name, clients, pp))
                }).await.map_err(|_| "error reading file field")?.0;
            }
        }

        Ok(File { bytes, name, size: unsafe { size.unwrap_unchecked() } })
    }
}

#[repr(u8)]
#[derive(Copy, Clone)]
enum Transmission { Mobile, Zipping, Desktop }

struct Server {
    connected: Arc::<DashMap::<Box::<str>, Box::<str>>>,

    qr_bytes: web::Bytes,

    downloads_dir: PathBuf,

    files: AtomicFiles,
    clients: AtomicClients,

    files_progress_pinger: AtomicProgressPinger,
    connected_devices_pinger: AtomicProgressPinger,

    zipping_progress_sender: AtomicSyncProgressSender,

    zipping_progress_streamer: AtomicProgressStreamer,
    connected_devices_streamer: AtomicDevicesBroadcaster,
    mobile_files_progress_streamer: AtomicProgressStreamer,
    desktop_files_progress_streamer: AtomicProgressStreamer
}

impl Server {
    #[inline(always)]
    fn lock_streamer(&self, transmission: Transmission) -> impl Future::<Output = TokioMutexGuard::<ProgressStreamer>> {
        use Transmission::*;
        match transmission {
            Mobile  => self.mobile_files_progress_streamer.lock(),
            Zipping => self.zipping_progress_streamer.lock(),
            Desktop => self.desktop_files_progress_streamer.lock()
        }
    }

    #[inline(always)]
    async fn streamer_send(&self, json: String, transmission: Transmission) {
        if let Err(e) = self.lock_streamer(transmission).await.as_ref().expect("SENDER IS NOT INITIALIZED").send(json) {
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

#[derive(Deserialize)]
struct Uuid {
    uuid: String
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UuidDeviceName {
    device_name: String,
    uuid: String,
}

#[get("/progress/{file_name}")]
async fn track_progress(state: Data::<Server>, rq: HttpRequest, path: Path::<String>, query: Query::<Uuid>) -> impl Responder {
    let Some(user_agent) = rq.headers().get("User-Agent").and_then(|header| header.to_str().ok()) else {
        return HttpResponse::BadRequest().body("Request to `/` that does not contain user agent")
    };

    let file_name = path.into_inner();
    println!("[INFO] client connected to <http://localhost:{PORT}/progress/{file_name}>");
    
    let tx = watch::channel(0).0;
    let rx = WatchStream::new(tx.subscribe());

    println!("[INFO] inserted: {file_name} into the clients hashmap");
    state.clients.insert(file_name, Client {
        size: 0,
        sender: tx,
        progress: 0,
        mobile: user_agent_is_mobile(user_agent),
        uuid: query.uuid.to_owned().into_boxed_str(),
        ip: rq.peer_addr().unwrap().ip().to_string().into_boxed_str(),
    });

    HttpResponse::Ok()
        .keep_alive()
        .content_type("text/event-stream")
        .append_header(("Cache-Control", "no-cache"))
        .streaming(rx.map(|data| {
            Ok::<_, actix_web::Error>(format!("data: {{ \"progress\": {data} }}\n\n").into())
        }))
}

#[get("/")]
async fn index(rq: HttpRequest) -> impl Responder {
    let Some(user_agent) = rq.headers().get("User-Agent").and_then(|header| header.to_str().ok()) else {
        return HttpResponse::BadRequest().body("Request to `/` that does not contain user agent")
    };

    HttpResponse::Ok()
        .content_type("text/html")
        .body(if user_agent_is_mobile(user_agent) {HOME_MOBILE_HTML} else {HOME_DESKTOP_HTML}) 
}

#[get("/qr.png")]
async fn qr_code(state: Data::<Server>) -> impl Responder {
    HttpResponse::Ok()
        .content_type("image/png")
        .body(web::Bytes::clone(&state.qr_bytes))
}

#[post("/upload-desktop")]
async fn upload_desktop(state: Data::<Server>, mut multipart: Multipart, query: Query::<Uuid>) -> impl Responder {
    println!("[INFO] upload-desktop requested, parsing multipart..");

    let file = match File::from_multipart(&mut multipart, Arc::clone(&state.clients), Arc::clone(&state.files_progress_pinger)).await {
        Ok(f) => f,
        Err(e) => return HttpResponse::BadRequest().body(e)
    };

    println!("[INFO] uploaded: {name}", name = file.name);

    {
        state.lock_files().push(file);
    }

    HttpResponse::Ok().finish()
}

#[post("/upload-mobile")]
async fn upload_mobile(state: Data::<Server>, mut multipart: Multipart, query: Query::<Uuid>) -> impl Responder {
    println!("[INFO] upload-mobile requested, parsing multipart..");

    let File { bytes, name, size } = match File::from_multipart(&mut multipart, Arc::clone(&state.clients), Arc::clone(&state.files_progress_pinger)).await {
        Ok(f) => f,
        Err(e) => return HttpResponse::BadRequest().body(e)
    };

    #[cfg(feature = "dbg")] let mut name = name;
    #[cfg(feature = "dbg")] { name = name + ".test" }

    /* TODO:
        We will have a `connected` hashmap here, and we'll check the ip of device current `uuid` is "connected" to,
        and if the ip matches we would just write the file right off the bat.

        If the ip does not match, we would do kinda the same thing we would do with the `upload-desktop` thing.
        We would just append the file to the `files` vector and wait till the client pressed the `download` button,
        then send the compressed files in zip.
    */

    if let Err(e) = actix_rt::task::spawn_blocking(move || {
        let file_path = format!{
            "{downloads}{DELIM}{name}",
            downloads = state.downloads_dir.display()
        };

        let file = match fs::File::create(&file_path) {
            Ok(f) => f,
            Err(e) => return Err(format!("could not create file: {name}: {e}"))
        };

        println!("[INFO] copying bytes to: {file_path}..");

        let mut wbuf = BufWriter::with_capacity(size, file);
        if let Err(e) = wbuf.write_all(&bytes) {
            return Err(format!("could not copy bytes: {name}: {e}"))
        }

        println!("[INFO] uploaded: {name}");

        Ok(())
    }).await {
        return HttpResponse::SeeOther().body(format!("error copying bytes: {e}"))
    }

    HttpResponse::Ok().finish()
}

struct ProgressTracker<W: Write> {
    writer: W,
    written: usize,
    total_size: usize,
    progress_sender: AtomicSyncProgressSender
}

impl<W: Write> ProgressTracker::<W> {
    #[inline(always)]
    pub fn new(writer: W, total_size: usize, progress_sender: AtomicSyncProgressSender) -> Self {
        Self { writer, written: 0, total_size, progress_sender }
    }

    #[inline(always)]
    pub fn progress(&self) -> usize {
        (self.written * 100 / self.total_size).min(100)
    }
}

impl<W: Write> Write for ProgressTracker::<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result::<usize> {
        let written_ = self.writer.write(buf)?;
        self.written += written_;

        let p = self.progress();
        if p % 5 == 0 {
            let progress_sender = self.progress_sender.lock().unwrap();
            progress_sender.as_ref().map(|ps| ps.try_send(p as _));
        }

        Ok(written_)
    }

    #[inline(always)]
    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
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

        {
            let mut opts = SimpleFileOptions::default()
                .compression_level(Some(8))
                .compression_method(CompressionMethod::Deflated);

            if size > const { GIG * 4 } || len > 65536 {
                opts = opts.large_file(true)
            }

            let mut zip = ProgressTracker::new(ZipWriter::new(&mut zip_bytes), size, Arc::clone(&state.zipping_progress_sender));
            {
                let files = files.lock().unwrap();
                for File { name, bytes, .. } in files.iter() {
                    zip.writer.start_file(&name, opts)?;
                    zip.write_all(&bytes)?
                }
            }

            zip.writer.finish().map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
            })?;
        }

        Ok::<_, std::io::Error>(zip_bytes.into_inner())
    }).await else {
        return HttpResponse::SeeOther().body("error zipping up your files")
    };

    println!("[INFO] finished zipping up the files, sending to your phone..");
    HttpResponse::Ok()
        .content_type("application/zip")
        .body(zip_bytes)
}

//////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
// I could've used the `FnOnce` and `FnMut` traits here and called different async closures do to different things, //
// but it seems that this feature is really, really underdeveloped yet.                                             //
//                                                                                                                  //
// I've tried to use `Pin` and `Box` and `Future` but it could not help, the compiler was unsatifisfied with the    //
// lifetimes, because I need to accept the state and the ping_receiver by a reference.                              //
//                                                                                                                  //
// I think can make it compile, but the complexity the codebase will gain is certainly not worth the effort.        //
//                                                                                                                  //
// I value simplicity, so I decided to use an enum.                                                                 //
//////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

async fn stream_progress(state: Data::<Server>, transmission: Transmission) -> impl Responder {
    use Transmission::*;

    let ptx = watch::channel("[]".to_owned()).0;
    let streamer = WatchStream::new(ptx.subscribe());

    {
        let progress_streamer = &mut state.lock_streamer(transmission).await;
        if progress_streamer.is_some() {
            progress_streamer.as_ref().unwrap().send("CONNECTION_REPLACED".to_owned()).unwrap();
            **progress_streamer = Some(ptx);
            return HttpResponse::Ok()
                .append_header(("Content-Type", "text/event-stream"))
                .append_header(("Cache-Control", "no-cache"))
                .append_header(("Connection", "keep-alive"))
                .streaming(streamer.map(|data| {
                    Ok::<_, actix_web::Error>(format!("data: {data}\n\n").into())
                }))
        }

        **progress_streamer = Some(ptx)
    }

    match transmission {
        Zipping => {
            let (tx, mut rx) = mpsc::channel(8);
            *state.zipping_progress_sender.lock().unwrap() = Some(tx);

            let state = Data::clone(&state);
            actix_rt::spawn(async move {
                loop {
                    if let Ok(progress) = rx.try_recv() {
                        let streamer = state.zipping_progress_streamer.lock().await;
                        let streamer = streamer.as_ref().unwrap();
                        _ = streamer.send(format!("{{ \"progress\": {progress} }}"));
                        tokio_sleep(TokioDuration::from_millis(100)).await
                    } else {
                        tokio_sleep(TokioDuration::from_millis(150)).await
                    }
                }
            })
        }
        _ => {
            let (tx, mut rx) = mpsc::channel(8);
            *state.files_progress_pinger.lock().await = Some(tx);

            let state = Data::clone(&state);
            actix_rt::spawn(async move {
                loop {
                    if rx.try_recv().is_err() {
                        tokio_sleep(TokioDuration::from_millis(150)).await;
                        continue
                    }

                    let mobile = matches!(transmission, Mobile);
                    let data = state.clients.iter().filter(|p| p.mobile != mobile).map(|p| {
                        TrackFile { name: p.key().to_owned(), progress: p.progress, size: p.size }
                    }).collect::<Vec::<_>>();

                    let json = serde_json::to_string(&data).unwrap();

                    state.streamer_send(json, transmission).await;
                    tokio_sleep(TokioDuration::from_millis(100)).await;
                }
            })
        }
    };

    HttpResponse::Ok()
        .append_header(("Content-Type", "text/event-stream"))
        .append_header(("Cache-Control", "no-cache"))
        .append_header(("Connection", "keep-alive"))
        .streaming(streamer.map(|data| {
            Ok::<_, actix_web::Error>(format!("data: {data}\n\n").into())
        }))
}

#[get("/download-files-progress-mobile")]
async fn download_files_progress_mobile(state: Data::<Server>) -> impl Responder {
    stream_progress(state, Transmission::Mobile).await
}

#[get("/download-files-progress-desktop")]
async fn download_files_progress_desktop(state: Data::<Server>) -> impl Responder {
    stream_progress(state, Transmission::Desktop).await
}

#[get("/zipping-progress")]
async fn zipping_progress(state: Data::<Server>) -> impl Responder {
    stream_progress(state, Transmission::Zipping).await
}

#[get("/connected-devices")]
async fn connected_devices(state: Data::<Server>) -> impl Responder {
    if state.connected_devices_pinger.lock().await.is_some() {
        let streamer = BroadcastStream::new(state.connected_devices_streamer.lock().await.subscribe());
        return HttpResponse::Ok()
            .keep_alive()
            .content_type("text/event-stream")
            .append_header(("Cache-Control", "no-cache"))
            .streaming(streamer.map(|data| {
                Ok::<_, actix_web::Error>(format!("data: {}\n\n", data.unwrap()).into())
            }))
    }

    let (tx, mut rx) = mpsc::channel(8);
    *state.connected_devices_pinger.lock().await = Some(tx);

    let state_ = Data::clone(&state);
    actix_rt::spawn(async move {
        loop {
            if rx.try_recv().is_ok() {
                let streamer = state.connected_devices_streamer.lock().await;
                let connected = state.connected.iter().map(|s| Box::clone(&*s.key())).collect::<Vec::<_>>();
                let json = serde_json::to_string(&connected).unwrap();
                _ = streamer.send(json);
                tokio_sleep(TokioDuration::from_millis(100)).await
            } else {
                tokio_sleep(TokioDuration::from_millis(150)).await
            }
        }
    });

    let streamer = BroadcastStream::new(state_.connected_devices_streamer.lock().await.subscribe());
    return HttpResponse::Ok()
        .keep_alive()
        .content_type("text/event-stream")
        .append_header(("Cache-Control", "no-cache"))
        .streaming(streamer.map(|data| {
            Ok::<_, actix_web::Error>(format!("data: {}\n\n", data.unwrap()).into())
        }))
}

#[post("/init-device")]
async fn init_device(state: Data::<Server>, query: Query::<UuidDeviceName>) -> impl Responder {
    state.connected.insert(query.uuid.to_owned().into_boxed_str(), query.device_name.to_owned().into_boxed_str());
    if let Some(sender) = state.connected_devices_pinger.lock().await.as_ref() {
        _ = sender.send(()).await
    }
    HttpResponse::Ok().finish()
}

#[post("/uninit-device")]
async fn uninit_device(state: Data::<Server>, query: Query::<UuidDeviceName>) -> impl Responder {
    state.connected.remove(&query.uuid.to_owned().into_boxed_str());
    if let Some(sender) = state.connected_devices_pinger.lock().await.as_ref() {
        _ = sender.send(()).await
    }
    HttpResponse::Ok().finish()
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
        connected: Arc::new(DashMap::new()),

        qr_bytes: gen_qr_png_bytes(&qr).expect("could not generate QR code image").into(),

        downloads_dir: {
            let mut dir = dirs::download_dir().expect("could not get user's `Downloads` directory");
            dir.push(DROPPA_DOWNLOADS_DIR);

            if !dir.exists() {
                fs::create_dir(&dir).expect("could not create `droppa` downloads sub-directory")
            } dir
        },

        files: Arc::new(Mutex::new(Vec::new())),
        clients: Arc::new(DashMap::new()),

        files_progress_pinger: Arc::new(TokioMutex::new(None)),

        connected_devices_pinger: Arc::new(TokioMutex::new(None)),

        zipping_progress_sender: Arc::new(Mutex::new(None)),

        zipping_progress_streamer: Arc::new(TokioMutex::new(None)),
        connected_devices_streamer: Arc::new(TokioMutex::new(broadcast::channel(64).0)),
        mobile_files_progress_streamer: Arc::new(TokioMutex::new(None)),
        desktop_files_progress_streamer: Arc::new(TokioMutex::new(None)),
    });

    println!("[INFO] serving at: <http://{local_ip}:{PORT}>");

    HttpServer::new(move || {
        App::new()
            .app_data(Data::clone(&server))
            .wrap(Logger::default())
            .service(index)
            .service(qr_code)
            .service(init_device)
            .service(uninit_device)
            .service(upload_mobile)
            .service(upload_desktop)
            .service(track_progress)
            .service(download_files)
            .service(zipping_progress)
            .service(connected_devices)            
            .service(download_files_progress_mobile)
            .service(download_files_progress_desktop)
            .service(ActixFiles::new("/", "./front"))
    }).bind((local_ip.to_string(), PORT))?.run().await
}
