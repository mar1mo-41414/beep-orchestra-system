//! OTA アップデート: 送られてきた新バイナリを検証 → 原子的入替 → 再起動。
//!
//! フロー:
//!  1. tempfile: `<binary_path>.new` に書き出す (chmod 0755)
//!  2. verify: `--version` を叩いて成功したものだけ受け入れる
//!  3. backup: 現行を `.old` にリネーム
//!  4. swap:   `.new` を binary_path にリネーム (POSIX rename は原子的)
//!  5. detach: `sh -c 'sleep 0.5; <restart_command>'` を setsid で切り離し起動
//!            → 応答返却後に systemd が新プロセスへ入れ替える
//!
//! エラー時 (verify NG, rename 失敗など) は 5xx を返し、既存バイナリは触らない。

use anyhow::{bail, Context, Result};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::{info, warn};

pub struct UpdateOutcome {
    pub old_size: u64,
    pub new_size: u64,
    pub version_output: String,
    pub backup_path: PathBuf,
}

pub fn apply_update(binary_path: &str, new_bytes: &[u8], verify: bool) -> Result<UpdateOutcome> {
    let target = Path::new(binary_path);
    let new_path = target.with_extension("new");
    let old_path = target.with_extension("old");

    if !target.exists() {
        bail!("target binary does not exist: {}", target.display());
    }

    // 1. temp に書き出し
    std::fs::write(&new_path, new_bytes)
        .with_context(|| format!("write new binary: {}", new_path.display()))?;
    let mut perm = std::fs::metadata(&new_path)?.permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&new_path, perm)
        .with_context(|| format!("chmod: {}", new_path.display()))?;

    // 2. verify
    let version_output = if verify {
        let out = Command::new(&new_path)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("spawn --version: {}", new_path.display()))?;
        if !out.status.success() {
            let _ = std::fs::remove_file(&new_path);
            bail!(
                "new binary --version failed (status={:?}): stderr={}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    } else {
        String::new()
    };
    info!(new = %new_path.display(), version = %version_output, "update: verified");

    let old_size = std::fs::metadata(target).map(|m| m.len()).unwrap_or(0);
    let new_size = new_bytes.len() as u64;

    // 3. backup
    // 既に .old がある場合は上書きしてよい (前回の .old)
    std::fs::rename(target, &old_path)
        .with_context(|| format!("backup {} -> {}", target.display(), old_path.display()))?;

    // 4. swap
    if let Err(e) = std::fs::rename(&new_path, target) {
        // 失敗時は .old を戻す
        warn!(error = %e, "rename failed, rolling back");
        let _ = std::fs::rename(&old_path, target);
        bail!("swap failed: {e}");
    }

    info!(
        target = %target.display(),
        backup = %old_path.display(),
        old_size,
        new_size,
        "update: swapped"
    );

    Ok(UpdateOutcome {
        old_size,
        new_size,
        version_output,
        backup_path: old_path,
    })
}

/// 応答返却後に走らせる再起動処理。
/// `sleep 0.5` を挟むのは、この関数が返ってから axum が HTTP レスポンスを
/// 完全に送出するまでの時間を確保するため。setsid で親プロセスから完全に切り離す。
pub fn schedule_restart(cmd: &[String]) -> Result<()> {
    if cmd.is_empty() {
        bail!("restart_command is empty");
    }
    // シェルにエスケープが必要なので、明示的に quoted に組む
    let quoted: Vec<String> = cmd.iter().map(|c| shell_escape(c)).collect();
    let shell_cmd = format!("sleep 0.5 && {}", quoted.join(" "));
    info!(cmd = %shell_cmd, "update: scheduling restart");

    // setsid で切り離し
    let child = Command::new("setsid")
        .arg("sh")
        .arg("-c")
        .arg(&shell_cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match child {
        Ok(_) => Ok(()),
        Err(_) => {
            // setsid が無ければ nohup + & の代替
            warn!("setsid not available, falling back to plain sh");
            Command::new("sh")
                .arg("-c")
                .arg(&format!("({}) &", shell_cmd))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("spawn restart shell")?;
            Ok(())
        }
    }
}

fn shell_escape(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | ',' | '=' | '@'))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}
