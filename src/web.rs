//! Companion page for image compare: a loopback HTTP server that shows the
//! locked and the live picture in a browser at full resolution, following
//! the terminal as the cursor moves (`Space w`, `:web`).
//!
//! Designed as a *capability*, not a file server: it binds `127.0.0.1` only,
//! every route lives under a random 128-bit token, and the only two
//! resources are `img/locked` and `img/live` — the paths ncoxide currently
//! holds. There is no path parameter, so there is nothing to traverse, and
//! a request whose `Host` is not this loopback origin is refused (a web page
//! you have open elsewhere cannot fetch from it by rebinding a name). Plain
//! `std::net`, one thread per connection, `Connection: close` on every
//! response: four GET routes on loopback do not justify an HTTP crate.

use std::fmt;
use std::fs::File;
use std::io::{self, Cursor, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use image::ImageFormat;

use crate::preview::image::{self as img, ImageMeta};

/// The page, inlined so the binary stays self-contained.
const PAGE: &str = include_str!("web/compare.html");
/// A request must fit in this many bytes (request line + headers).
const MAX_REQUEST: usize = 8 * 1024;

/// One picture the page can ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    pub path: PathBuf,
    pub name: String,
    /// `1920×1080 PNG · 2.3 MB`, from [`ImageMeta::summary`].
    pub summary: String,
    pub format: ImageFormat,
}

impl ImageRef {
    pub fn new(path: &Path, meta: &ImageMeta) -> Self {
        ImageRef {
            path: path.to_path_buf(),
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            summary: meta.summary(),
            format: meta.format,
        }
    }
}

/// What the page shows. `generation` changes whenever a picture does, so
/// the page reloads only then.
#[derive(Debug, Default)]
pub struct WebState {
    pub generation: u64,
    pub locked: Option<ImageRef>,
    pub live: Option<ImageRef>,
}

/// Shared between the app (publishes) and the connection threads (read).
struct Ctx {
    token: String,
    port: u16,
    state: Mutex<WebState>,
}

impl Ctx {
    fn state(&self) -> MutexGuard<'_, WebState> {
        // A poisoned lock only means a connection thread panicked mid-read;
        // the state itself is still consistent.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The `Host` values that name this server: what a browser sends when
    /// it was given our URL, and nothing else.
    fn host_ok(&self, host: Option<&str>) -> bool {
        let Some(host) = host else {
            return false;
        };
        let host = host.to_ascii_lowercase();
        host == format!("127.0.0.1:{}", self.port) || host == format!("localhost:{}", self.port)
    }
}

/// The running server. Dropping it stops the listener.
pub struct WebServer {
    url: String,
    ctx: Arc<Ctx>,
    stop: Arc<AtomicBool>,
    accept_thread: Option<JoinHandle<()>>,
}

impl fmt::Debug for WebServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebServer").field("url", &self.url).finish()
    }
}

impl WebServer {
    /// Bind `127.0.0.1:port` (`0` = any free port) and start serving.
    pub fn start(port: u16) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        let token = random_token()?;
        let url = format!("http://127.0.0.1:{port}/{token}/");
        let ctx = Arc::new(Ctx {
            token,
            port,
            state: Mutex::new(WebState::default()),
        });
        let stop = Arc::new(AtomicBool::new(false));

        let accept_thread = {
            let ctx = Arc::clone(&ctx);
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("ncoxide-web".into())
                .spawn(move || {
                    for stream in listener.incoming() {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        let Ok(stream) = stream else {
                            continue;
                        };
                        let ctx = Arc::clone(&ctx);
                        // Detached: a slow image read must not stall the
                        // page's state polls, and each one ends on its own.
                        let _ = std::thread::Builder::new()
                            .name("ncoxide-web-conn".into())
                            .spawn(move || handle(stream, &ctx));
                    }
                })?
        };
        log::info!("compare page at {url}");
        Ok(WebServer {
            url,
            ctx,
            stop,
            accept_thread: Some(accept_thread),
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn port(&self) -> u16 {
        self.ctx.port
    }

    /// Set what the page shows. Bumps the generation only on a change, so
    /// the page does not reload pictures on every keystroke. Returns true
    /// when something changed.
    pub fn publish(&self, locked: Option<ImageRef>, live: Option<ImageRef>) -> bool {
        let mut state = self.ctx.state();
        if state.locked == locked && state.live == live {
            return false;
        }
        state.locked = locked;
        state.live = live;
        state.generation += 1;
        true
    }

    /// The current generation (tests).
    pub fn generation(&self) -> u64 {
        self.ctx.state().generation
    }
}

impl Drop for WebServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking `accept` so the thread sees the flag and drops
        // the listener; the port is closed once `join` returns.
        let addr: SocketAddr = ([127, 0, 0, 1], self.ctx.port).into();
        let _ = TcpStream::connect_timeout(&addr, Duration::from_millis(200));
        if let Some(t) = self.accept_thread.take() {
            let _ = t.join();
        }
    }
}

