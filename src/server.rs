//! Dedicated web server for rendering uploaded Minecraft worlds.
//!
//! Started automatically when the binary is launched with no launch arguments
//! (see `main`). It serves a small upload page, accepts a world either as a
//! `.zip` archive or as a plain folder (multipart upload), renders it in a
//! background thread through the same pipeline as the CLI, and lets the
//! browser poll progress and download the resulting PNG (single image) or a
//! zip archive of the chunk PNGs.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use indicatif::ProgressBar;
use tiny_http::{Header, Method, Request, Response, ResponseBox, Server, StatusCode};
use zip::{ZipArchive, ZipWriter};

use crate::pipeline::{run_render, scan_dimension_stats, validate_config, RenderConfig};

/// Port the dedicated server listens on.
const PORT: u16 = 7878;
/// Maximum size of the whole multipart body (bytes) — 2 GiB.
const MAX_BODY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Maximum number of entries in an uploaded zip archive.
const MAX_ARCHIVE_ENTRIES: usize = 200_000;
/// Hard ceiling on total extracted bytes (zip-bomb guard) — 20 GiB.
const MAX_EXTRACTED_BYTES: u64 = 20 * 1024 * 1024 * 1024;
/// How long a finished job (and its temporary output) is kept for download.
const JOB_RETENTION: Duration = Duration::from_secs(30 * 60);
/// How long an uploaded-but-unrendered world ("ready" job) is kept before its
/// temporary workspace is dropped.
const READY_RETENTION: Duration = Duration::from_secs(15 * 60);

/// The rendered artifact of a job, ready to be downloaded.
#[derive(Clone)]
struct JobOutput {
    /// Single-image mode: the PNG path plus its pixel dimensions.
    single: Option<(PathBuf, u32, u32)>,
    /// Per-chunk mode: the directory holding the chunk PNGs.
    chunk_dir: Option<PathBuf>,
    /// Suggested base file name (e.g. "world.png" or "the_nether.png").
    file_name: String,
}

/// In-memory state for one render job, shared between the server loop (which
/// reads it) and the background render thread (which writes it).
struct Job {
    id: String,
    /// "ready" | "pending" | "working" | "done" | "error".
    status: Mutex<String>,
    /// Human-readable stage message.
    message: Mutex<String>,
    /// Last render error, if any.
    error: Mutex<Option<String>>,
    /// The rendered output, once done.
    output: Mutex<Option<JobOutput>>,
    /// When the job finished (used to decide when it can be reaped).
    finished: Option<Instant>,
    /// When the job was created (used to reap worlds that were uploaded but
    /// never rendered).
    created: Instant,
    /// The temporary workspace; removed when the job is reaped.
    workspace: Option<PathBuf>,
    /// The prepared world root (set after upload), used by `/render`.
    world_dir: Option<PathBuf>,
    /// A progress-bar clone sharing state with the render thread's bar, so the
    /// server can read live position/length.
    progress: ProgressBar,
}

impl Job {
    fn set_status(&self, status: &str, message: &str) {
        *self.status.lock().unwrap() = status.to_string();
        *self.message.lock().unwrap() = message.to_string();
    }
}

/// A registry of all in-flight and recently-finished jobs.
struct JobRegistry {
    jobs: Mutex<HashMap<String, Job>>,
    counter: AtomicUsize,
}

impl JobRegistry {
    fn new() -> Self {
        JobRegistry {
            jobs: Mutex::new(HashMap::new()),
            counter: AtomicUsize::new(0),
        }
    }

    fn create_id(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        format!(
            "{:06x}{:04x}",
            (n ^ std::process::id() as usize) & 0xffffff,
            n & 0xffff
        )
    }

    fn insert(&self, job: Job) {
        self.jobs.lock().unwrap().insert(job.id.clone(), job);
    }

    /// Acquire a job and apply a mutation to it.
    fn with_job(&self, id: &str, f: impl FnOnce(&mut Job)) {
        let mut guard = self.jobs.lock().unwrap();
        if let Some(job) = guard.get_mut(id) {
            f(job);
        }
    }

    fn get_output(&self, id: &str) -> Option<JobOutput> {
        let guard = self.jobs.lock().unwrap();
        guard.get(id).and_then(|job| job.output.lock().unwrap().clone())
    }

