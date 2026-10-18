use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use serde_json::Value;

use crate::listing::{Entry, IndexItem};
use crate::selection::PlanItem;
use crate::text::parse_rfc3339;

pub const REMOTE: &str = "onedrive";
const SCOPES: &str = "Files.Read Files.Read.All Sites.Read.All offline_access";

pub fn find_rclone() -> Option<PathBuf> {
    let name = if cfg!(windows) { "rclone.exe" } else { "rclone" };
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(custom) = std::env::var_os("ONEDRIVE_COPIER_RCLONE") {
        return Some(PathBuf::from(custom)).filter(|p| p.is_file());
    }
    if let Ok(exe) = std::env::current_exe() {
        dirs.extend(exe.parent().map(Path::to_path_buf));
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if cfg!(target_os = "macos") {
        dirs.push("/opt/homebrew/bin".into());
        dirs.push("/usr/local/bin".into());
    }
    if cfg!(windows) {
        dirs.extend(dirs::data_local_dir().map(|d| d.join("Microsoft").join("WinGet").join("Links")));
        dirs.extend(dirs::home_dir().map(|d| d.join("scoop").join("shims")));
        dirs.push("C:\\Program Files\\rclone".into());
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

pub fn config_path() -> PathBuf {
    if let Some(custom) = std::env::var_os("ONEDRIVE_COPIER_CONF") {
        return custom.into();
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("onedrive-copier")
        .join("rclone.conf")
}

pub fn is_logged_in(conf: &Path) -> bool {
    std::fs::read_to_string(conf)
        .map(|text| text.contains(&format!("[{REMOTE}]")) && text.contains("drive_id"))
        .unwrap_or(false)
}

pub fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn command(rclone: &Path, conf: &Path) -> Command {
    let mut cmd = Command::new(rclone);
    cmd.arg("--config").arg(conf).stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

fn remote_arg(path: &str) -> String {
    format!("{REMOTE}:{path}")
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone, Default)]
pub struct Control {
    slot: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
}

impl Control {
    pub fn reset(&self) {
        self.cancelled.store(false, Ordering::SeqCst);
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(child) = lock(&self.slot).as_mut() {
            let _ = child.kill();
        }
    }

    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn spawn(&self, mut cmd: Command, pipe_stdout: bool) -> Result<(Option<ChildStdout>, Option<ChildStderr>), String> {
        cmd.stdout(if pipe_stdout { Stdio::piped() } else { Stdio::null() })
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| format!("No se pudo ejecutar rclone: {e}"))?;
        let streams = (child.stdout.take(), child.stderr.take());
        let mut slot = lock(&self.slot);
        *slot = Some(child);
        if self.cancelled()
            && let Some(child) = slot.as_mut()
        {
            let _ = child.kill();
        }
        Ok(streams)
    }

    fn wait(&self) -> bool {
        let child = lock(&self.slot).take();
        child.and_then(|mut c| c.wait().ok()).is_some_and(|s| s.success())
    }
}

struct Captured {
    stdout: String,
    stderr: String,
    success: bool,
}

fn read_all<R: Read>(reader: Option<R>) -> String {
    let mut buf = Vec::new();
    if let Some(mut r) = reader {
        let _ = r.read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn for_each_line<R: Read>(reader: R, mut on_line: impl FnMut(&str)) {
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => on_line(String::from_utf8_lossy(&buf).trim_end()),
        }
    }
}

fn capture(cmd: Command, ctl: &Control) -> Result<Captured, String> {
    let (out, err) = ctl.spawn(cmd, true)?;
    let err_reader = thread::spawn(move || read_all(err));
    let stdout = read_all(out);
    let stderr = err_reader.join().unwrap_or_default();
    let success = ctl.wait();
    if ctl.cancelled() {
        return Err("Cancelado".to_string());
    }
    Ok(Captured { stdout, stderr, success })
}

fn clean_log_line(line: &str) -> &str {
    let line = line.trim();
    let b = line.as_bytes();
    let dated = b.len() > 20 && b[4] == b'/' && b[7] == b'/' && b[10] == b' ' && b[13] == b':' && b[16] == b':';
    let rest = if dated { line.get(20..).unwrap_or(line).trim_start() } else { line };
    ["ERROR : ", "ERROR: ", "NOTICE: ", "CRITICAL: "].iter().find_map(|p| rest.strip_prefix(p)).unwrap_or(rest)
}

fn last_line(text: &str) -> String {
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map_or("error desconocido de rclone", clean_log_line)
        .to_string()
}

fn failed_dirs(stderr: &str) -> Vec<String> {
    let mut names: Vec<String> = stderr
        .lines()
        .filter_map(|l| clean_log_line(l).split_once(": error listing").map(|(name, _)| name.to_string()))
        .collect();
    names.dedup();
    names
}

pub fn pick_drive(option: &Value) -> String {
    let examples = option["Examples"].as_array().cloned().unwrap_or_default();
    let onedrive = examples
        .iter()
        .find(|e| e["Help"].as_str().is_some_and(|h| h.starts_with("OneDrive (")));
    onedrive
        .or(examples.first())
        .and_then(|e| e["Value"].as_str())
        .or(option["DefaultStr"].as_str())
        .unwrap_or("")
        .to_string()
}

pub fn answer_for(option: &Value) -> String {
    match option["Name"].as_str().unwrap_or("") {
        "" => String::new(),
        "config_is_local" => "true".to_string(),
        "config_type" => "onedrive".to_string(),
        "config_driveid" => pick_drive(option),
        _ if option["Type"] == "bool" => "true".to_string(),
        _ => option["DefaultStr"].as_str().unwrap_or("").to_string(),
    }
}

fn wizard(rclone: &Path, conf: &Path, ctl: &Control, args: &[&str]) -> Result<Value, String> {
    let mut cmd = command(rclone, conf);
    cmd.args(args);
    let out = capture(cmd, ctl)?;
    serde_json::from_str(out.stdout.trim()).map_err(|_| last_line(&out.stderr))
}

pub fn login(rclone: &Path, conf: &Path, ctl: &Control) -> Result<(), String> {
    if let Some(dir) = conf.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("No se pudo crear {}: {e}", dir.display()))?;
    }
    let mut staging = conf.as_os_str().to_owned();
    staging.push(".new");
    let staging = PathBuf::from(staging);
    let _ = std::fs::remove_file(&staging);

    let scopes = format!("access_scopes={SCOPES}");
    let mut step = wizard(rclone, &staging, ctl, &["config", "create", REMOTE, "onedrive", &scopes, "--non-interactive"])?;
    for _ in 0..12 {
        if let Some(error) = step["Error"].as_str().filter(|e| !e.is_empty()) {
            return Err(error.to_string());
        }
        let state = step["State"].as_str().unwrap_or("").to_string();
        if state.is_empty() {
            return std::fs::rename(&staging, conf).map_err(|e| format!("No se pudo guardar la sesión: {e}"));
        }
        let answer = answer_for(&step["Option"]);
        step = wizard(
            rclone,
            &staging,
            ctl,
            &["config", "update", REMOTE, "--continue", "--state", &state, "--result", &answer, "--non-interactive"],
        )?;
    }
    Err("El asistente de rclone no terminó".to_string())
}

fn entry_from_json(parent: &str, item: &Value) -> Option<Entry> {
    Some(Entry::new(
        parent,
        item["Name"].as_str()?,
        item["IsDir"].as_bool().unwrap_or(false),
        item["Size"].as_i64().and_then(|s| u64::try_from(s).ok()),
        item["ModTime"].as_str().and_then(parse_rfc3339),
    ))
}

pub fn list_entries(rclone: &Path, conf: &Path, parent: &str) -> Result<Vec<Entry>, String> {
    let mut cmd = command(rclone, conf);
    cmd.args(["lsjson", "--no-mimetype"]).arg(remote_arg(parent));
    let out = capture(cmd, &Control::default())?;
    if !out.success {
        return Err(last_line(&out.stderr));
    }
    let items: Value = serde_json::from_str(out.stdout.trim()).map_err(|e| format!("Respuesta de rclone no válida: {e}"))?;
    Ok(items.as_array().map(|a| a.iter().filter_map(|i| entry_from_json(parent, i)).collect()).unwrap_or_default())
}

pub fn parse_index_line(line: &str) -> Option<IndexItem> {
    let line = line.trim().trim_end_matches(',');
    if !line.starts_with('{') {
        return None;
    }
    let item: Value = serde_json::from_str(line).ok()?;
    if item["IsDir"] != true {
        return None;
    }
    Some(IndexItem::new(item["Path"].as_str()?, item["ModTime"].as_str().and_then(parse_rfc3339)))
}

pub fn index_dirs(rclone: &Path, conf: &Path, ctl: &Control, emit: &dyn Fn(Vec<IndexItem>)) -> Result<Option<String>, String> {
    let mut cmd = command(rclone, conf);
    cmd.args(["lsjson", "-R", "--dirs-only", "--no-mimetype"]).arg(remote_arg(""));
    let (out, err) = ctl.spawn(cmd, true)?;
    let err_reader = thread::spawn(move || read_all(err));
    let (mut batch, mut total) = (Vec::new(), 0usize);
    if let Some(out) = out {
        for_each_line(out, |line| {
            if let Some(item) = parse_index_line(line) {
                batch.push(item);
                total += 1;
                if batch.len() >= 100 {
                    emit(std::mem::take(&mut batch));
                }
            }
        });
    }
    if !batch.is_empty() {
        emit(batch);
    }
    let stderr = err_reader.join().unwrap_or_default();
    let success = ctl.wait();
    if ctl.cancelled() {
        return Err("Cancelado".to_string());
    }
    match (success, total) {
        (true, _) => Ok(None),
        (false, 0) => Err(last_line(&stderr)),
        (false, _) => {
            let names = failed_dirs(&stderr);
            let detail = if names.is_empty() { last_line(&stderr) } else { names.join(", ") };
            Ok(Some(format!("Algunas carpetas no se pudieron listar: {detail}")))
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Progress {
    pub bytes: u64,
    pub total_bytes: u64,
    pub speed: f64,
    pub eta: Option<u64>,
    pub files_done: u64,
    pub files_total: u64,
    pub current: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum LogLine {
    Stats(Progress),
    Error(String),
    Other,
}

pub fn parse_log_line(line: &str) -> LogLine {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return LogLine::Other;
    };
    if let Some(s) = v.get("stats") {
        return LogLine::Stats(Progress {
            bytes: s["bytes"].as_u64().unwrap_or(0),
            total_bytes: s["totalBytes"].as_u64().unwrap_or(0),
            speed: s["speed"].as_f64().unwrap_or(0.0),
            eta: s["eta"].as_u64(),
            files_done: s["transfers"].as_u64().unwrap_or(0),
            files_total: s["totalTransfers"].as_u64().unwrap_or(0),
            current: s["transferring"]
                .as_array()
                .map(|a| a.iter().take(4).filter_map(|t| t["name"].as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
        });
    }
    if v["level"] == "error" {
        let msg = v["msg"].as_str().unwrap_or("error");
        return LogLine::Error(match v["object"].as_str() {
            Some(object) => format!("{object}: {msg}"),
            None => msg.to_string(),
        });
    }
    LogLine::Other
}

fn glob_escape(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | '\\') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

fn list_lines(item: &PlanItem) -> Option<Vec<String>> {
    match item {
        PlanItem::Folder { excludes, .. } if excludes.is_empty() => None,
        PlanItem::Folder { excludes, .. } => Some(
            excludes
                .iter()
                .flat_map(|e| {
                    let glob = glob_escape(e);
                    [format!("/{glob}"), format!("/{glob}/**")]
                })
                .collect(),
        ),
        PlanItem::Files { files, .. } => Some(files.clone()),
    }
}

fn source_of(item: &PlanItem) -> &str {
    match item {
        PlanItem::Folder { src, .. } => src,
        PlanItem::Files { src_dir, .. } => src_dir,
    }
}

fn filter_args(item: &PlanItem, list: Option<&Path>) -> Vec<OsString> {
    let Some(list) = list else { return Vec::new() };
    let flag = if matches!(item, PlanItem::Files { .. }) { "--files-from-raw" } else { "--exclude-from" };
    vec![flag.into(), list.into()]
}

fn join_dest(base: &Path, relative: &str) -> PathBuf {
    relative.split('/').filter(|s| !s.is_empty()).fold(base.to_path_buf(), |path, part| path.join(part))
}

pub fn item_name(item: &PlanItem) -> String {
    match item {
        PlanItem::Folder { src, .. } => leaf(src).to_string(),
        PlanItem::Files { files, .. } if files.len() == 1 => files[0].clone(),
        PlanItem::Files { src_dir, files, .. } => {
            let place = if src_dir.is_empty() { "la raíz" } else { leaf(src_dir) };
            format!("{} ficheros de {place}", files.len())
        }
    }
}

pub fn copy_args(item: &PlanItem, dest_base: &Path, list: Option<&Path>, skip_existing: bool, transfers: u32) -> Vec<OsString> {
    let dest = match item {
        PlanItem::Folder { dest, .. } | PlanItem::Files { dest, .. } => join_dest(dest_base, dest),
    };
    let mut args: Vec<OsString> = vec!["copy".into(), remote_arg(source_of(item)).into(), dest.into()];
    args.extend(filter_args(item, list));
    if skip_existing {
        args.push("--ignore-existing".into());
    }
    args.extend(["--transfers".to_string(), transfers.max(1).to_string()].map(OsString::from));
    args.extend(["--use-json-log", "--stats", "1s", "--stats-log-level", "NOTICE"].map(OsString::from));
    args
}

struct TempList(PathBuf);

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl TempList {
    fn create(lines: &[String]) -> Result<Self, String> {
        let name = format!("onedrive-copier-{}-{}.txt", std::process::id(), TEMP_COUNTER.fetch_add(1, Ordering::SeqCst));
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, lines.join("\n")).map_err(|e| format!("No se pudo crear la lista temporal: {e}"))?;
        Ok(Self(path))
    }
}

impl Drop for TempList {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn temp_list_for(item: &PlanItem) -> Result<Option<TempList>, String> {
    list_lines(item).map(|lines| TempList::create(&lines)).transpose()
}

pub struct CopyJob {
    pub plan: Vec<PlanItem>,
    pub dest: PathBuf,
    pub skip_existing: bool,
    pub transfers: u32,
}

pub enum CopyEvent {
    Start { index: usize, total: usize, name: String },
    Stats(Progress),
    Error(String),
    Finished { cancelled: bool, failed: Vec<String> },
}

fn stream_stderr(cmd: Command, ctl: &Control, on_line: impl FnMut(&str)) -> Result<bool, String> {
    let (_, err) = ctl.spawn(cmd, false)?;
    if let Some(err) = err {
        for_each_line(err, on_line);
    }
    Ok(ctl.wait())
}

pub fn run_copy(rclone: &Path, conf: &Path, job: &CopyJob, ctl: &Control, emit: &dyn Fn(CopyEvent)) {
    let total = job.plan.len();
    let mut failed = Vec::new();
    for (index, item) in job.plan.iter().enumerate() {
        if ctl.cancelled() {
            break;
        }
        let name = item_name(item);
        emit(CopyEvent::Start { index, total, name: name.clone() });
        let result = temp_list_for(item).and_then(|list| {
            let mut cmd = command(rclone, conf);
            cmd.args(copy_args(item, &job.dest, list.as_ref().map(|l| l.0.as_path()), job.skip_existing, job.transfers));
            stream_stderr(cmd, ctl, |line| match parse_log_line(line) {
                LogLine::Stats(p) => emit(CopyEvent::Stats(p)),
                LogLine::Error(m) => emit(CopyEvent::Error(m)),
                LogLine::Other => {}
            })
        });
        let ok = result.unwrap_or_else(|e| {
            emit(CopyEvent::Error(e));
            false
        });
        if !ok && !ctl.cancelled() {
            failed.push(name);
        }
    }
    emit(CopyEvent::Finished { cancelled: ctl.cancelled(), failed });
}

pub fn size_args(item: &PlanItem, list: Option<&Path>) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["size".into(), "--json".into(), remote_arg(source_of(item)).into()];
    args.extend(filter_args(item, list));
    args
}

pub fn selection_size(rclone: &Path, conf: &Path, plan: &[PlanItem], ctl: &Control) -> Result<(u64, u64), String> {
    let (mut count, mut bytes) = (0, 0);
    for item in plan {
        let list = temp_list_for(item)?;
        let mut cmd = command(rclone, conf);
        cmd.args(size_args(item, list.as_ref().map(|l| l.0.as_path())));
        let out = capture(cmd, ctl)?;
        if !out.success {
            return Err(last_line(&out.stderr));
        }
        let size: Value = serde_json::from_str(out.stdout.trim()).map_err(|e| format!("Respuesta de rclone no válida: {e}"))?;
        count += size["count"].as_u64().unwrap_or(0);
        bytes += size["bytes"].as_u64().unwrap_or(0);
    }
    Ok((count, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(args: &[OsString]) -> Vec<String> {
        args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    fn folder(src: &str, dest: &str, excludes: &[&str]) -> PlanItem {
        PlanItem::Folder { src: src.into(), dest: dest.into(), excludes: excludes.iter().map(|e| e.to_string()).collect() }
    }

    #[test]
    fn leaf_returns_last_component() {
        assert_eq!(leaf("Imagenes/2015/Toledo"), "Toledo");
        assert_eq!(leaf("Solo"), "Solo");
    }

    #[test]
    fn pick_drive_prefers_the_onedrive_entry_over_the_default() {
        let option = json!({
            "DefaultStr": "b!ArchiveId",
            "Examples": [
                {"Value": "b!ArchiveId", "Help": "ODCMetadataArchive (personal)"},
                {"Value": "8A3960C92A9D1C32", "Help": "OneDrive (personal)"},
                {"Value": "b!Other", "Help": "Bundles_x"}
            ]
        });
        assert_eq!(pick_drive(&option), "8A3960C92A9D1C32");
    }

    #[test]
    fn pick_drive_falls_back_to_first_example() {
        let option = json!({"Examples": [{"Value": "X", "Help": "Algo"}]});
        assert_eq!(pick_drive(&option), "X");
    }

    #[test]
    fn answer_for_wizard_questions() {
        assert_eq!(answer_for(&json!({"Name": "config_is_local", "Type": "bool"})), "true");
        assert_eq!(answer_for(&json!({"Name": "config_type"})), "onedrive");
        assert_eq!(answer_for(&json!({"Name": "config_drive_ok", "Type": "bool"})), "true");
        assert_eq!(answer_for(&Value::Null), "");
    }

    #[test]
    fn parse_stats_line() {
        let line = r#"{"level":"notice","msg":"stats","stats":{"bytes":203852781,"totalBytes":357589194,"speed":1442284.5,"eta":106,"transfers":84,"totalTransfers":159,"transferring":[{"name":"a.jpg","size":10},{"name":"b.jpg","size":20}]}}"#;
        let LogLine::Stats(p) = parse_log_line(line) else { panic!("no es stats") };
        assert_eq!(p.bytes, 203852781);
        assert_eq!(p.total_bytes, 357589194);
        assert_eq!(p.eta, Some(106));
        assert_eq!((p.files_done, p.files_total), (84, 159));
        assert_eq!(p.current, vec!["a.jpg", "b.jpg"]);
    }

    #[test]
    fn parse_stats_line_with_null_eta() {
        let LogLine::Stats(p) = parse_log_line(r#"{"stats":{"bytes":0,"totalBytes":0,"eta":null}}"#) else { panic!() };
        assert_eq!(p.eta, None);
    }

    #[test]
    fn parse_error_line_includes_object() {
        let line = r#"{"level":"error","msg":"Failed to copy: boom","object":"x/y.jpg"}"#;
        assert_eq!(parse_log_line(line), LogLine::Error("x/y.jpg: Failed to copy: boom".to_string()));
    }

    #[test]
    fn parse_ignores_info_and_garbage() {
        assert_eq!(parse_log_line(r#"{"level":"info","msg":"Copied (new)","object":"a"}"#), LogLine::Other);
        assert_eq!(parse_log_line("no es json"), LogLine::Other);
    }

    #[test]
    fn log_lines_lose_their_timestamp_and_level() {
        assert_eq!(clean_log_line("2026/10/18 10:39:47 ERROR : Almacén personal: error listing: x"), "Almacén personal: error listing: x");
        assert_eq!(clean_log_line("2026/10/18 10:41:24 NOTICE: Failed to lsjson"), "Failed to lsjson");
        assert_eq!(clean_log_line("  sin formato  "), "sin formato");
        assert_eq!(last_line("a\n2026/10/18 10:41:24 CRITICAL: boom\n\n"), "boom");
        assert_eq!(last_line(""), "error desconocido de rclone");
    }

    #[test]
    fn failed_folders_are_named_once() {
        let stderr = "2026/10/18 10:39:47 ERROR : Almacén personal: error listing: couldn't list files: x\n\
                      2026/10/18 10:39:47 ERROR : Almacén personal: error listing: couldn't list files: x\n\
                      2026/10/18 10:41:24 NOTICE: Failed to lsjson with 2 errors";
        assert_eq!(failed_dirs(stderr), ["Almacén personal"]);
        assert!(failed_dirs("nada").is_empty());
    }

    #[test]
    fn index_lines_tolerate_commas_and_ignore_brackets_and_files() {
        let dir = r#"{"Path":"Imágenes/2015","Name":"2015","Size":-1,"ModTime":"2026-07-16T10:10:45Z","IsDir":true},"#;
        let item = parse_index_line(dir).unwrap();
        assert_eq!((item.path.as_str(), item.name.as_str()), ("Imágenes/2015", "2015"));
        assert!(item.modified.is_some());
        assert_eq!(parse_index_line("["), None);
        assert_eq!(parse_index_line("]"), None);
        assert_eq!(parse_index_line(r#"{"Path":"a.txt","IsDir":false}"#), None);
        assert_eq!(parse_index_line("{roto"), None);
    }

    #[test]
    fn entries_read_size_and_date_and_hide_unknown_sizes() {
        let file = json!({"Name": "a.jpg", "IsDir": false, "Size": 1234, "ModTime": "2000-03-01T00:00:00Z"});
        let entry = entry_from_json("Imágenes", &file).unwrap();
        assert_eq!((entry.path.as_str(), entry.size, entry.modified), ("Imágenes/a.jpg", Some(1234), Some(951_868_800)));
        let dir = json!({"Name": "Sub", "IsDir": true, "Size": -1, "ModTime": ""});
        let entry = entry_from_json("", &dir).unwrap();
        assert!(entry.is_dir && entry.size.is_none() && entry.modified.is_none());
    }

    #[test]
    fn copy_never_overwrites_by_default_and_sets_parallel_transfers() {
        let args = text(&copy_args(&folder("Imagenes/Toledo", "Toledo", &[]), Path::new("/tmp/d"), None, true, 8));
        assert_eq!(&args[..3], ["copy", "onedrive:Imagenes/Toledo", "/tmp/d/Toledo"]);
        assert!(args.contains(&"--ignore-existing".to_string()));
        let at = args.iter().position(|a| a == "--transfers").unwrap();
        assert_eq!(args[at + 1], "8");
        let permissive = text(&copy_args(&folder("a", "a", &[]), Path::new("/x"), None, false, 0));
        assert!(!permissive.contains(&"--ignore-existing".to_string()));
        assert!(permissive.contains(&"1".to_string()));
    }

    #[test]
    fn folders_with_exclusions_and_loose_files_use_filter_lists() {
        let list = Path::new("/tmp/lista.txt");
        let excl = text(&copy_args(&folder("A", "A", &["B"]), Path::new("/d"), Some(list), true, 4));
        assert!(excl.windows(2).any(|w| w == ["--exclude-from", "/tmp/lista.txt"]));
        let files = PlanItem::Files { src_dir: "A/B".into(), dest: "A/B".into(), files: vec!["x.jpg".into()] };
        let args = text(&copy_args(&files, Path::new("/d"), Some(list), true, 4));
        assert_eq!(&args[..3], ["copy", "onedrive:A/B", "/d/A/B"]);
        assert!(args.windows(2).any(|w| w == ["--files-from-raw", "/tmp/lista.txt"]));
    }

    #[test]
    fn exclusion_lists_escape_glob_characters_and_cover_files_and_folders() {
        let lines = list_lines(&folder("A", "A", &["B/foto [1].jpg", "x*y"])).unwrap();
        assert_eq!(lines, ["/B/foto \\[1\\].jpg", "/B/foto \\[1\\].jpg/**", "/x\\*y", "/x\\*y/**"]);
        assert!(list_lines(&folder("A", "A", &[])).is_none());
    }

    #[test]
    fn size_command_reuses_the_same_filters() {
        let list = Path::new("/tmp/l.txt");
        let args = text(&size_args(&folder("A", "A", &["B"]), Some(list)));
        assert_eq!(&args[..3], ["size", "--json", "onedrive:A"]);
        assert!(args.windows(2).any(|w| w == ["--exclude-from", "/tmp/l.txt"]));
    }

    #[test]
    fn plan_items_have_readable_names() {
        assert_eq!(item_name(&folder("Imagenes/Toledo", "Toledo", &[])), "Toledo");
        let many = PlanItem::Files { src_dir: "A/Raw".into(), dest: "Raw".into(), files: vec!["1".into(), "2".into()] };
        assert_eq!(item_name(&many), "2 ficheros de Raw");
        let one = PlanItem::Files { src_dir: String::new(), dest: String::new(), files: vec!["solo.txt".into()] };
        assert_eq!(item_name(&one), "solo.txt");
    }

    #[test]
    fn destination_paths_are_built_per_segment() {
        assert_eq!(join_dest(Path::new("/d"), "A/B/C"), Path::new("/d/A/B/C"));
        assert_eq!(join_dest(Path::new("/d"), ""), Path::new("/d"));
    }

    #[test]
    #[ignore = "usa OneDrive real (tarda ~1,5 min): ONEDRIVE_COPIER_CONF"]
    fn live_index_streams_batches_and_tolerates_broken_folders() {
        let rclone = find_rclone().expect("rclone");
        let conf = config_path();
        let batches = Mutex::new(Vec::new());
        let result = index_dirs(&rclone, &conf, &Control::default(), &|b| lock(&batches).push(b)).expect("indexar");
        let batches = batches.into_inner().unwrap();
        let items: Vec<&IndexItem> = batches.iter().flatten().collect();
        assert!(!items.is_empty(), "no se indexó ninguna carpeta");
        eprintln!("indexadas {} carpetas en {} tandas; nota: {result:?}", items.len(), batches.len());
    }

    #[test]
    #[ignore = "usa OneDrive real: ONEDRIVE_COPIER_CONF, ONEDRIVE_COPIER_TEST_FOLDER y ONEDRIVE_COPIER_TEST_DEST"]
    fn live_list_index_size_and_copy() {
        let rclone = find_rclone().expect("rclone");
        let conf = config_path();
        assert!(is_logged_in(&conf), "no hay sesión en {}", conf.display());
        let roots = list_entries(&rclone, &conf, "").expect("listar raíz");
        assert!(roots.iter().any(|e| e.is_dir && e.modified.is_some()), "raíz: {roots:?}");
        let folder = std::env::var("ONEDRIVE_COPIER_TEST_FOLDER").expect("carpeta pequeña con al menos 2 ficheros");
        let inside = list_entries(&rclone, &conf, &folder).expect("listar carpeta");
        assert!(inside.iter().filter(|e| !e.is_dir && e.size.is_some()).count() >= 2, "hacen falta 2 ficheros: {inside:?}");

        let mut selection = crate::selection::Selection::default();
        selection.set(&folder, true, true);
        let first_file = inside.iter().find(|e| !e.is_dir).unwrap();
        selection.set(&first_file.path, false, false);
        let plan = selection.plan();

        let (count, bytes) = selection_size(&rclone, &conf, &plan, &Control::default()).expect("tamaño");
        assert_eq!(count as usize, inside.iter().filter(|e| !e.is_dir).count() - 1);
        assert!(bytes > 0);

        let dest = PathBuf::from(std::env::var("ONEDRIVE_COPIER_TEST_DEST").expect("destino"));
        let job = CopyJob { plan, dest: dest.clone(), skip_existing: true, transfers: 4 };
        let events = Mutex::new(Vec::new());
        run_copy(&rclone, &conf, &job, &Control::default(), &|e| lock(&events).push(e));
        let events = events.into_inner().unwrap();
        let Some(CopyEvent::Finished { cancelled, failed }) = events.last() else { panic!("sin Finished") };
        assert!(!cancelled && failed.is_empty(), "fallos: {failed:?}");
        let copied = dest.join(leaf(&folder));
        assert_eq!(copied.read_dir().unwrap().count(), count as usize);
        assert!(!copied.join(&first_file.name).exists(), "el fichero excluido se copió");
    }
}
