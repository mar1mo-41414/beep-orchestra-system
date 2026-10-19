//! static_peers を周期的に GET /nodes して peer table に流し込む。
//!
//! discovery (UDP マルチキャスト) が届かない環境 (別サブネット, Tailscale,
//! WireGuard 等) 向けのフォールバック。

use beep_common::node::NodeInfo;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::state::parking_lot_ish::PeerTable;

#[derive(Debug, Deserialize)]
struct NodesResponse {
    self_node: NodeInfo,
}

pub fn spawn(endpoints: Vec<String>, poll_ms: u64, self_uuid: Uuid, peers: Arc<PeerTable>) {
    if endpoints.is_empty() {
        return;
    }
    let interval = Duration::from_millis(poll_ms.max(500));
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "static_peers: reqwest client build failed");
                return;
            }
        };
        let mut tick = tokio::time::interval(interval);
        loop {
            tick.tick().await;
            for ep in &endpoints {
                let url = format!("http://{}/nodes", ep);
                match client.get(&url).send().await {
                    Ok(r) if r.status().is_success() => match r.json::<NodesResponse>().await {
                        Ok(nodes) => {
                            if nodes.self_node.uuid == self_uuid {
                                debug!(url, "static_peer is self, skipping");
                                continue;
                            }
                            // 相手は listen_addr=0.0.0.0 の場合 self_node.ip="0.0.0.0" を返すため、
                            // 実際に到達できたエンドポイント側の ip/port で上書きする
                            let mut info = nodes.self_node;
                            if let Some((h, p)) = ep.rsplit_once(':') {
                                if let Ok(ip) = h.parse::<std::net::IpAddr>() {
                                    info.ip = ip;
                                }
                                if let Ok(port) = p.parse::<u16>() {
                                    info.http_port = port;
                                }
                            }
                            debug!(
                                url,
                                peer = %info.name,
                                ip = %info.ip,
                                "static_peer upsert"
                            );
                            peers.upsert(info);
                        }
                        Err(e) => warn!(url, error = %e, "static_peer: parse /nodes failed"),
                    },
                    Ok(r) => warn!(url, status = %r.status(), "static_peer: bad status"),
                    Err(e) => warn!(url, error = %e, "static_peer: request failed"),
                }
            }
        }
    });
}