    /// Remove finished jobs whose output has been kept long enough, and
    /// uploaded-but-unrendered jobs that have sat idle too long.
    fn reap_finished(&self, retention: Duration) {
        let mut guard = self.jobs.lock().unwrap();
        let now = Instant::now();
        let stale: Vec<String> = guard
            .iter()
            .filter(|(_, job)| {
                let st = job.status.lock().unwrap();
                match st.as_str() {
                    "done" | "error" => job
                        .finished
                        .map(|t| t.elapsed() > retention)
                        .unwrap_or(false),
                    "ready" => now.duration_since(job.created) > READY_RETENTION,
                    _ => false,
                }
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            if let Some(job) = guard.remove(&id) {
                if let Some(ws) = job.workspace {
                    let _ = fs::remove_dir_all(ws);
                }
            }
        }
    }
}

/// Launch the dedicated server and serve requests until the process exits.
///
/// This is the entry point used when the binary is launched with no launch
/// arguments. It never returns on its own; the process is terminated by the
/// operator (or by closing the console window).
pub fn run_server() -> Result<(), String> {
    let addr = format!("0.0.0.0:{PORT}");
    let listener = Server::http(&addr).map_err(|e| {
        format!("failed to bind the dedicated server to {addr} (is the port in use?): {e}")
    })?;
    eprintln!("worldraw: dedicated server listening on http://localhost:{PORT}/");
    eprintln!("         (render progress appears in this console; open the URL in a browser)");

    let registry = Arc::new(JobRegistry::new());

    // Serve requests one at a time (tiny_http's `accept` is sequential). This
    // keeps the code straightforward and is plenty for a local upload tool.
    for request in listener.incoming_requests() {
        handle_request(&registry, request);
    }
    Ok(())
}

/// Route a single HTTP request to the matching handler.
fn handle_request(registry: &Arc<JobRegistry>, mut request: Request) {
    let url = request.url().to_string();
    let (raw_path, query) = match url.split_once('?') {
        Some((p, q)) => (p, q),
        None => (url.as_str(), ""),
    };
    let path = url_decode(raw_path).unwrap_or_else(|| raw_path.to_string());
    let query = query.to_string();

    let (response, reap) = match (request.method(), path.as_str()) {
        (Method::Get, "/") => (
            Response::from_string(index_html())
                .with_header(text_header())
                .boxed(),
            false,
        ),
        (Method::Get, "/health") => (json_body(r#"{"status":"ok"}"#, StatusCode(200)), false),
        (Method::Post, "/upload") => (upload_response(registry, &mut request), false),
        (Method::Post, "/render") => (render_response(registry, &query), false),
        (Method::Get, "/progress") => (progress_response(registry, &query), false),
        (Method::Get, "/result") => (result_response(registry, &query), true),
        _ => (
            Response::from_string("Not found")
                .with_status_code(StatusCode(404))
                .boxed(),
            false,
        ),
    };

    // Send the response first (this blocks until it has been written to the
    // client), then clean up a job whose output has just been downloaded.
    if let Err(e) = request.respond(response) {
        eprintln!("worldraw: failed to send a response: {e}");
    }
    if reap {
        registry.reap_finished(JOB_RETENTION);
    }
}

/// `Content-Type: text/html; charset=utf-8`
fn text_header() -> Header {
    Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap()
}

/// `Content-Type: application/json; charset=utf-8`
fn json_header() -> Header {
    Header::from_bytes("Content-Type", "application/json; charset=utf-8").unwrap()
}

fn json_body(body: &str, status: StatusCode) -> ResponseBox {
    Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(json_header())
        .boxed()
}

fn json_error(status: StatusCode, message: &str) -> ResponseBox {
    json_body(&format!(r#"{{"error":{}}}"#, json_string(message)), status)
}

/// `/progress?job=ID` → live progress + status, as JSON.
fn progress_response(registry: &Arc<JobRegistry>, query: &str) -> ResponseBox {
    let id = query_value(query, "job").unwrap_or_default();
    let guard = registry.jobs.lock().unwrap();
    let Some(job) = guard.get(&id) else {
        return json_error(StatusCode(404), "unknown job");
    };
    let status = job.status.lock().unwrap().clone();
    let message = job.message.lock().unwrap().clone();
    let error = job.error.lock().unwrap().clone();
    let has_result = job
        .output
        .lock()
        .unwrap()
        .as_ref()
        .map(|o| o.single.is_some() || o.chunk_dir.is_some())
        .unwrap_or(false);
    let current = job.progress.position();
    let total = job.progress.length().unwrap_or(0);
    drop(guard);

    let pct = if total > 0 {
        (current as f64 / total as f64 * 10000.0).round() / 100.0
    } else {
        0.0
    };
    let err_field = match &error {
        Some(e) => format!(r#", "error": {}"#, json_string(e)),
        None => String::new(),
    };
    let body = format!(
        r#"{{"status": {}, "message": {}, "current": {}, "total": {}, "pct": {}{err_field}, "has_result": {}}}"#,
        json_string(&status),
        json_string(&message),
        current,
        total,
        pct,
        has_result,
    );
    json_body(&body, StatusCode(200))
}

/// `/result?job=ID` → the rendered PNG or a zip of chunk PNGs.
fn result_response(registry: &Arc<JobRegistry>, query: &str) -> ResponseBox {
    let id = query_value(query, "job").unwrap_or_default();
    let Some(output) = registry.get_output(&id) else {
        return json_error(StatusCode(404), "no result for this job (yet)");
    };

    if let Some((path, width, height)) = &output.single {
        return match fs::File::open(path) {
            Ok(file) => Response::from_file(file)
                .with_status_code(StatusCode(200))
                .with_header(png_header())
                .with_header(disposition(&output.file_name))
                .with_header(
                    Header::from_bytes("X-Image-Width", width.to_string()).unwrap_or(json_header()),
                )
                .with_header(
                    Header::from_bytes("X-Image-Height", height.to_string()).unwrap_or(json_header()),
                )
                .boxed(),
            Err(e) => json_error(
                StatusCode(500),
                &format!("failed to read the rendered image: {e}"),
            ),
        };
    }

    if let Some(dir) = &output.chunk_dir {
        let zip_name = PathBuf::from(&output.file_name).with_extension("zip");
        let out = dir.join(&zip_name);
        let zip_file_name = zip_name.to_str().unwrap_or("chunks.zip");
        return match zip_chunk_dir(dir, &out) {
            Ok(()) => match fs::File::open(&out) {
                Ok(file) => Response::from_file(file)
                    .with_status_code(StatusCode(200))
                    .with_header(zip_header())
                    .with_header(disposition(zip_file_name))
                    .boxed(),
                Err(e) => json_error(
                    StatusCode(404),
                    &format!("result file disappeared: {e}"),
                ),
            },
            Err(e) => json_error(
                StatusCode(500),
                &format!("failed to build chunk zip: {e}"),
            ),
        };
    }

    json_error(StatusCode(500), "job has no downloadable output")
}

/// `Content-Type: image/png` header.
fn png_header() -> Header {
    Header::from_bytes("Content-Type", "image/png").unwrap()
}

/// `Content-Type: application/zip` header.
fn zip_header() -> Header {
    Header::from_bytes("Content-Type", "application/zip").unwrap()
}

/// A `Content-Disposition` header that names the downloaded file.
fn disposition(name: &str) -> Header {
    let value = format!("attachment; filename=\"{name}\"");
    Header::from_bytes("Content-Disposition", value.as_str()).unwrap()
}

/// Zip every top-level PNG in `dir` into `out` (used for per-chunk results).
fn zip_chunk_dir(dir: &Path, out: &Path) -> std::io::Result<()> {
    let file = fs::File::create(out)?;
    let mut writer = ZipWriter::new(file);
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("png") {
            let name = path.file_name().unwrap().to_str().unwrap_or("chunk.png").to_string();
            writer
                .start_file(name, zip::write::FileOptions::default())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
            writer.write_all(&fs::read(&path)?)?;
        }
    }
    writer.finish()?;
    Ok(())
}

/// Minimal JSON string encoder (sufficient for our small, controlled payloads).
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
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

/// Extract `key=value` from a query string (the first match), URL-decoded.
fn query_value(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(url_decode(v).unwrap_or_else(|| v.to_string()));
            }
        }
    }
    None
}

/// Parse a whole `key=value&key=value` query string into a URL-decoded map.
/// Used by `/render`, which sends the render config as query parameters.
fn parse_query_fields(query: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            let key = url_decode(k).unwrap_or_else(|| k.to_string());
            let value = url_decode(v).unwrap_or_else(|| v.to_string());
            map.insert(key, value);
        }
    }
    map
}

