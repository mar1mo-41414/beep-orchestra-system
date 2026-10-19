//! オーケストラモード: マスターが MIDI をパートに分割し、各ノードへ配布する。

use beep_common::node::NodeInfo;
use beep_common::score::Score;
use beep_common::SelectPolicy;
use serde::Serialize;
use std::time::Duration;
use tracing::{info, warn};

use crate::api::PlayPartRequest;

#[derive(Debug, Serialize)]
pub struct DispatchOutcome {
    pub uuid: String,
    pub name: String,
    pub http: String,
    pub notes: usize,
    pub duration_sec: f64,
    pub voice_index: usize,
    pub ok: bool,
    pub error: Option<String>,
}

/// 各ノードへ voice ごとの Score を配布する。
///
/// * `nodes` は「マスターも含めて演奏に参加する順序」。1 台目 = 最高音ボイス。
/// * `voices[i]` は voice_i (0 が最高音)。voices.len() <= nodes.len() 前提。
pub async fn dispatch_voices(
    nodes: &[NodeInfo],
    voices: Vec<Score>,
    policy: SelectPolicy,
    start_at: Option<f64>,
    self_uuid: uuid::Uuid,
    self_engine: &crate::engine::Engine,
) -> Vec<DispatchOutcome> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build reqwest client");

    let mut out = Vec::with_capacity(voices.len());
    for (i, (score, node)) in voices.into_iter().zip(nodes.iter()).enumerate() {
        let notes = score.notes.len();
        let duration = score.duration;
        let http = format!("http://{}:{}", node.ip, node.http_port);
        let is_self = node.uuid == self_uuid;
        info!(
            voice = i,
            node = %node.name,
            uuid = %node.uuid,
            is_self,
            notes,
            duration,
            "assigning voice"
        );

        let (ok, error) = if is_self {
            match self_engine.send(crate::engine::Command::Play {
                score,
                policy,
                start_at_epoch: start_at,
            }) {
                Ok(_) => (true, None),
                Err(e) => (false, Some(e.to_string())),
            }
        } else {
            let req = PlayPartRequest {
                score,
                policy: Some(policy),
                start_at,
            };
            let url = format!("{}/play_part", http);
            match client.post(&url).json(&req).send().await {
                Ok(r) if r.status().is_success() => (true, None),
                Ok(r) => (false, Some(format!("HTTP {}", r.status()))),
                Err(e) => {
                    warn!(node=%node.name, error=%e, "dispatch failed");
                    (false, Some(e.to_string()))
                }
            }
        };
        out.push(DispatchOutcome {
            uuid: node.uuid.to_string(),
            name: node.name.clone(),
            http,
            notes,
            duration_sec: duration,
            voice_index: i,
            ok,
            error,
        });
    }
    let _ = client;
    out
}

/// nodes と voices を並べる:
///   - 参加順: self を先頭に、残りは name ソートで安定化
///   - 長さは min(voices.len(), nodes.len())
pub fn stable_order(
    peers: &[NodeInfo],
    self_info: &NodeInfo,
    include_self: bool,
) -> Vec<NodeInfo> {
    let mut all: Vec<NodeInfo> = if include_self {
        std::iter::once(self_info.clone())
            .chain(peers.iter().cloned())
            .collect()
    } else {
        peers.to_vec()
    };
    // 重複除去 (UUID キー)
    all.sort_by(|a, b| {
        // self を先頭にしたい: self_info と同じ UUID が最優先
        let a_self = a.uuid == self_info.uuid;
        let b_self = b.uuid == self_info.uuid;
        b_self.cmp(&a_self).then_with(|| a.name.cmp(&b.name))
    });
    let mut seen = std::collections::HashSet::new();
    all.retain(|n| seen.insert(n.uuid));
    all
}