/// 128 random bits as hex, from the kernel (this crate is Linux-only).
fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Open `url` in the user's browser when there is one on this machine:
/// never over SSH (the browser is on the other end, and `xdg-open` might
/// open one on a remote display), and only with a display present.
/// Returns whether a browser was asked.
pub fn open_in_browser(url: &str) -> bool {
    let local_display =
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
    if !browser_allowed(std::env::var_os("SSH_CONNECTION").is_some(), local_display) {
        return false;
    }
    Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

/// The rule behind [`open_in_browser`], separated so it can be tested
/// without touching the process environment.
fn browser_allowed(over_ssh: bool, local_display: bool) -> bool {
    !over_ssh && local_display
}

struct Request {
    method: String,
    target: String,
    host: Option<String>,
}

/// Read one request head (line + headers) off the socket. `None` for
/// anything malformed, oversized or cut short.
fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let end = loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = memchr::memmem::find(&buf, b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > MAX_REQUEST {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_string();
    let target = request_line.next()?.to_string();
    let host = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("host")
            .then(|| value.trim().to_string())
    });
    Some(Request {
        method,
        target,
        host,
    })
}

struct Response {
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
    location: Option<String>,
}

impl Response {
    fn text(status: &'static str, body: &str) -> Self {
        Response {
            status,
            content_type: "text/plain; charset=utf-8",
            body: body.as_bytes().to_vec(),
            location: None,
        }
    }

    fn bytes(content_type: &'static str, body: Vec<u8>) -> Self {
        Response {
            status: "200 OK",
            content_type,
            body,
            location: None,
        }
    }

    fn redirect(location: String) -> Self {
        Response {
            status: "301 Moved Permanently",
            content_type: "text/plain; charset=utf-8",
            body: Vec::new(),
            location: Some(location),
        }
    }

    fn write(self, stream: &mut TcpStream, head_only: bool) {
        let mut head = format!(
            "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
             Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
             Referrer-Policy: no-referrer\r\nConnection: close\r\n",
            self.status,
            self.content_type,
            self.body.len()
        );
        if let Some(location) = &self.location {
            head.push_str(&format!("Location: {location}\r\n"));
        }
        head.push_str("\r\n");
        let _ = stream.write_all(head.as_bytes());
        if !head_only {
            let _ = stream.write_all(&self.body);
        }
        let _ = stream.flush();
        let _ = stream.shutdown(Shutdown::Write);
    }
}

fn handle(mut stream: TcpStream, ctx: &Ctx) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let Some(req) = read_request(&mut stream) else {
        Response::text("400 Bad Request", "bad request").write(&mut stream, false);
        return;
    };
    let head_only = req.method == "HEAD";
    let response = route(ctx, &req);
    response.write(&mut stream, head_only);
}

fn route(ctx: &Ctx, req: &Request) -> Response {
    if req.method != "GET" && req.method != "HEAD" {
        return Response::text("405 Method Not Allowed", "GET only");
    }
    if !ctx.host_ok(req.host.as_deref()) {
        return Response::text("403 Forbidden", "wrong host");
    }
    let path = req.target.split('?').next().unwrap_or("");
    let prefix = format!("/{}/", ctx.token);
    if path == prefix.trim_end_matches('/') {
        // Relative URLs in the page need the trailing slash.
        return Response::redirect(prefix);
    }
    let Some(rest) = path.strip_prefix(prefix.as_str()) else {
        return Response::text("404 Not Found", "not found");
    };
    match rest {
        "" => Response::bytes("text/html; charset=utf-8", PAGE.as_bytes().to_vec()),
        "state" => Response::bytes("application/json", state_json(&ctx.state()).into_bytes()),
        "img/locked" => image_response(ctx.state().locked.clone()),
        "img/live" => image_response(ctx.state().live.clone()),
        _ => Response::text("404 Not Found", "not found"),
    }
}