fn url_decode(input: &str) -> Option<String> {
    let bytes: Vec<u8> = input.bytes().collect();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = hex_val(bytes[i + 1])?;
                let lo = hex_val(bytes[i + 2])?;
                out.push(hi * 16 + lo);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Run the render for an existing job in a background thread, updating that
/// job's progress bar and status as it goes. The thread is detached; it keeps
/// running until the render finishes or fails.
fn render_thread(registry: Arc<JobRegistry>, id: String, pb: ProgressBar, config: RenderConfig) {
    registry.with_job(&id, |job| job.set_status("working", "rendering…"));
    let result = run_render(&config, &pb);
    pb.finish();
    match result {
        Ok(r) => {
            let output = JobOutput {
                single: r
                    .single_image
                    .as_ref()
                    .map(|p| (p.clone(), r.width, r.height)),
                chunk_dir: r.chunk_dir.clone(),
                file_name: r.out_file_name.to_string(),
            };
            registry.with_job(&id, |job| {
                job.output.lock().unwrap().replace(output);
                job.finished = Some(Instant::now());
                job.set_status("done", "complete");
            });
        }
        Err(e) => {
            registry.with_job(&id, |job| {
                job.error.lock().unwrap().replace(e);
                job.finished = Some(Instant::now());
                job.set_status("error", "failed");
            });
        }
    }
}

/// Create an in-memory job for an already-prepared world, in the "ready" state
/// (world on disk, awaiting a `/render` request). No render thread is started.
fn create_ready_job(
    registry: &Arc<JobRegistry>,
    workspace: PathBuf,
    world_dir: PathBuf,
) -> String {
    let id = registry.create_id();
    let job = Job {
        id: id.clone(),
        status: Mutex::new("ready".to_string()),
        message: Mutex::new("world ready".to_string()),
        error: Mutex::new(None),
        output: Mutex::new(None),
        finished: None,
        created: Instant::now(),
        workspace: Some(workspace),
        world_dir: Some(world_dir),
        progress: ProgressBar::new(0),
    };
    registry.insert(job);
    id
}

/// Start the background render for a "ready" job. Returns `false` when the job
/// is unknown (or already gone), so the caller can report an error.
fn start_render(registry: &Arc<JobRegistry>, id: &str, config: RenderConfig) -> bool {
    let pb = {
        let guard = registry.jobs.lock().unwrap();
        guard.get(id).map(|job| job.progress.clone())
    };
    let Some(pb) = pb else {
        return false;
    };
    let registry_bg = Arc::clone(registry);
    let id_bg = id.to_string();
    std::thread::Builder::new()
        .name(format!("render-{id}"))
        .spawn(move || render_thread(registry_bg, id_bg, pb, config))
        .is_ok()
}

/// `POST /upload` — parse the multipart form, prepare the world, start a job.
fn upload_response(registry: &Arc<JobRegistry>, request: &mut Request) -> ResponseBox {
    let mut body: Vec<u8> = Vec::new();
    if let Err(e) = request
        .as_reader()
        .take(MAX_BODY_BYTES + 1)
        .read_to_end(&mut body)
    {
        return json_error(StatusCode(400), &format!("failed to read upload body: {e}"));
    }
    if body.len() as u64 > MAX_BODY_BYTES {
        return json_error(StatusCode(413), "upload is too large (max 2 GiB)");
    }

    // The multipart boundary lives in the `Content-Type` header
    // ("multipart/form-data; boundary=..."), not in the body itself.
    let content_type = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("content-type"))
        .map(|h| h.value.as_str().to_string());

    let parsed = match parse_multipart(&body, content_type.as_deref()) {
        Ok(parsed) => parsed,
        Err(msg) => return json_error(StatusCode(400), &msg),
    };
    let Multipart { fields: _, parts } = parsed;

    let workspace =
        PathBuf::from(std::env::temp_dir()).join(format!("worldraw-{}", registry.create_id()));
    if let Err(e) = fs::create_dir_all(&workspace) {
        return json_error(StatusCode(500), &format!("failed to create workspace: {e}"));
    }

    let prepared = match prepare_world(&parts, &workspace) {
        Ok(root) => root,
        Err(msg) => {
            let _ = fs::remove_dir_all(&workspace);
            return json_error(StatusCode(400), &msg);
        }
    };
    let world_dir = match find_world_root(&prepared) {
        Some(root) => root,
        None => {
            let _ = fs::remove_dir_all(&workspace);
            return json_error(
                StatusCode(400),
                "no Minecraft world found in the upload (looked for a level.dat or a region folder)",
            );
        }
    };

    // Outputs go to a dedicated subfolder so per-chunk zips only contain PNGs.
    let output_dir = workspace.join("out");
    if let Err(e) = fs::create_dir_all(&output_dir) {
        let _ = fs::remove_dir_all(&workspace);
        return json_error(
            StatusCode(500),
            &format!("failed to create the output directory: {e}"),
        );
    }

    // Measure the world (fast, index-only) for every dimension so the browser
    // can preview any of them — and switch dimension — without re-uploading.
    let dims = [("overworld", 0i32), ("nether", 1i32), ("the_end", -1i32)];
    let mut dims_json = String::new();
    for (i, (name, id)) in dims.iter().enumerate() {
        let s = scan_dimension_stats(&world_dir, *id);
        if i > 0 {
            dims_json.push(',');
        }
        dims_json.push_str(&format!(
            r#"{}:{{"chunks":{},"blocks_w":{},"blocks_h":{},"region_files":{}}}"#,
            json_string(name),
            s.chunks,
            s.blocks_w,
            s.blocks_h,
            s.region_files,
        ));
    }

    // The world is prepared and measured; hold it in a "ready" job. The actual
    // render is a separate `POST /render` so the user can review the size
    // estimate (and tweak settings) before committing to it.
    let id = create_ready_job(registry, workspace, world_dir.clone());
    let body = format!(
        r#"{{"job":{},"world":{},"dimensions":{{{dims_json}}}}}"#,
        json_string(&id),
        json_string(&world_dir.display().to_string()),
    );
    json_body(&body, StatusCode(201))
}

