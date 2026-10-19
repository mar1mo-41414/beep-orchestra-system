//! beep-player の設定。TOML でロードする。

use anyhow::{Context, Result};
use beep_common::SelectPolicy;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BackendConfig {
    /// 呼び出す実行ファイル。デフォルトは `beep`。
    pub command: String,
    /// beep へ渡すユーザ定義追加オプション。周波数・長さは自動付与する。
    pub extra_args: Vec<String>,
    /// 本物の beep を呼ばない (ログのみ)。
    pub dry_run: bool,
    /// 音の最短長 (ms)。短すぎるとカチカチするだけになるので下限を設ける。
    pub min_duration_ms: u32,
    /// スケジューリング用: 発音区間の後ろにこの ms だけ無音を挿入する
    /// (beep プロセス起動オーバヘッドを吸収)。
    pub inter_note_gap_ms: u32,
    /// 発音時間を要求長に対して何 % にするか。100=通常、30=デューティ 30%。
    /// pcspkr / コイルスピーカーは振幅制御が無いので、代わりに「音の面積」を
    /// 減らして体感音量を下げる。100 未満のときは min_duration_ms は無視される。
    pub gain_percent: u8,
    /// 周波数を何オクターブシフトするか。-1 = 半分, +1 = 2倍。
    /// コイルスピーカーの刺々しい高域を避けたいときに -1 か -2 を使う。
    pub pitch_shift_octaves: i8,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            command: "beep".into(),
            extra_args: Vec::new(),
            dry_run: false,
            min_duration_ms: 30,
            // 20ms 以上ないと同ピッチ連打の切れ目が耳で判別できない (人間の聴覚限界)。
            // バッチ内 `-D` にも使われるので刻みの分解能はほぼこの値で決まる。
            inter_note_gap_ms: 20,
            gain_percent: 100,
            pitch_shift_octaves: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerConfig {
    pub backend: BackendConfig,
    pub priority_algorithm: SelectPolicy,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            backend: BackendConfig::default(),
            priority_algorithm: SelectPolicy::Highest,
        }
    }
}

impl PlayerConfig {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let s = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("read config: {}", path.as_ref().display()))?;
        let cfg: Self = toml::from_str(&s).context("parse config TOML")?;
        Ok(cfg)
    }
}
