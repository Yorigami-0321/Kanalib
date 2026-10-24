use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::json;

use crate::model::TagHit;

#[derive(Debug, Deserialize)]
pub struct TagItem {
    pub path: String,
    pub ok: bool,
    pub error: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub rating: Option<String>,
    pub group: Option<String>,
    pub tags: Option<Vec<TagHit>>,
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
}

pub struct ReadyInfo {
    pub device: String,
    pub half: bool,
    pub classes: usize,
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
        if dir.join("sidecar").join("tagger").join("server.py").exists() {
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
        let mut child = Command::new(&python)
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
            .stderr(Stdio::inherit())
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
}