/// `POST /render?job=ID&<config>` — start a render for an uploaded ("ready")
/// job. The config travels as query parameters so the existing `build_config`
/// (and its validation) is reused for the request.
fn render_response(registry: &Arc<JobRegistry>, query: &str) -> ResponseBox {
    let id = query_value(query, "job").unwrap_or_default();
    let fields = parse_query_fields(query);

    let (world_dir, output_dir) = {
        let guard = registry.jobs.lock().unwrap();
        match guard.get(&id) {
            Some(job) => match (job.world_dir.clone(), job.workspace.clone()) {
                (Some(wd), Some(ws)) => (Some(wd), Some(ws.join("out"))),
                _ => (None, None),
            },
            None => (None, None),
        }
    };
    let (Some(world_dir), Some(output_dir)) = (world_dir, output_dir) else {
        return json_error(StatusCode(404), "unknown job (is the world still uploaded?)");
    };

    let config = match build_config(&fields, world_dir, output_dir) {
        Ok(config) => config,
        Err(msg) => return json_error(StatusCode(400), &msg),
    };

    if !start_render(registry, &id, config) {
        return json_error(StatusCode(404), "unknown job (is the world still uploaded?)");
    }
    json_body(&format!(r#"{{"job":{}}}"#, json_string(&id)), StatusCode(200))
}

/// Turn the multipart form fields into a `RenderConfig`, mirroring the CLI's
/// validation rules (see `args.rs`).
fn build_config(
    fields: &HashMap<String, String>,
    world_path: PathBuf,
    output_dir: PathBuf,
) -> Result<RenderConfig, String> {
    let get = |key: &str| fields.get(key).map(|s| s.trim()).filter(|s| !s.is_empty());
    let flag = |key: &str| matches!(get(key), Some(v) if v == "on" || v == "true" || v == "1");

    let dim = match get("dimension").unwrap_or("overworld") {
        "0" | "overworld" => 0,
        "1" | "nether" | "the_nether" => 1,
        "-1" | "end" | "the_end" => -1,
        other => {
            return Err(format!(
                "unknown dimension '{other}' (use overworld, nether, or the_end)"
            ))
        }
    };

    let single = match get("mode") {
        None | Some("single") => true,
        Some("chunks") | Some("chunk") => false,
        Some(other) => return Err(format!("unknown mode '{other}' (use single or chunks)")),
    };

    let scale = match get("scale") {
        None => 1,
        Some(s) => s
            .parse::<u32>()
            .ok()
            .filter(|n| *n >= 1)
            .ok_or_else(|| "scale must be a positive integer".to_string())?,
    };

    // The web UI sends a single `sampling` radio (normal / supersample /
    // hypersample), so only one sampling mode is ever active at a time. Legacy
    // direct-POST clients that still send the old boolean `supersample` /
    // `hypersample` fields are honored when no `sampling` value is present.
    let (supersample, hypersample) = match get("sampling") {
        Some("supersample") | Some("ss") => (true, false),
        Some("hypersample") | Some("hs") => (false, true),
        Some(_) => (false, false), // "normal" (or any other value)
        None => (flag("supersample"), flag("hypersample")),
    };

    let config = RenderConfig {
        world_path,
        dim,
        single,
        scale,
        shadows: flag("shadows"),
        supersample,
        hypersample,
        ambient_occlusion: flag("ao"),
        bloom: flag("bloom"),
        transparency: flag("transparency"),
        night: flag("night"),
        output_dir,
    };
    validate_config(&config)?;
    Ok(config)
}

/// A file part from the multipart form.
struct FilePart {
    file_name: Option<String>,
    data: Vec<u8>,
}

/// The parsed multipart form body.
struct Multipart {
    fields: HashMap<String, String>,
    parts: Vec<FilePart>,
}

/// Split a raw multipart body into its text fields and file parts.
fn parse_multipart(body: &[u8], content_type: Option<&str>) -> Result<Multipart, String> {
    let boundary = extract_boundary(content_type.unwrap_or(""))
        .ok_or_else(|| "not a multipart/form-data body (missing boundary)".to_string())?;
    let mut multipart = Multipart {
        fields: HashMap::new(),
        parts: Vec::new(),
    };
    for raw in split_multipart(body, &boundary) {
        let (headers, content) = split_headers_content(raw);
        let name = extract_header(headers, "name")
            .and_then(|n| unquote(&n))
            .unwrap_or_default();
        let file_name = extract_header(headers, "filename").as_deref().and_then(unquote);
        if file_name.is_some() {
            multipart.parts.push(FilePart {
                file_name,
                data: content.to_vec(),
            });
        } else {
            multipart.fields.insert(
                name,
                String::from_utf8_lossy(content).to_string(),
            );
        }
    }
    Ok(multipart)
}

/// Find the `boundary=` parameter of a `Content-Type` header value, minus any
/// surrounding quotes.
fn extract_boundary(content_type: &str) -> Option<String> {
    // Lowercase only to locate the `boundary=` key case-insensitively. The
    // boundary value itself is case-sensitive and must be taken verbatim from
    // the original header, since MIME boundaries are matched byte-for-byte.
    let lower = content_type.to_ascii_lowercase();
    let key = "boundary=";
    let at = lower.find(key)? + key.len();
    let rest = &content_type[at..];
    let end = rest.find(|c: char| c == '"' || c == ';' || c == ' ').unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

/// Split a multipart body on `--boundary`, skipping the preamble and the
/// terminating `--`.
fn split_multipart<'a>(body: &'a [u8], boundary: &str) -> Vec<&'a [u8]> {
    let delim = format!("--{boundary}");
    let mut parts: Vec<&[u8]> = Vec::new();
    let mut from = 0usize;
    loop {
        match find_bytes_from(body, delim.as_bytes(), from) {
            Some(pos) => {
                let rest = &body[pos + delim.len()..];
                if rest.starts_with(b"--") {
                    break;
                }
                from = pos + delim.len();
                if let Some(next) = find_bytes_from(body, delim.as_bytes(), from) {
                    // Strip the framing CRLF that separates parts.
                    let end = if next >= 2 && &body[next - 2..next] == b"\r\n" {
                        next - 2
                    } else if next >= 1 && &body[next - 1..next] == b"\n" {
                        next - 1
                    } else {
                        next
                    };
                    if from < end {
                        parts.push(&body[from..end]);
                    }
                    from = next;
                } else {
                    break;
                }
            }
            None => break,
        }
    }
    parts
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    find_bytes_from(haystack, needle, 0)
}

fn find_bytes_from(haystack: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let last = haystack.len() - needle.len();
    (start..=last).find(|&i| &haystack[i..i + needle.len()] == needle)
}

/// Split a part into its header block and its content (on the first blank line).
fn split_headers_content(part: &[u8]) -> (&[u8], &[u8]) {
    const CRLFCRLF: &[u8] = b"\r\n\r\n";
    const LFLF: &[u8] = b"\n\n";
    if let Some(pos) = find_bytes(part, CRLFCRLF) {
        (&part[..pos], &part[pos + CRLFCRLF.len()..])
    } else if let Some(pos) = find_bytes(part, LFLF) {
        (&part[..pos], &part[pos + LFLF.len()..])
    } else {
        (part, &[])
    }
}

/// Read a header value (case-insensitive) from a header block.
fn extract_header(headers: &[u8], key: &str) -> Option<String> {
    let needle_owned = format!("{key}=");
    let needle = needle_owned.as_bytes();
    for line in headers.split(|&b| b == b'\n') {
        let mut end = line.len();
        while end > 0 && line[end - 1] == b'\r' {
            end -= 1;
        }
        let line = &line[..end];
        if let Some(pos) = find_bytes(line, needle) {
            let value = &line[pos + needle.len()..];
            return Some(String::from_utf8_lossy(value).trim().to_string());
        }
    }
    None
}

fn unquote(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        Some(trimmed[1..trimmed.len() - 1].to_string())
    } else {
        Some(trimmed.to_string())
    }
}

/// Region folders that mark a directory as a world root (stable Minecraft
/// conventions; mirrors the candidates in `dimension.rs`).
const CANDIDATE_REGION_DIRS: &[&str] = &[
    "region",
    "DIM1/region",
    "DIM-1/region",
    "dimensions/minecraft:the_nether/region",
    "dimensions/minecraft:the_end/region",
];

/// Materialize the uploaded world parts inside `workspace` and return the
/// directory that was prepared (the unzipped archive dir, or the directory
/// holding the uploaded files).
fn prepare_world(parts: &[FilePart], workspace: &Path) -> Result<PathBuf, String> {
    if parts.is_empty() {
        return Err("no world files were uploaded (add a folder or a .zip)".to_string());
    }

    if parts.len() == 1 {
        let part = &parts[0];
        let is_zip = part
            .file_name
            .as_deref()
            .map(|n| n.to_lowercase().ends_with(".zip"))
            .unwrap_or(false)
            || looks_like_zip(&part.data);
        if is_zip {
            let zip_path = workspace.join("uploaded.zip");
            fs::write(&zip_path, &part.data)
                .map_err(|e| format!("failed to save the uploaded archive: {e}"))?;
            return extract_zip_to(&zip_path, &workspace.join("unzipped"));
        }
        return Err(
            "a single non-zip file is not a valid world (upload a folder or a .zip)".to_string(),
        );
    }

    let dir = workspace.join("upload");
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create the upload directory: {e}"))?;
    for part in parts {
        let Some(name) = part.file_name.as_deref() else {
            continue;
        };
        let Some(target) = safe_join(&dir, name) else {
            return Err(format!("skipping an uploaded file with an unsafe name: {name}"));
        };
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("failed to create a directory: {e}"))?;
        }
        fs::write(&target, &part.data)
            .map_err(|e| format!("failed to write {}: {e}", target.display()))?;
    }
    Ok(dir)
}

