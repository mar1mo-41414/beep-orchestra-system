//! HTTP API 定義。
//!
//! - GET  /nodes           自ノード + 発見済みピアの一覧
//! - GET  /status          現在の再生状態
//! - POST /play            multipart で MIDI アップロード → 再生開始
//!                         フィールド: file=@x.mid, [policy=highest|...], [start_at=<unix秒>]
//! - POST /stop            停止
//! - POST /pause           一時停止
//! - POST /resume          再開

use axum::{
    extract::{DefaultBodyLimit, Multipart, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::{get, post},
    Router,
};
use beep_common::score::Score;
use beep_common::{midi, SelectPolicy};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{info, warn};

use crate::engine::Command;
use crate::state::AppState;

/// POST /play_part / /orchestra/play で受ける、事前生成済み Score リクエスト
#[derive(Debug, Serialize, Deserialize)]
pub struct PlayPartRequest {
    pub score: Score,
    pub policy: Option<SelectPolicy>,
    pub start_at: Option<f64>,
}

/// POST /orchestra/play の追加パラメータ
#[derive(Debug, Deserialize)]
struct OrchestraOpts {
    /// マスター自身も演奏に参加するか (デフォルト true)。
    /// `node_order` を明示指定した場合はこの値は無視され、そのリストの通りになる。
    #[serde(default = "default_true")]
    include_self: bool,
    /// 全体オフセット (秒)。now + lead_time_sec を start_at にする。
    /// 明示 start_at が来たらそちらを優先。
    #[serde(default = "default_lead_time")]
    lead_time_sec: f64,
    /// 明示的な絶対時刻 (unix 秒)。指定時は lead_time_sec を無視。
    start_at: Option<f64>,
    /// ボイス選音アルゴリズム (通常 highest で問題ない)
    policy: Option<SelectPolicy>,
    /// 明示的なノード順。UUID または node_name のどちらでも指定できる。
    /// 例: ["nodeB", "nodeC", "nodeA"]  → voice 0=nodeB, voice 1=nodeC, voice 2=nodeA
    /// 未知の名前が来たら 400 を返す。
    node_order: Option<Vec<String>>,
    /// パート分配戦略。デフォルトは "pitch" (高音優先)。
    /// "track" にすると SMF トラック順で分配する
    #[serde(default)]
    voicing: beep_common::VoicingStrategy,
}
fn default_true() -> bool {
    true
}
fn default_lead_time() -> f64 {
    2.0
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(get_ui))
        .route("/ui", get(get_ui))
        .route("/nodes", get(get_nodes))
        .route("/status", get(get_status))
        .route("/play", post(post_play))
        .route("/play_part", post(post_play_part))
        .route("/orchestra/play", post(post_orchestra_play))
        .route("/stop", post(post_stop))
        .route("/stop_all", post(post_stop_all))
        .route("/pause", post(post_pause))
        .route("/resume", post(post_resume))
        .route("/update", post(post_update))
        // MIDI は数百KB以下を想定、beepd の OTA バイナリでも 16 MiB あれば十分
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state)
}

/// GET / と GET /ui: 埋め込みの Web UI を返す (単一 HTML)。
async fn get_ui() -> Html<&'static str> {
    Html(include_str!("ui/index.html"))
}

/// POST /stop_all: 自ノード + 発見済み peer 全部に /stop を叩く。
/// Web UI から 1 リクエストで全体停止するためのマスター専用ヘルパ。
async fn post_stop_all(State(s): State<AppState>) -> Json<serde_json::Value> {
    // 自ノード停止
    let self_ok = s.engine.send(crate::engine::Command::Stop).is_ok();

    let peers = s.peers.snapshot();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .ok();

    let mut peer_results = Vec::new();
    if let Some(client) = client {
        let mut handles = Vec::new();
        for p in peers {
            let url = format!("http://{}:{}/stop", p.ip, p.http_port);
            let name = p.name.clone();
            let cli = client.clone();
            handles.push(tokio::spawn(async move {
                let r = cli.post(&url).send().await;
                match r {
                    Ok(resp) if resp.status().is_success() => (name, true, None),
                    Ok(resp) => (name, false, Some(format!("HTTP {}", resp.status()))),
                    Err(e) => (name, false, Some(e.to_string())),
                }
            }));
        }
        for h in handles {
            if let Ok((name, ok, err)) = h.await {
                peer_results.push(json!({ "name": name, "ok": ok, "error": err }));
            }
        }
    }

    let all_ok = self_ok && peer_results.iter().all(|v| v["ok"].as_bool().unwrap_or(false));
    Json(json!({
        "ok": all_ok,
        "self_ok": self_ok,
        "peers": peer_results,
    }))
}

#[derive(Debug, Serialize)]
struct NodesResponse {
    self_node: beep_common::node::NodeInfo,
    peers: Vec<beep_common::node::NodeInfo>,
}

