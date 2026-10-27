use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;

use crate::model::TagHit;

#[derive(Debug, Deserialize)]
pub struct TagItem {
    pub path: String,
    pub ok: bool,
    pub error: Option<String>,
    pub error_detail: Option<String>,
    pub damage: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub rating: Option<String>,
    pub group: Option<String>,
    pub tags: Option<Vec<TagHit>>,
    /// 新版 sidecar 返回的每类 top-K（含阈值下分数）；旧 sidecar 缺省为 None。
    #[serde(default)]
    pub candidates: Option<Vec<TagHit>>,
}

#[derive(Debug, Deserialize)]
struct RepairEnvelope {
    #[serde(default)]
    ok: bool,
    width: Option<u32>,
    height: Option<u32>,
    damage: Option<String>,
    error: Option<String>,
    error_detail: Option<String>,
}

pub struct RepairOutcome {
    pub width: u32,
    pub height: u32,
    pub damage: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Envelope {
    id: Option<u64>,
    #[serde(default)]
    ok: bool,
    error: Option<String>,
    items: Option<Vec<TagItem>>,
    ready: Option<bool>,
    device: Option<String>,
    half: Option<bool>,
    classes: Option<usize>,
    /// 模型配置里的类别默认阈值；旧 sidecar 缺省为空。
    thresholds: Option<HashMap<String, f32>>,
}

pub struct ReadyInfo {
    pub device: String,
    pub half: bool,
    pub classes: usize,
    pub thresholds: HashMap<String, f32>,
}

pub struct Sidecar {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.to_path_buf()))
}