/// Find the world root inside `prepared`: the directory that actually contains
/// a `level.dat` or a region folder, whether that's `prepared` itself or a
/// single nested wrapper directory.
fn find_world_root(prepared: &Path) -> Option<PathBuf> {
    if is_world_root(prepared) {
        return Some(prepared.to_path_buf());
    }
    // If the upload is a single folder wrapper ("myworld/..."), descend into it.
    let entries: Vec<PathBuf> = fs::read_dir(prepared)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    if entries.len() == 1 {
        let candidate = &entries[0];
        if is_world_root(candidate) {
            return Some(candidate.clone());
        }
    }
    None
}

/// A directory counts as a world root if it holds a `level.dat` or a region folder.
fn is_world_root(dir: &Path) -> bool {
    if dir.join("level.dat").is_file() {
        return true;
    }
    CANDIDATE_REGION_DIRS.iter().any(|rel| {
        let region_dir = dir.join(*rel);
        region_dir.is_dir()
            && fs::read_dir(&region_dir)
                .is_ok_and(|mut d| d.next().is_some())
    })
}

fn looks_like_zip(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == b'P' && data[1] == b'K'
}

/// Join `base` and a relative (possibly nested) name, refusing absolute paths
/// and `..` traversal so uploads cannot escape the workspace.
fn safe_join(base: &Path, name: &str) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    if name.starts_with('/') || name.starts_with('\\') {
        return None;
    }
    if name.len() >= 2 && name.as_bytes()[1] == b':' {
        return None;
    }
    if name.split(['/', '\\']).any(|seg| seg.trim() == "..") {
        return None;
    }
    Some(base.join(name))
}