async fn get_nodes(State(s): State<AppState>) -> Json<NodesResponse> {
    Json(NodesResponse {
        self_node: (*s.self_info).clone(),
        peers: s.peers.snapshot(),
    })
}

async fn get_status(State(s): State<AppState>) -> Json<crate::engine::PlaybackStatus> {
    Json(s.engine.status())
}

async fn post_play(
    State(s): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let mut midi_bytes: Option<Vec<u8>> = None;
    let mut policy: Option<SelectPolicy> = None;
    let mut start_at: Option<f64> = None;
    let mut filename: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("multipart: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                filename = field.file_name().map(|s| s.to_string());
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| (StatusCode::BAD_REQUEST, format!("read file: {e}")))?;
                midi_bytes = Some(data.to_vec());
            }
            "policy" => {
                let t = field
                    .text()
                    .await
                    .map_err(|e| (StatusCode::BAD_REQUEST, format!("read policy: {e}")))?;
                policy = Some(parse_policy(&t).map_err(|e| (StatusCode::BAD_REQUEST, e))?);
            }
            "start_at" => {
                let t = field
                    .text()
                    .await
                    .map_err(|e| (StatusCode::BAD_REQUEST, format!("read start_at: {e}")))?;
                start_at = Some(
                    t.trim()
                        .parse::<f64>()
                        .map_err(|e| (StatusCode::BAD_REQUEST, format!("start_at: {e}")))?,
                );
            }
            other => {
                warn!(field = %other, "ignoring unknown multipart field");
            }
        }
    }

    let bytes = midi_bytes.ok_or((
        StatusCode::BAD_REQUEST,
        "missing 'file' field (MIDI upload)".to_string(),
    ))?;
    let score = midi::load_from_bytes(&bytes, filename.clone())
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("parse MIDI: {e:#}")))?;
    let policy = policy.unwrap_or(s.config.priority_algorithm);

    let duration = score.duration;
    let notes = score.notes.len();
    let title = score.title.clone();

    info!(notes, duration, ?policy, start_at, ?filename, "POST /play");
    s.engine
        .send(Command::Play {
            score,
            policy,
            start_at_epoch: start_at,
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({
        "ok": true,
        "notes": notes,
        "duration_sec": duration,
        "title": title,
        "policy": policy,
        "start_at": start_at,
    })))
}

async fn post_play_part(
    State(s): State<AppState>,
    Json(req): Json<PlayPartRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let policy = req.policy.unwrap_or(s.config.priority_algorithm);
    let notes = req.score.notes.len();
    let duration = req.score.duration;
    info!(notes, duration, ?policy, start_at = req.start_at, "POST /play_part");
    s.engine
        .send(Command::Play {
            score: req.score,
            policy,
            start_at_epoch: req.start_at,
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({"ok": true, "notes": notes, "duration_sec": duration})))
}

async fn post_orchestra_play(
    State(s): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let mut midi_bytes: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    // OrchestraOpts をフィールドごとに集める
    let mut opts_json = serde_json::Map::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("multipart: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                filename = field.file_name().map(|s| s.to_string());
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| (StatusCode::BAD_REQUEST, format!("read file: {e}")))?;
                midi_bytes = Some(data.to_vec());
            }
            "include_self" | "lead_time_sec" | "start_at" | "policy" | "node_order"
            | "voicing" => {
                let t = field
                    .text()
                    .await
                    .map_err(|e| (StatusCode::BAD_REQUEST, format!("read {name}: {e}")))?;
                // 型に応じて JSON 化
                let v = match name.as_str() {
                    "include_self" => match t.trim() {
                        "true" | "1" | "yes" => serde_json::Value::Bool(true),
                        "false" | "0" | "no" => serde_json::Value::Bool(false),
                        _ => serde_json::Value::String(t),
                    },
                    "lead_time_sec" | "start_at" => t
                        .trim()
                        .parse::<f64>()
                        .map(|x| serde_json::json!(x))
                        .map_err(|e| {
                            (StatusCode::BAD_REQUEST, format!("parse {name}: {e}"))
                        })?,
                    "node_order" => {
                        let v: Vec<String> = t
                            .split(',')
                            .map(|x| x.trim().to_string())
                            .filter(|x| !x.is_empty())
                            .collect();
                        serde_json::json!(v)
                    }
                    _ => serde_json::Value::String(t),
                };
                opts_json.insert(name, v);
            }
            other => warn!(field = %other, "ignoring unknown multipart field"),
        }
    }

    let opts: OrchestraOpts = serde_json::from_value(serde_json::Value::Object(opts_json))
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("opts: {e}")))?;

    let bytes = midi_bytes.ok_or((
        StatusCode::BAD_REQUEST,
        "missing 'file' field (MIDI upload)".to_string(),
    ))?;
    let score = midi::load_from_bytes(&bytes, filename.clone())
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("parse MIDI: {e:#}")))?;
    let policy = opts.policy.unwrap_or(s.config.priority_algorithm);

    // ノード順を確定
    let peers = s.peers.snapshot();
    let all_candidates: Vec<beep_common::node::NodeInfo> =
        std::iter::once((*s.self_info).clone())
            .chain(peers.iter().cloned())
            .collect();

    let nodes: Vec<beep_common::node::NodeInfo> = if let Some(order) = opts.node_order.as_ref() {
        // 明示指定: UUID または name で解決。未知は 400。
        let mut selected: Vec<beep_common::node::NodeInfo> = Vec::new();
        for id in order {
            let m = all_candidates
                .iter()
                .find(|n| n.uuid.to_string() == *id || n.name == *id);
            match m {
                Some(n) => selected.push(n.clone()),
                None => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        format!(
                            "unknown node in node_order: '{id}' (candidates: {})",
                            all_candidates
                                .iter()
                                .map(|n| n.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                }
            }
        }
        selected
    } else {
        crate::orchestra::stable_order(&peers, &s.self_info, opts.include_self)
    };

    if nodes.is_empty() {
        return Err((
            StatusCode::CONFLICT,
            "no nodes available (self excluded and no peers discovered)".into(),
        ));
    }

    // ボイス分割 (pitch or track)
    let n_voices = nodes.len();
    let voices = beep_common::assign_voicing(&score, n_voices, opts.voicing);

    // start_at 決定
    use std::time::{SystemTime, UNIX_EPOCH};
    let start_at = opts.start_at.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
            + opts.lead_time_sec.max(0.5)
    });

    info!(
        nodes = nodes.len(),
        voices = voices.len(),
        start_at,
        ?policy,
        voicing = ?opts.voicing,
        "orchestra: dispatching voices"
    );

    // 各ノードへディスパッチ (self は engine 経由)
    let self_uuid = s.self_info.uuid;
    let outcomes = crate::orchestra::dispatch_voices(
        &nodes,
        voices,
        policy,
        Some(start_at),
        self_uuid,
        &s.engine,
    )
    .await;

    Ok(Json(json!({
        "ok": true,
        "start_at": start_at,
        "nodes": nodes.iter().map(|n| json!({
            "uuid": n.uuid.to_string(),
            "name": n.name,
            "http": format!("http://{}:{}", n.ip, n.http_port),
        })).collect::<Vec<_>>(),
        "outcomes": outcomes,
    })))
}