fn state_json(state: &WebState) -> String {
    let image = |r: &Option<ImageRef>| match r {
        None => "null".to_string(),
        Some(r) => format!(
            "{{\"name\":{},\"path\":{},\"summary\":{}}}",
            json_str(&r.name),
            json_str(&r.path.to_string_lossy()),
            json_str(&r.summary)
        ),
    };
    format!(
        "{{\"generation\":{},\"locked\":{},\"live\":{}}}",
        state.generation,
        image(&state.locked),
        image(&state.live)
    )
}

/// A JSON string literal (quotes and escapes included).
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The media type a browser renders natively; `None` for formats we must
/// transcode (TIFF, QOI).
fn browser_type(format: ImageFormat) -> Option<&'static str> {
    match format {
        ImageFormat::Png => Some("image/png"),
        ImageFormat::Jpeg => Some("image/jpeg"),
        ImageFormat::Gif => Some("image/gif"),
        ImageFormat::WebP => Some("image/webp"),
        ImageFormat::Bmp => Some("image/bmp"),
        ImageFormat::Ico => Some("image/x-icon"),
        _ => None,
    }
}

/// The picture's own bytes when the browser can show them, else a PNG
/// transcode (under the preview's decode limits).
fn image_response(image: Option<ImageRef>) -> Response {
    let Some(image) = image else {
        return Response::text("404 Not Found", "no picture");
    };
    if !image.path.is_file() {
        // Deleted or moved since it was published; the next state poll
        // will tell the page so.
        return Response::text("404 Not Found", "gone");
    }
    match browser_type(image.format) {
        Some(content_type) => match std::fs::read(&image.path) {
            Ok(bytes) => Response::bytes(content_type, bytes),
            Err(e) => Response::text("404 Not Found", &format!("cannot read: {e}")),
        },
        None => match img::decode(&image.path) {
            Ok(decoded) => {
                let mut png = Cursor::new(Vec::new());
                match decoded.write_to(&mut png, ImageFormat::Png) {
                    Ok(()) => Response::bytes("image/png", png.into_inner()),
                    Err(e) => Response::text("500 Internal Server Error", &e.to_string()),
                }
            }
            Err(e) => Response::text("500 Internal Server Error", &e),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::Shutdown;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ncoxide_web_{}_{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_image(path: &Path, w: u32, h: u32, format: ImageFormat) {
        image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x * 6) as u8, (y * 10) as u8, 128])
        })
        .save_with_format(path, format)
        .unwrap();
    }

    fn image_ref(path: &Path) -> ImageRef {
        ImageRef::new(path, &img::probe(path).expect("is an image"))
    }

    /// A raw request; `host` of `None` sends the server's own origin.
    fn request(
        port: u16,
        method: &str,
        target: &str,
        host: Option<&str>,
    ) -> (u16, String, Vec<u8>) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let host = host
            .map(str::to_string)
            .unwrap_or(format!("127.0.0.1:{port}"));
        write!(stream, "{method} {target} HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let split = memchr::memmem::find(&raw, b"\r\n\r\n").expect("header end");
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        let status: u16 = head
            .split(' ')
            .nth(1)
            .and_then(|s| s.parse().ok())
            .expect("status");
        (status, head, raw[split + 4..].to_vec())
    }

    fn get(port: u16, target: &str) -> (u16, String, Vec<u8>) {
        request(port, "GET", target, None)
    }

    #[test]
    fn test_routes_page_state_and_pictures_under_the_token() {
        let dir = temp_dir("routes");
        let a = dir.join("a.png");
        let b = dir.join("b.jpg");
        write_image(&a, 12, 8, ImageFormat::Png);
        write_image(&b, 8, 12, ImageFormat::Jpeg);
        let server = WebServer::start(0).unwrap();
        let port = server.port();
        let token = server
            .url()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();
        assert_eq!(token.len(), 32);
        assert!(
            server
                .url()
                .starts_with(&format!("http://127.0.0.1:{port}/"))
        );

        // Nothing published yet: page and state serve, pictures are 404.
        let (status, head, body) = get(port, &format!("/{token}/"));
        assert_eq!(status, 200);
        assert!(head.contains("text/html"), "{head}");
        assert!(String::from_utf8_lossy(&body).contains("ncoxide"));
        let (status, _, body) = get(port, &format!("/{token}/state"));
        assert_eq!(status, 200);
        assert_eq!(
            String::from_utf8_lossy(&body),
            "{\"generation\":0,\"locked\":null,\"live\":null}"
        );
        assert_eq!(get(port, &format!("/{token}/img/live")).0, 404);

        // Publish: state carries names and facts, pictures are the files'
        // own bytes with the right media type.
        assert!(server.publish(Some(image_ref(&a)), Some(image_ref(&b))));
        assert!(
            !server.publish(Some(image_ref(&a)), Some(image_ref(&b))),
            "no change, no bump"
        );
        assert_eq!(server.generation(), 1);
        let (_, _, body) = get(port, &format!("/{token}/state"));
        let json = String::from_utf8_lossy(&body);
        assert!(json.contains("\"generation\":1"), "{json}");
        assert!(
            json.contains("\"name\":\"a.png\"") && json.contains("12×8 PNG"),
            "{json}"
        );
        assert!(json.contains("\"name\":\"b.jpg\""), "{json}");
        let (status, head, body) = get(port, &format!("/{token}/img/locked?g=1"));
        assert_eq!(status, 200);
        assert!(head.contains("image/png"), "{head}");
        assert_eq!(body, fs::read(&a).unwrap());
        let (status, head, body) = get(port, &format!("/{token}/img/live"));
        assert_eq!(status, 200);
        assert!(head.contains("image/jpeg"), "{head}");
        assert_eq!(body, fs::read(&b).unwrap());

        // HEAD: headers only. Missing trailing slash: redirected to it.
        let (status, head, body) = request(port, "HEAD", &format!("/{token}/img/live"), None);
        assert_eq!(status, 200);
        assert!(head.contains("image/jpeg") && body.is_empty());
        let (status, head, _) = get(port, &format!("/{token}"));
        assert_eq!(status, 301);
        assert!(head.contains(&format!("Location: /{token}/")), "{head}");

        // Unlock: the picture is gone again, generation moved on.
        assert!(server.publish(None, Some(image_ref(&b))));
        assert_eq!(get(port, &format!("/{token}/img/locked")).0, 404);
        assert_eq!(server.generation(), 2);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_refuses_everything_outside_the_capability() {
        let dir = temp_dir("refuse");
        let a = dir.join("a.png");
        write_image(&a, 4, 4, ImageFormat::Png);
        let server = WebServer::start(0).unwrap();
        server.publish(Some(image_ref(&a)), Some(image_ref(&a)));
        let port = server.port();
        let token = server
            .url()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();

        // No token, wrong token, favicon, anything not one of the routes.
        for target in [
            "/",
            "/state",
            "/img/live",
            "/favicon.ico",
            "/deadbeef/state",
        ] {
            assert_eq!(get(port, target).0, 404, "{target}");
        }
        for target in [
            "nope",
            "img/other",
            "img/live/x",
            "img/../../etc/passwd",
            "../",
        ] {
            assert_eq!(get(port, &format!("/{token}/{target}")).0, 404, "{target}");
        }
        // Paths are never taken from the request: a traversal-looking
        // target under the token cannot reach the filesystem.
        let (status, _, body) = get(port, &format!("/{token}/img/..%2F..%2Fetc%2Fpasswd"));
        assert_eq!(status, 404);
        assert!(!String::from_utf8_lossy(&body).contains("root:"));

        // Wrong Host (DNS rebinding), missing Host, and non-GET methods.
        let ok = format!("/{token}/state");
        assert_eq!(request(port, "GET", &ok, Some("evil.example:80")).0, 403);
        assert_eq!(
            request(port, "GET", &ok, Some(&format!("evil.example:{port}"))).0,
            403
        );
        assert_eq!(
            request(port, "GET", &ok, Some(&format!("LOCALHOST:{port}"))).0,
            200
        );
        assert_eq!(request(port, "POST", &ok, None).0, 405);
        assert_eq!(request(port, "DELETE", &ok, None).0, 405);

        // Garbage and oversized requests get a 400, not a panic or a hang.
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s.write_all(b"\x00\x01\x02 garbage without an end").unwrap();
        s.shutdown(Shutdown::Write).unwrap();
        let mut raw = String::new();
        s.read_to_string(&mut raw).unwrap();
        assert!(raw.starts_with("HTTP/1.1 400"), "{raw}");
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let huge = format!(
            "GET /{token}/state HTTP/1.1\r\nX: {}\r\n",
            "a".repeat(MAX_REQUEST)
        );
        let _ = s.write_all(huge.as_bytes());
        let _ = s.shutdown(Shutdown::Write);
        let mut raw = String::new();
        let _ = s.read_to_string(&mut raw);
        assert!(raw.starts_with("HTTP/1.1 400"), "{raw}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_transcodes_formats_browsers_cannot_show() {
        let dir = temp_dir("transcode");
        let q = dir.join("pic.qoi");
        write_image(&q, 10, 6, ImageFormat::Qoi);
        let server = WebServer::start(0).unwrap();
        server.publish(None, Some(image_ref(&q)));
        let port = server.port();
        let token = server
            .url()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();

        let (status, head, body) = get(port, &format!("/{token}/img/live"));
        assert_eq!(status, 200);
        assert!(head.contains("image/png"), "{head}");
        let decoded = image::load_from_memory_with_format(&body, ImageFormat::Png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (10, 6));

        // A picture that vanished after publishing: 404, not a crash — on
        // both the transcode and the raw path.
        fs::remove_file(&q).unwrap();
        assert_eq!(get(port, &format!("/{token}/img/live")).0, 404);
        let p = dir.join("gone.png");
        write_image(&p, 2, 2, ImageFormat::Png);
        server.publish(None, Some(image_ref(&p)));
        fs::remove_file(&p).unwrap();
        assert_eq!(get(port, &format!("/{token}/img/live")).0, 404);
        // A file that is there but not decodable: 500 with the reason.
        fs::write(&q, b"QOIF but not really").unwrap();
        let broken = ImageRef {
            path: q.clone(),
            name: "pic.qoi".into(),
            summary: String::new(),
            format: ImageFormat::Qoi,
        };
        server.publish(None, Some(broken));
        assert_eq!(get(port, &format!("/{token}/img/live")).0, 500);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_drop_closes_the_port_and_fixed_port_is_honoured() {
        let server = WebServer::start(0).unwrap();
        let port = server.port();
        assert!(TcpStream::connect(("127.0.0.1", port)).is_ok());
        // The port is taken while the server runs.
        assert!(WebServer::start(port).is_err());
        drop(server);
        assert!(
            TcpStream::connect(("127.0.0.1", port)).is_err(),
            "listener still open after drop"
        );
        // ... and free again afterwards.
        let again = WebServer::start(port).unwrap();
        assert_eq!(again.port(), port);
    }

    #[test]
    fn test_json_str_escapes() {
        assert_eq!(json_str("plain"), "\"plain\"");
        assert_eq!(json_str("a\"b\\c\nd\u{1}"), "\"a\\\"b\\\\c\\nd\\u0001\"");
        assert_eq!(json_str("ünïcödé ×"), "\"ünïcödé ×\"");
    }

    #[test]
    fn test_browser_only_with_a_local_display_and_never_over_ssh() {
        assert!(browser_allowed(false, true));
        assert!(
            !browser_allowed(true, true),
            "over SSH the browser is elsewhere"
        );
        assert!(
            !browser_allowed(false, false),
            "headless: print the URL instead"
        );
        assert!(!browser_allowed(true, false));
    }
}