/// Extract an uploaded `.zip` archive into `dest`, guarding against path
/// traversal and runaway (zip-bomb) sizes. Returns `dest`.
fn extract_zip_to(zip_path: &Path, dest: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(dest).map_err(|e| format!("failed to create the extract directory: {e}"))?;
    let file = fs::File::open(zip_path).map_err(|e| format!("failed to open the archive: {e}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|e| format!("failed to read the archive: {e}"))?;
    let count = archive.len();
    if count > MAX_ARCHIVE_ENTRIES {
        return Err(format!("the archive has too many entries ({count})"));
    }
    let mut extracted = 0u64;
    for i in 0..count {
        let entry = archive
            .by_index(i)
            .map_err(|e| format!("failed to read an archive entry: {e}"))?;
        let raw_name = entry.name().to_string();
        let name = raw_name.replace('\\', "/");
        let name = name.trim_start_matches("./").trim().to_string();
        if name.is_empty() {
            continue;
        }
        let is_dir = name.ends_with('/');
        let target = safe_join(dest, &name)
            .ok_or_else(|| format!("archive entry has an unsafe path: {raw_name}"))?;

        if is_dir {
            fs::create_dir_all(&target).map_err(|e| format!("failed to create a directory: {e}"))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create a directory: {e}"))?;
        }

        let size = entry.size();
        if extracted.saturating_add(size) > MAX_EXTRACTED_BYTES {
            return Err("the archive would extract to too many bytes (possible zip bomb)".to_string());
        }
        let mut buf: Vec<u8> = Vec::new();
        entry
            .take(MAX_EXTRACTED_BYTES)
            .read_to_end(&mut buf)
            .map_err(|e| format!("failed to read an archive entry: {e}"))?;
        if buf.len() as u64 > MAX_EXTRACTED_BYTES.saturating_sub(extracted) {
            return Err("the archive would extract to too many bytes (possible zip bomb)".to_string());
        }
        extracted += buf.len() as u64;
        fs::write(&target, &buf)
            .map_err(|e| format!("failed to write {}: {e}", target.display()))?;
    }
    Ok(dest.to_path_buf())
}

/// The single-page upload UI.
fn index_html() -> String {
    r###"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>worldraw — Minecraft world renderer</title>
<style>
  /* ── Theme variables ─────────────────────────────────────────────── */
  :root, [data-theme="dark"] {
    color-scheme: dark;
    --bg: #0a0e0a;
    --bg-card: #111a11;
    --bg-input: #0d150d;
    --bg-bar: #090d09;
    --border: #1a3a1a;
    --border-hover: #2a5a2a;
    --text: #d4e8d4;
    --text-muted: #6a8a6a;
    --text-hint: #4a6a4a;
    --accent: #00cc44;
    --accent-hover: #00e850;
    --accent-disabled: #1a3a1a;
    --accent-text: #003311;
    --bar-fill: linear-gradient(90deg, #00cc44, #44ff88);
    --result-bg: #00aa33;
    --err: #ff5555;
    --done: #44ff88;
    --dragover-bg: #162216;
  }
  [data-theme="light"] {
    color-scheme: light;
    --bg: #fefefe;
    --bg-card: #ffffff;
    --bg-input: #fafafa;
    --bg-bar: #f0f0f0;
    --border: #e0c0c0;
    --border-hover: #cc6666;
    --text: #1a1a1a;
    --text-muted: #7a5a5a;
    --text-hint: #9a7a7a;
    --accent: #cc2200;
    --accent-hover: #e83000;
    --accent-disabled: #e0c0c0;
    --accent-text: #ffffff;
    --bar-fill: linear-gradient(90deg, #cc2200, #ff5533);
    --result-bg: #aa1100;
    --err: #cc0000;
    --done: #008844;
    --dragover-bg: #fff0f0;
  }
  * { box-sizing: border-box; }
  body {
    font-family: system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
    margin: 0;
    background: var(--bg);
    color: var(--text);
    transition: background .25s, color .25s;
  }
  .wrap { max-width: 720px; margin: 0 auto; padding: 32px 16px 80px; }
  h1 { font-size: 24px; margin: 0 0 4px; font-weight: 700; letter-spacing: -0.5px; }
  p.sub { color: var(--text-muted); margin: 0 0 24px; font-size: 14px; }
  #themeToggle {
    position: fixed;
    top: 14px; right: 14px; z-index: 999;
    width: 42px; height: 42px;
    border: 1px solid var(--border);
    border-radius: 2px;
    background: var(--bg-card);
    color: var(--accent);
    font-size: 20px; cursor: pointer;
    display: flex; align-items: center; justify-content: center;
    transition: background .2s, border-color .2s, transform .1s;
    line-height: 1;
  }
  #themeToggle:hover { border-color: var(--border-hover); background: var(--dragover-bg); }
  #themeToggle:active { transform: scale(0.95); }
  .card {
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: 2px;
    padding: 18px 18px;
    margin-bottom: 14px;
    transition: background .25s, border-color .25s;
  }
  label { display: block; font-size: 13px; color: var(--text-muted); margin: 12px 0 5px; font-weight: 500; }
  input[type="text"], input[type="number"], select {
    width: 100%;
    padding: 10px 10px;
    min-height: 44px;
    border-radius: 2px;
    border: 1px solid var(--border);
    background: var(--bg-input);
    color: var(--text);
    font-size: 15px;
    transition: border-color .2s, background .25s;
  }
  input[type="text"]:focus, input[type="number"]:focus, select:focus {
    outline: none;
    border-color: var(--accent);
  }
  .row { display: flex; flex-wrap: wrap; gap: 12px 16px; }
  .row > div { flex: 1 1 160px; min-width: 0; }
  .checks { display: grid; grid-template-columns: 1fr 1fr; gap: 10px 16px; margin-top: 8px; }
  .checks label {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    font-size: 15px;
    color: var(--text);
    min-height: 36px;
    cursor: pointer;
  }
  .checks input[type="radio"], .checks input[type="checkbox"] {
    width: 18px;
    height: 18px;
    accent-color: var(--accent);
    cursor: pointer;
  }
  button#go {
    margin-top: 18px;
    width: 100%;
    padding: 14px;
    min-height: 48px;
    border: 1px solid var(--accent);
    border-radius: 2px;
    background: var(--accent);
    color: var(--accent-text);
    font-size: 16px;
    font-weight: 700;
    cursor: pointer;
    transition: background .2s, border-color .2s;
  }
  button#go:hover:not(:disabled) { background: var(--accent-hover); border-color: var(--accent-hover); }
  button#go:disabled { background: var(--accent-disabled); border-color: var(--accent-disabled); color: var(--text-hint); cursor: not-allowed; }
  input:disabled { opacity: .4; cursor: not-allowed; }
  label:has(input:disabled) { opacity: .5; }
  .files { display: flex; flex-direction: column; gap: 10px; }
  .files > div { flex: 1; }
  input[type="file"] { width: 100%; font-size: 14px; color: var(--text-muted); padding: 8px 0; }
  .card.dropzone { transition: border-color .15s, background .15s; }
  .card.dropzone.dragover { border-color: var(--accent); background: var(--dragover-bg); }
  .dropzone-hint { color: var(--text-hint); font-size: 12px; margin: 0 0 10px; }
  #stats { display: none; }
  .stats { display: grid; grid-template-columns: 1fr 1fr; gap: 8px 16px; margin-top: 4px; }
  .stats .item { display: flex; justify-content: space-between; gap: 10px; font-size: 14px; }
  .stats .item .k { color: var(--text-muted); }
  .stats .item .v { color: var(--text); font-weight: 600; text-align: right; }
  .estimate-note { color: var(--text-hint); font-size: 12px; margin: 12px 0 0; }
  #status { display: none; }
  .bar { height: 8px; background: var(--bg-bar); border-radius: 1px; overflow: hidden; margin: 4px 0 10px; }
  .bar > div { height: 100%; width: 0; background: var(--bar-fill); transition: width .3s; }
  .msg { font-size: 14px; color: var(--text-muted); }
  .err { color: var(--err); }
  .done { color: var(--done); }
  a.result {
    display: inline-block; margin-top: 12px;
    padding: 12px 20px; min-height: 44px;
    border-radius: 2px;
    background: var(--result-bg);
    color: #fff; text-decoration: none;
    font-weight: 600; font-size: 14px;
  }
  a.result:active { opacity: .8; }
  small.hint { color: var(--text-hint); font-size: 12px; margin-top: 4px; display: block; }
  @media (max-width: 520px) {
    .wrap { padding: 24px 10px 60px; }
    h1 { font-size: 20px; }
    p.sub { font-size: 13px; margin-bottom: 18px; }
    .card { padding: 14px 12px; margin-bottom: 12px; }
    .row { flex-direction: column; gap: 0; }
    .row > div { flex: 1 1 100%; margin-bottom: 4px; }
    .checks { grid-template-columns: 1fr; gap: 6px; }
    .stats { grid-template-columns: 1fr; }
    #themeToggle { top: 10px; right: 10px; width: 38px; height: 38px; font-size: 18px; }
  }
</style>
</head>
<body>
<button id="themeToggle" title="Toggle theme" aria-label="Toggle theme">&#9681;</button>
<div class="wrap">
  <h1>worldraw</h1>
  <p class="sub">Upload a Minecraft world (a folder or a .zip) and render a top-down map.</p>

  <form id="form">
    <div class="card dropzone" id="dropzone">
      <label>World to render</label>
      <p class="dropzone-hint" id="dropnote">Drag &amp; drop a world folder or a .zip archive here, or use the buttons below.</p>
      <div class="files">
        <div>
          <input type="file" id="folder" webkitdirectory directory multiple>
          <small class="hint">…or select the world folder</small>
        </div>
        <div>
          <input type="file" id="zip" accept=".zip">
          <small class="hint">…or a .zip archive</small>
        </div>
      </div>
    </div>

    <div class="card">
      <div class="row">
        <div>
          <label>Dimension</label>
          <select name="dimension">
            <option value="overworld" selected>Overworld</option>
            <option value="nether">The Nether</option>
            <option value="the_end">The End</option>
          </select>
        </div>
        <div>
          <label>Output</label>
          <select name="mode">
            <option value="single" selected>One big PNG</option>
            <option value="chunks">One PNG per chunk (zip)</option>
          </select>
        </div>
        <div>
          <label>Scale (blocks per pixel)</label>
          <input type="number" name="scale" min="1" value="1">
        </div>
      </div>

      <label>Sampling</label>
      <div class="checks">
        <label><input type="radio" name="sampling" value="normal" checked> Normal</label>
        <label><input type="radio" name="sampling" value="supersample"> Supersample (5&times;)</label>
        <label><input type="radio" name="sampling" value="hypersample"> Hypersample (15&times;)</label>
      </div>

      <label>Options</label>
      <div class="checks">
        <label><input type="checkbox" name="shadows" value="on"> Shadows</label>
        <label><input type="checkbox" name="ao" value="on"> Ambient occlusion</label>
        <label><input type="checkbox" name="bloom" value="on"> Bloom</label>
        <label><input type="checkbox" name="transparency" value="on"> Transparency</label>
        <label><input type="checkbox" name="night" value="on"> Night / moon</label>
      </div>
    </div>

    <div class="card" id="stats">
      <label>World size estimate</label>
      <div class="stats" id="statsBody"></div>
      <p class="estimate-note" id="estimateNote"></p>
    </div>

    <button id="go" type="submit">Render world</button>
  </form>

  <div class="card" id="status">
    <div class="bar"><div id="fill"></div></div>
    <div class="msg" id="msg">Working…</div>
    <div id="result"></div>
  </div>
</div>
<script>
  (function() {
    var saved = localStorage.getItem('worldraw-theme') || 'dark';
    document.documentElement.setAttribute('data-theme', saved);
    var btn = document.getElementById('themeToggle');
    btn.textContent = saved === 'dark' ? '\u263E' : '\u263D';
    btn.addEventListener('click', function() {
      var cur = document.documentElement.getAttribute('data-theme');
      var next = cur === 'dark' ? 'light' : 'dark';
      document.documentElement.setAttribute('data-theme', next);
      localStorage.setItem('worldraw-theme', next);
      btn.textContent = next === 'dark' ? '\u263E' : '\u263D';
    });
  })();
  const form = document.getElementById('form');
  const go = document.getElementById('go');
  const statusBox = document.getElementById('status');
  const fill = document.getElementById('fill');
  const msg = document.getElementById('msg');
  const resultBox = document.getElementById('result');
  const statsBox = document.getElementById('stats');
  const statsBody = document.getElementById('statsBody');
  const estimateNote = document.getElementById('estimateNote');
  const FLAGS = ['shadows', 'ao', 'bloom', 'transparency', 'night'];
  let timer = null;
  let uploading = false;
  let currentJob = null; // id of the uploaded ("ready") job
  let worldDims = null; // per-dimension stats from /upload

  const samplingRadios = form.querySelectorAll('input[name="sampling"]');

  // Interlocking form constraints. A disabled control is kept in a forced
  // state (value/checked) so the request always matches what the UI shows.
  function currentSampling() {
    for (const r of samplingRadios) if (r.checked) return r.value;
    return 'normal';
  }
  function applyConstraints() {
    const isNormal = currentSampling() === 'normal';
    const isSingle = form.mode.value === 'single';

    // Scale only applies in Normal mode; force it back to 1 otherwise.
    form.scale.disabled = !isNormal;
    if (!isNormal) form.scale.value = '1';

    // Ambient occlusion and Bloom are only available with supersampling;
    // force them off in Normal mode.
    form.ao.disabled = isNormal;
    form.bloom.disabled = isNormal;
    if (isNormal) { form.ao.checked = false; form.bloom.checked = false; }

    // Shadows are only available for a single big PNG; force off per-chunk.
    form.shadows.disabled = !isSingle;
    if (!isSingle) form.shadows.checked = false;
  }
  function onOptionsChange() { applyConstraints(); updateStats(); }
  samplingRadios.forEach(r => r.addEventListener('change', onOptionsChange));
  form.mode.addEventListener('change', onOptionsChange);
  form.dimension.addEventListener('change', updateStats);
  form.scale.addEventListener('input', updateStats);
  form.scale.addEventListener('change', updateStats);
  go.disabled = true; // enable once a world has been uploaded
  applyConstraints();

  // --- Drag & drop onto the upload card: accepts a world folder or a .zip ---
  const dropZone = document.getElementById('dropzone');
  const folderInput = document.getElementById('folder');
  const zipInput = document.getElementById('zip');
  const dropNote = document.getElementById('dropnote');
  let dropped = null; // { files: File[] } loaded via drag & drop

  function readAllEntries(reader) {
    return new Promise((resolve) => {
      const acc = [];
      const read = () => reader.readEntries(
        (entries) => (entries.length === 0 ? resolve(acc) : (acc.push(...entries), read())),
        () => resolve(acc),
      );
      read();
    });
  }
  async function walkEntry(entry, prefix, out) {
    if (entry.isFile) {
      await new Promise((resolve, reject) =>
        entry.file((f) => { f.__relpath = prefix; out.push(f); resolve(); }, reject));
    } else if (entry.isDirectory) {
      for (const e of await readAllEntries(entry.createReader())) {
        await walkEntry(e, prefix + '/' + e.name, out);
      }
    }
  }
  function setInputFiles(input, files) {
    const dt = new DataTransfer();
    for (const f of files) dt.items.add(f);
    input.files = dt.files;
  }
  function setNote(text) { if (dropNote) dropNote.textContent = text; }

  // Selecting via a button uploads immediately (same as a drop), so the size
  // estimate shows up as soon as the world has been measured.
  folderInput.addEventListener('change', () => {
    dropped = null;
    if (folderInput.files.length) { setInputFiles(zipInput, []); uploadWorld(folderInput.files); }
  });
  zipInput.addEventListener('change', () => {
    dropped = null;
    if (zipInput.files.length) { setInputFiles(folderInput, []); uploadWorld(zipInput.files); }
  });

  async function handleDrop(e) {
    let folderEntry = null, zipFile = null;
    for (const it of Array.from(e.dataTransfer.items)) {
      if (it.kind !== 'file') continue;
      const entry = it.webkitGetAsEntry ? it.webkitGetAsEntry() : null;
      if (entry && entry.isDirectory) {
        if (!folderEntry) folderEntry = entry;
      } else {
        const f = it.getAsFile();
        if (f && /\.zip$/i.test(f.name) && !zipFile) zipFile = f;
      }
    }
    if (folderEntry) {
      const files = [];
      await walkEntry(folderEntry, folderEntry.name, files);
      if (files.length === 0) { setNote('That folder is empty — nothing to upload.'); return; }
      dropped = { files };
      setInputFiles(folderInput, files);
      setInputFiles(zipInput, []);
      setNote('Folder loaded — uploading…');
      uploadWorld(files);
    } else if (zipFile) {
      dropped = { files: [zipFile] };
      setInputFiles(zipInput, [zipFile]);
      setInputFiles(folderInput, []);
      setNote('Archive loaded — uploading…');
      uploadWorld([zipFile]);
    } else {
      setNote('Drop a world folder or a .zip archive.');
    }
  }
  ['dragenter', 'dragover'].forEach((ev) => dropZone.addEventListener(ev, (e) => {
    e.preventDefault(); e.stopPropagation();
    dropZone.classList.add('dragover');
  }));
  dropZone.addEventListener('dragleave', (e) => {
    e.preventDefault();
    if (!dropZone.contains(e.relatedTarget)) dropZone.classList.remove('dragover');
  });
  dropZone.addEventListener('drop', (e) => {
    e.preventDefault(); e.stopPropagation();
    dropZone.classList.remove('dragover');
    handleDrop(e);
  });

  // --- Upload (phase 1): send the world, get back its per-dimension size stats. ---
  async function uploadWorld(files) {
    if (!files || files.length === 0) return;
    if (uploading) return;
    uploading = true;
    if (timer) { clearInterval(timer); timer = null; }
    currentJob = null;
    worldDims = null;
    go.disabled = true;
    resultBox.innerHTML = '';
    statsBox.style.display = 'none';
    statusBox.style.display = 'block';
    fill.style.width = '0%';
    msg.className = 'msg';
    msg.textContent = 'Uploading and measuring world…';

    const fd = new FormData();
    for (const f of files) fd.append('world', f, f.__relpath || f.webkitRelativePath || f.name);
    try {
      const res = await fetch('/upload', { method: 'POST', body: fd });
      const json = await res.json();
      if (!res.ok) throw new Error(json.error || 'upload failed');
      currentJob = json.job;
      worldDims = json.dimensions;
      msg.className = 'msg done';
      msg.textContent = 'World ready — review the estimate, then render.';
      updateStats();
      go.disabled = false;
    } catch (e) {
      showErr(e.message);
    } finally {
      uploading = false;
    }
  }

  // --- Live size estimate, computed client-side from the uploaded stats. ---
  function fmtInt(n) { return n.toLocaleString('en-US'); }
  function fmtBytes(n) {
    if (n < 1024) return n + ' B';
    if (n < 1048576) return (n / 1024).toFixed(1) + ' KB';
    if (n < 1073741824) return (n / 1048576).toFixed(1) + ' MB';
    return (n / 1073741824).toFixed(2) + ' GB';
  }
  function upsampleFactor() {
    const s = currentSampling();
    return s === 'hypersample' ? 15 : s === 'supersample' ? 5 : 1;
  }
  function statItem(k, v) {
    return '<div class="item"><span class="k">' + k + '</span><span class="v">' + v + '</span></div>';
  }
  function updateStats() {
    if (!worldDims) { statsBox.style.display = 'none'; return; }
    const d = worldDims[form.dimension.value];
    if (!d || d.chunks === 0) {
      statsBox.style.display = 'block';
      statsBody.innerHTML = statItem('Chunks', '0');
      estimateNote.textContent = 'No region data found for this dimension.';
      return;
    }
    const ss = upsampleFactor();
    const scale = Math.max(1, parseInt(form.scale.value, 10) || 1);
    const single = form.mode.value === 'single';
    const items = [
      statItem('Chunks', fmtInt(d.chunks)),
      statItem('Region files', fmtInt(d.region_files)),
      statItem('World span', fmtInt(d.blocks_w) + ' \u00d7 ' + fmtInt(d.blocks_h) + ' blocks'),
    ];
    let pixels = 0;
    let note;
    if (single) {
      const w = ss > 1 ? d.blocks_w * ss : Math.ceil(d.blocks_w / scale);
      const h = ss > 1 ? d.blocks_h * ss : Math.ceil(d.blocks_h / scale);
      pixels = w * h;
      items.push(statItem('Image size', fmtInt(w) + ' \u00d7 ' + fmtInt(h) + ' px'));
      items.push(statItem('Pixels', fmtInt(pixels)));
      note = 'One PNG. Size is a rough estimate (~1 byte/pixel after PNG compression); detailed maps can be larger.';
    } else {
      const cpx = ss > 1 ? 16 * ss : Math.ceil(16 / scale);
      pixels = d.chunks * cpx * cpx;
      items.push(statItem('Chunk PNGs', fmtInt(d.chunks) + ' files'));
      items.push(statItem('Each', cpx + ' \u00d7 ' + cpx + ' px'));
      items.push(statItem('Pixels (total)', fmtInt(pixels)));
      note = 'One PNG per chunk, sent as a zip. Size is a rough estimate (~1 byte/pixel after compression).';
    }
    items.push(statItem('Estimated size', '\u2248 ' + fmtBytes(pixels)));
    statsBody.innerHTML = items.join('');
    estimateNote.textContent = note;
    statsBox.style.display = 'block';
  }

  // --- Render (phase 2): start the render for the uploaded job. ---
  form.addEventListener('submit', async (ev) => {
    ev.preventDefault();
    if (!currentJob) {
      alert('Please choose a world folder or a .zip archive first.');
      return;
    }
    const params = new URLSearchParams();
    params.append('job', currentJob);
    params.append('dimension', form.dimension.value);
    params.append('mode', form.mode.value);
    params.append('scale', form.scale.value);
    params.append('sampling', currentSampling());
    for (const n of FLAGS) { const el = form[n]; if (el && el.checked) params.append(n, 'on'); }

    go.disabled = true;
    msg.className = 'msg';
    msg.textContent = 'Rendering…';
    resultBox.innerHTML = '';
    fill.style.width = '0%';
    statusBox.style.display = 'block';

    try {
      const res = await fetch('/render?' + params.toString(), { method: 'POST' });
      const json = await res.json();
      if (!res.ok) throw new Error(json.error || 'failed to start render');
      poll(json.job || currentJob);
    } catch (e) {
      showErr(e.message);
    }
  });

  function poll(job) {
    if (timer) clearInterval(timer);
    timer = setInterval(async () => {
      try {
        const res = await fetch('/progress?job=' + encodeURIComponent(job));
        const p = await res.json();
        if (p.error) { clearInterval(timer); showErr(p.error); return; }
        const pct = p.total > 0 ? Math.min(100, (p.current / p.total) * 100) : 0;
        if (p.status === 'done') {
          clearInterval(timer);
          fill.style.width = '100%';
          msg.className = 'msg done';
          msg.textContent = 'Done!' + (p.total ? ' ' + p.current + ' chunk(s) rendered.' : ' Complete.');
          resultBox.innerHTML = '<a class="result" href="/result?job=' + encodeURIComponent(job) + '">Download result</a>';
          go.disabled = false;
        } else if (p.status === 'error') {
          clearInterval(timer);
          showErr(p.error || p.message || 'render failed');
        } else {
          fill.style.width = pct.toFixed(1) + '%';
          msg.className = 'msg';
          msg.textContent = (p.message || 'Working…') + ' (' + pct.toFixed(0) + '%)';
        }
      } catch (e) {
        clearInterval(timer);
        showErr(e.message);
      }
    }, 1000);
  }

  function showErr(text) {
    msg.className = 'msg err';
    msg.textContent = text;
    resultBox.innerHTML = '';
    go.disabled = false;
  }
</script>
</body>
</html>"###
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDARY: &str = "BOUNDARY123";

    /// Build a realistic multipart/form-data body (CRLF, quoted `name`s) with
    /// the given text fields followed by a single zip file part.
    fn build_multipart(text_fields: &[(&str, &str)], file_name: &str, file_data: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        for (name, value) in text_fields {
            body.extend_from_slice(format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes());
        }
        body.extend_from_slice(format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"world\"; filename=\"{file_name}\"\r\nContent-Type: application/zip\r\n\r\n"
        )
        .as_bytes());
        body.extend_from_slice(file_data);
        body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
        body
    }

    fn content_type() -> String {
        format!("multipart/form-data; boundary={BOUNDARY}")
    }

    fn config_for(text_fields: &[(&str, &str)]) -> RenderConfig {
        let body = build_multipart(text_fields, "world.zip", b"PK\x03\x04zipdata");
        let parsed = parse_multipart(&body, Some(&content_type())).expect("multipart should parse");
        build_config(&parsed.fields, PathBuf::from("world"), PathBuf::from("out")).expect("config")
    }

    #[test]
    fn multipart_field_names_are_unquoted() {
        let body = build_multipart(&[("sampling", "supersample")], "world.zip", b"PKzipdata");
        let parsed = parse_multipart(&body, Some(&content_type())).expect("parse");
        assert_eq!(
            parsed.fields.get("sampling").map(String::as_str),
            Some("supersample")
        );
        // The regression that made every UI option a no-op: the name was
        // stored WITH its surrounding quotes, so no lookup ever matched.
        assert!(
            !parsed.fields.contains_key("\"sampling\""),
            "field name must not retain its quotes"
        );
    }

    #[test]
    fn sampling_radio_selects_supersample() {
        let config = config_for(&[("sampling", "supersample")]);
        assert!(config.supersample, "radio should enable supersampling");
        assert!(!config.hypersample);
    }

    #[test]
    fn sampling_radio_selects_hypersample() {
        let config = config_for(&[("sampling", "hypersample")]);
        assert!(config.hypersample, "radio should enable hypersampling");
        assert!(!config.supersample);
    }

    #[test]
    fn sampling_normal_disables_sampling_even_if_legacy_flag_present() {
        let config = config_for(&[("sampling", "normal"), ("supersample", "on")]);
        assert!(!config.supersample && !config.hypersample);
    }

    #[test]
    fn legacy_boolean_flags_still_work_when_no_sampling_is_sent() {
        let config = config_for(&[("supersample", "on")]);
        assert!(config.supersample && !config.hypersample);
    }

    #[test]
    fn other_options_are_parsed_from_unquoted_names() {
        let config = config_for(&[
            ("dimension", "nether"),
            ("mode", "chunks"),
            ("scale", "2"),
            ("night", "on"),
        ]);
        assert_eq!(config.dim, 1);
        assert!(!config.single);
        assert_eq!(config.scale, 2);
        assert!(config.night);
    }
}