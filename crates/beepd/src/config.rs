//! beepd 用の設定。

use anyhow::{Context, Result};
use beep_common::SelectPolicy;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BackendConfig {
    pub command: String,
    pub extra_args: Vec<String>,
    pub dry_run: bool,
    pub min_duration_ms: u32,
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
pub struct DaemonConfig {
    pub node_name: String,
    pub master_mode: bool,
    pub listen_addr: String,
    pub listen_port: u16,
    pub priority_algorithm: SelectPolicy,
    pub backend: BackendConfig,
    /// LAN discovery のマルチキャスト設定
    pub discovery: DiscoveryConfig,
    /// マルチキャスト届かない環境 (別サブネット, Tailscale, WireGuard 等) で
    /// 明示的に peer を指定する。 "host:port" の形式。
    /// 各エントリは周期的に `GET /nodes` で自己情報を取得し peer table に反映する。
    pub static_peers: Vec<String>,
    /// static_peers を再取得する周期 (ms)
    pub static_peer_poll_ms: u64,
    /// OTA アップデート機能の設定
    pub update: UpdateConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    /// POST /update を有効にするか。無効の場合は 403 を返す。
    pub enabled: bool,
    /// 認証トークン。空文字なら update は必ず 403。
    /// クライアントは `X-Update-Token: <this>` ヘッダで認可される。
    pub token: String,
    /// 更新するバイナリの絶対パス。既存の /opt/beepd/beepd と同じであるべき。
    pub binary_path: String,
    /// 更新後に走る再起動コマンド。デフォルトは systemctl restart beepd。
    /// 応答返却後に detach で起動される。
    pub restart_command: Vec<String>,
    /// バイナリを検証するために --version を叩く。失敗したらロールバック。
    pub verify_with_version: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            token: String::new(),
            binary_path: "/opt/beepd/beepd".to_string(),
            restart_command: vec![
                "systemctl".to_string(),
                "restart".to_string(),
                "beepd".to_string(),
            ],
            verify_with_version: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscoveryConfig {
    pub enabled: bool,
    pub multicast_addr: String,
    pub multicast_port: u16,
    pub announce_interval_ms: u64,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            multicast_addr: "239.42.42.42".into(),
            multicast_port: 4242,
            announce_interval_ms: 3000,
        }
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            node_name: hostname_or_default(),
            master_mode: false,
            listen_addr: "0.0.0.0".into(),
            listen_port: 8642,
            priority_algorithm: SelectPolicy::Highest,
            backend: BackendConfig::default(),
            discovery: DiscoveryConfig::default(),
            static_peers: Vec::new(),
            static_peer_poll_ms: 3000,
            update: UpdateConfig::default(),
        }
    }
}

impl DaemonConfig {
    pub fn from_file<P: AsRef<Path>>(p: P) -> Result<Self> {
        let s = std::fs::read_to_string(p.as_ref())
            .with_context(|| format!("read config: {}", p.as_ref().display()))?;
        let cfg: Self = toml::from_str(&s).context("parse config TOML")?;
        Ok(cfg)
    }
}

fn hostname_or_default() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "beep-node".into())
}