pub fn project_root() -> PathBuf {
    if let Some(value) = std::env::var_os("KANALIB_ROOT") {
        return PathBuf::from(value);
    }
    if let Some(dir) = exe_dir() {
        if dir
            .join("sidecar")
            .join("tagger")
            .join("server.py")
            .exists()
        {
            return dir;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn resolve_model_dir(root: &std::path::Path) -> PathBuf {
    let packaged = root.join("dist").join("models").join("pixai-tagger-v1.0");
    if packaged.join("model.safetensors").exists() {
        return packaged;
    }
    root.join("models").join("pixai-tagger-v1.0")
}

pub fn model_dir() -> PathBuf {
    std::env::var_os("KANALIB_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| resolve_model_dir(&project_root()))
}

fn bundled_python(root: &std::path::Path) -> Option<PathBuf> {
    let relative = if cfg!(windows) {
        PathBuf::from("python.exe")
    } else {
        PathBuf::from("bin").join("python3")
    };
    let mut bases = vec![root.to_path_buf()];
    if let Some(dir) = exe_dir() {
        bases.push(dir);
    }
    bases
        .into_iter()
        .map(|base| base.join("runtime").join("python").join(&relative))
        .find(|candidate| candidate.exists())
}

fn python_executable(root: &std::path::Path) -> PathBuf {
    if let Some(value) = std::env::var_os("KANALIB_PYTHON") {
        return PathBuf::from(value);
    }
    if let Some(python) = bundled_python(root) {
        return python;
    }
    #[cfg(debug_assertions)]
    {
        let comfy = PathBuf::from("D:/APP/ComfyUI-aki-v3/python/python.exe");
        if comfy.exists() {
            return comfy;
        }
    }
    PathBuf::from("python")
}

pub fn sidecar_log_path(root: &Path) -> PathBuf {
    root.join(".kanalib").join("sidecar.log")
}

// GUI 父进程没有控制台时，console 子进程会被分配新窗口；stdout 已走管道，
// 新控制台只剩空白，必须显式禁止，并把 stderr 落盘以便取证。
fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

fn open_sidecar_log(root: &Path) -> Stdio {
    let path = sidecar_log_path(root);
    let Some(dir) = path.parent() else {
        return Stdio::null();
    };
    if let Err(error) = std::fs::create_dir_all(dir) {
        log::warn!("create sidecar log dir {}: {error}", dir.display());
        return Stdio::null();
    }
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut file) => {
            let started = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or(0);
            let _ = writeln!(file, "\n=== sidecar spawn (unix {started}) ===");
            Stdio::from(file)
        }
        Err(error) => {
            log::warn!("open sidecar log {}: {error}", path.display());
            Stdio::null()
        }
    }
}

fn append_sidecar_log(root: &Path, label: &str, bytes: &[u8]) {
    let path = sidecar_log_path(root);
    let Some(dir) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{label}");
        let _ = file.write_all(bytes);
        let _ = writeln!(file);
    }
}

pub fn repair_file(input: &Path, output: &Path, max_size: Option<u32>) -> Result<RepairOutcome> {
    let root = project_root();
    let script = root.join("sidecar").join("tagger").join("repair.py");
    if !script.exists() {
        bail!("repair script not found: {}", script.display());
    }
    let python = python_executable(&root);
    if python.is_absolute() && !python.exists() {
        bail!(
            "python not found: {} (place runtime/python next to the app or set KANALIB_PYTHON)",
            python.display()
        );
    }
    let mut command = Command::new(&python);
    command
        .arg(&script)
        .arg("--input")
        .arg(input)
        .arg("--output")
        .arg(output)
        .current_dir(&root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(size) = max_size {
        command.arg("--max-size").arg(size.to_string());
    }
    hide_console(&mut command);
    let result = command
        .output()
        .with_context(|| format!("run repair for {}", input.display()))?;
    if !result.stderr.is_empty() {
        append_sidecar_log(&root, "=== repair stderr ===", &result.stderr);
    }
    let stdout = String::from_utf8_lossy(&result.stdout);
    if stdout.trim().is_empty() {
        bail!(
            "repair produced no output (exit {:?}); see {}",
            result.status.code(),
            sidecar_log_path(&root).display()
        );
    }
    let envelope: RepairEnvelope =
        serde_json::from_str(stdout.trim()).context("parse repair output")?;
    if !envelope.ok {
        if let Some(detail) = &envelope.error_detail {
            append_sidecar_log(
                &root,
                &format!("repair failed: {}", input.display()),
                detail.as_bytes(),
            );
        }
        bail!(
            "{}",
            envelope.error.unwrap_or_else(|| "图片修复失败".to_string())
        );
    }
    Ok(RepairOutcome {
        width: envelope.width.unwrap_or(0),
        height: envelope.height.unwrap_or(0),
        damage: envelope.damage,
    })
}

impl Sidecar {
    pub fn spawn() -> Result<(Self, ReadyInfo)> {
        let root = project_root();
        let script = root.join("sidecar").join("tagger").join("server.py");
        let model = model_dir();
        if !script.exists() {
            bail!("sidecar script not found: {}", script.display());
        }
        if !model.join("model.safetensors").exists() {
            bail!("model not found: {}", model.display());
        }
        let python = python_executable(&root);
        if python.is_absolute() && !python.exists() {
            bail!(
                "python not found: {} (place runtime/python next to the app or set KANALIB_PYTHON)",
                python.display()
            );
        }

        log::info!("sidecar python: {}", python.display());
        log::info!("sidecar model: {}", model.display());
        log::info!("sidecar log: {}", sidecar_log_path(&root).display());
        let mut command = Command::new(&python);
        command
            .arg(&script)
            .arg("--model")
            .arg(&model)
            .arg("--device")
            .arg("auto")
            .current_dir(&root)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("PYTHONUNBUFFERED", "1")
            .env("PYTHONUTF8", "1")
            .env("PYTHONIOENCODING", "utf-8")
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(open_sidecar_log(&root));
        hide_console(&mut command);
        let mut child = command
            .spawn()
            .with_context(|| format!("spawn sidecar with {}", python.display()))?;

        let stdin = child.stdin.take().context("sidecar stdin")?;
        let stdout = child.stdout.take().context("sidecar stdout")?;
        let mut sidecar = Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            next_id: 1,
        };

        let mut line = String::new();
        sidecar
            .stdout
            .read_line(&mut line)
            .context("read sidecar ready line")?;
        if line.trim().is_empty() {
            bail!("sidecar exited before ready");
        }
        let envelope: Envelope =
            serde_json::from_str(line.trim()).context("parse sidecar ready line")?;
        if envelope.ready != Some(true) {
            bail!(
                "sidecar failed to load model: {}",
                envelope.error.unwrap_or_else(|| "unknown error".into())
            );
        }
        let ready = ReadyInfo {
            device: envelope.device.unwrap_or_else(|| "unknown".into()),
            half: envelope.half.unwrap_or(false),
            classes: envelope.classes.unwrap_or(0),
            thresholds: envelope.thresholds.unwrap_or_default(),
        };
        Ok((sidecar, ready))
    }

    pub fn tag(&mut self, paths: &[PathBuf]) -> Result<Vec<TagItem>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "id": id,
            "cmd": "tag",
            "paths": paths.iter().map(|path| path.to_string_lossy().to_string()).collect::<Vec<_>>(),
        });
        writeln!(self.stdin, "{request}")?;
        self.stdin.flush()?;

        let mut line = String::new();
        let read = self.stdout.read_line(&mut line)?;
        if read == 0 {
            bail!("sidecar exited while tagging");
        }
        let envelope: Envelope =
            serde_json::from_str(line.trim()).context("parse sidecar tag response")?;
        if envelope.id != Some(id) {
            bail!("sidecar response id mismatch");
        }
        if !envelope.ok {
            bail!(
                "sidecar tag error: {}",
                envelope.error.unwrap_or_else(|| "unknown error".into())
            );
        }
        Ok(envelope.items.unwrap_or_default())
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "{{\"cmd\":\"shutdown\"}}");
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_model_dir;
    use std::fs;

    #[test]
    fn packaged_model_takes_precedence() {
        let root = std::env::temp_dir().join(format!(
            "kanalib-model-dir-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let packaged = root.join("dist").join("models").join("pixai-tagger-v1.0");
        fs::create_dir_all(&packaged).unwrap();
        fs::write(packaged.join("model.safetensors"), b"stub").unwrap();

        assert_eq!(resolve_model_dir(&root), packaged);

        fs::remove_dir_all(&packaged).unwrap();
        assert_eq!(
            resolve_model_dir(&root),
            root.join("models").join("pixai-tagger-v1.0")
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn parses_optional_candidates_and_ready_thresholds() {
        let envelope: super::Envelope = serde_json::from_str(
            r#"{"id":1,"ok":true,"items":[
                {"path":"a.png","ok":true,"tags":[],
                 "candidates":[{"tag":"low","category":"general","score":0.1}]},
                {"path":"b.png","ok":true,"tags":[]}
            ]}"#,
        )
        .unwrap();
        let items = envelope.items.unwrap();
        assert_eq!(items[0].candidates.as_ref().unwrap().len(), 1);
        assert!(items[1].candidates.is_none());

        let ready: super::Envelope = serde_json::from_str(
            r#"{"ready":true,"device":"cuda","half":true,"classes":30877,
                "thresholds":{"general":0.17,"character":0.27}}"#,
        )
        .unwrap();
        let thresholds = ready.thresholds.unwrap();
        assert_eq!(thresholds.get("general").copied(), Some(0.17));
    }

    #[test]
    #[ignore = "needs the bundled runtime/python"]
    fn repair_script_reencodes_a_truncated_jpeg() {
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        // SAFETY: this ignored test runs alone and only points project_root at the workspace.
        unsafe { std::env::set_var("KANALIB_ROOT", workspace) };
        let root = std::env::temp_dir().join(format!(
            "kanalib-repair-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let input = root.join("bad.jpg");
        let image = image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([(x % 255) as u8, (y % 255) as u8, 90])
        });
        image.save(&input).unwrap();
        let bytes = fs::read(&input).unwrap();
        fs::write(&input, &bytes[..bytes.len().saturating_sub(32)]).unwrap();

        let output = root.join("fixed.png");
        let outcome = super::repair_file(&input, &output, Some(512)).unwrap();
        assert_eq!((outcome.width, outcome.height), (64, 48));
        assert!(outcome.damage.is_some());
        assert!(output.exists());

        fs::remove_dir_all(&root).unwrap();
    }
}