async fn post_update(
    State(s): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let cfg = &s.config.update;
    if !cfg.enabled || cfg.token.is_empty() {
        return Err((StatusCode::FORBIDDEN, "OTA update is disabled".into()));
    }
    let supplied = headers
        .get("x-update-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    // 定数時間比較にしたいが、まずは長さ+eqで十分
    if supplied.len() != cfg.token.len() || supplied != cfg.token {
        return Err((StatusCode::UNAUTHORIZED, "invalid X-Update-Token".into()));
    }

    let mut binary_bytes: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("multipart: {e}")))?
    {
        if field.name() == Some("binary") {
            let data = field
                .bytes()
                .await
                .map_err(|e| (StatusCode::BAD_REQUEST, format!("read binary: {e}")))?;
            binary_bytes = Some(data.to_vec());
        }
    }
    let bytes = binary_bytes.ok_or((
        StatusCode::BAD_REQUEST,
        "missing 'binary' field".to_string(),
    ))?;
    info!(size = bytes.len(), "POST /update: received binary");

    let outcome = match crate::updater::apply_update(&cfg.binary_path, &bytes, cfg.verify_with_version) {
        Ok(o) => o,
        Err(e) => {
            warn!(error = %e, "update failed");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("update failed: {e:#}")));
        }
    };

    // 応答返却後に再起動を走らせる
    if let Err(e) = crate::updater::schedule_restart(&cfg.restart_command) {
        warn!(error = %e, "schedule_restart failed");
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("swapped but restart failed: {e:#}"),
        ));
    }

    Ok(Json(json!({
        "ok": true,
        "old_size": outcome.old_size,
        "new_size": outcome.new_size,
        "version": outcome.version_output,
        "backup": outcome.backup_path.display().to_string(),
        "restart_scheduled_in_ms": 500,
    })))
}

async fn post_stop(State(s): State<AppState>) -> impl IntoResponse {
    let _ = s.engine.send(Command::Stop);
    Json(json!({"ok": true}))
}
async fn post_pause(State(s): State<AppState>) -> impl IntoResponse {
    let _ = s.engine.send(Command::Pause);
    Json(json!({"ok": true}))
}
async fn post_resume(State(s): State<AppState>) -> impl IntoResponse {
    let _ = s.engine.send(Command::Resume);
    Json(json!({"ok": true}))
}

fn parse_policy(s: &str) -> Result<SelectPolicy, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "highest" => Ok(SelectPolicy::Highest),
        "lowest" => Ok(SelectPolicy::Lowest),
        "first" => Ok(SelectPolicy::First),
        "last" => Ok(SelectPolicy::Last),
        other => Err(format!(
            "unknown policy '{other}' (use highest|lowest|first|last)"
        )),
    }
}
