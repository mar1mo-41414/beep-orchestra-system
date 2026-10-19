//! LAN ノード発見: UDP マルチキャストによる周期告知 & 受信。
//!
//! - Announcer: 自ノード情報を JSON にして周期送信
//! - Listener : 受信した告知を PeerTable に upsert (TTL で自然消滅)
//!
//! ワイヤフォーマット (JSON, UTF-8):
//! ```json
//! {
//!   "kind": "announce",
//!   "uuid": "...",
//!   "name": "...",
//!   "ip":   "192.0.2.10",
//!   "http_port": 8642,
//!   "capabilities": ["beep"],
//!   "ts": 1750000000.123
//! }
//! ```

use anyhow::{Context, Result};
use beep_common::node::NodeInfo;
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::config::DiscoveryConfig;
use crate::state::parking_lot_ish::PeerTable;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Announce {
    kind: String,
    uuid: Uuid,
    name: String,
    ip: String,
    http_port: u16,
    capabilities: Vec<String>,
    ts: f64,
}

/// Announcer と Listener を tokio タスクとして起動する。
///
/// - `self_info` は自ノードとして告知される情報
/// - `peers` は Listener が受信したピアを反映する
pub fn spawn(cfg: DiscoveryConfig, self_info: Arc<NodeInfo>, peers: Arc<PeerTable>) -> Result<()> {
    if !cfg.enabled {
        info!("discovery disabled");
        return Ok(());
    }

    let mcast_ip: Ipv4Addr = cfg
        .multicast_addr
        .parse()
        .with_context(|| format!("parse multicast_addr: {}", cfg.multicast_addr))?;
    let mcast_port = cfg.multicast_port;

    // 送信/受信ソケットは分けて生成
    let listener_sock = make_multicast_listener(mcast_ip, mcast_port)?;
    let sender_sock = make_multicast_sender()?;

    info!(
        %mcast_ip,
        mcast_port,
        interval_ms = cfg.announce_interval_ms,
        "discovery: starting"
    );

    // Listener
    let peers_l = peers.clone();
    let self_uuid = self_info.uuid;
    tokio::spawn(async move {
        if let Err(e) = run_listener(listener_sock, peers_l, self_uuid).await {
            warn!(error = %e, "discovery listener stopped");
        }
    });

    // Announcer
    let self_a = self_info.clone();
    let interval = Duration::from_millis(cfg.announce_interval_ms.max(200));
    tokio::spawn(async move {
        if let Err(e) = run_announcer(sender_sock, mcast_ip, mcast_port, self_a, interval).await {
            warn!(error = %e, "discovery announcer stopped");
        }
    });

    let _ = peers;
    Ok(())
}

/// static_peers / discovery いずれの経路にも共通の GC タスクを起動する。
/// TTL は 9 秒固定 (poll/announce 間隔 3s の 3 倍)。
pub fn spawn_gc(peers: std::sync::Arc<crate::state::parking_lot_ish::PeerTable>) {
    let ttl = std::time::Duration::from_secs(9);
    let interval = std::time::Duration::from_secs(3);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        loop {
            tick.tick().await;
            peers.gc_older_than(ttl);
        }
    });
}

fn make_multicast_listener(mcast_ip: Ipv4Addr, port: u16) -> Result<UdpSocket> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .context("create UDP socket")?;
    sock.set_reuse_address(true)
        .context("set_reuse_address")?;
    #[cfg(unix)]
    {
        sock.set_reuse_port(true).context("set_reuse_port")?;
    }
    let bind: SocketAddr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into();
    sock.bind(&SockAddr::from(bind)).context("bind udp")?;
    sock.set_multicast_loop_v4(true)
        .context("set_multicast_loop_v4")?;
    sock.join_multicast_v4(&mcast_ip, &Ipv4Addr::UNSPECIFIED)
        .context("join_multicast_v4")?;
    sock.set_nonblocking(true).context("set_nonblocking")?;
    let std_sock: std::net::UdpSocket = sock.into();
    UdpSocket::from_std(std_sock).context("wrap tokio UdpSocket (listener)")
}

fn make_multicast_sender() -> Result<UdpSocket> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .context("create UDP send socket")?;
    sock.set_reuse_address(true)?;
    #[cfg(unix)]
    {
        sock.set_reuse_port(true)?;
    }
    // 送信ソケットは適当なエフェメラルポートに bind
    let bind: SocketAddr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0).into();
    sock.bind(&SockAddr::from(bind)).context("bind send udp")?;
    sock.set_multicast_loop_v4(true)?;
    sock.set_nonblocking(true)?;
    let std_sock: std::net::UdpSocket = sock.into();
    UdpSocket::from_std(std_sock).context("wrap tokio UdpSocket (sender)")
}

async fn run_announcer(
    sock: UdpSocket,
    mcast_ip: Ipv4Addr,
    mcast_port: u16,
    self_info: Arc<NodeInfo>,
    interval: Duration,
) -> Result<()> {
    let dest = SocketAddr::from(SocketAddrV4::new(mcast_ip, mcast_port));
    let mut tick = tokio::time::interval(interval);
    loop {
        tick.tick().await;
        let msg = Announce {
            kind: "announce".into(),
            uuid: self_info.uuid,
            name: self_info.name.clone(),
            ip: self_info.ip.to_string(),
            http_port: self_info.http_port,
            capabilities: self_info.capabilities.clone(),
            ts: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0),
        };
        let json = serde_json::to_vec(&msg).unwrap();
        match sock.send_to(&json, dest).await {
            Ok(_) => debug!(?dest, bytes = json.len(), "announced"),
            Err(e) => warn!(error = %e, "announce send failed"),
        }
    }
}

async fn run_listener(sock: UdpSocket, peers: Arc<PeerTable>, self_uuid: Uuid) -> Result<()> {
    let mut buf = vec![0u8; 4096];
    loop {
        let (n, from) = match sock.recv_from(&mut buf).await {
            Ok(x) => x,
            Err(e) => {
                warn!(error = %e, "listener recv failed");
                continue;
            }
        };
        let payload = &buf[..n];
        let announce: Announce = match serde_json::from_slice(payload) {
            Ok(a) => a,
            Err(e) => {
                debug!(error = %e, from = %from, "ignored non-JSON announce");
                continue;
            }
        };
        if announce.kind != "announce" {
            continue;
        }
        if announce.uuid == self_uuid {
            // ループバックした自分自身の告知は無視
            continue;
        }
        // ip フィールドが 0.0.0.0 の場合 (listen_addr=0.0.0.0) は
        // 到達元アドレスを採用する
        let observed_ip = match announce.ip.parse::<std::net::IpAddr>() {
            Ok(ip) if !ip.is_unspecified() => ip,
            _ => from.ip(),
        };
        let info = NodeInfo {
            uuid: announce.uuid,
            name: announce.name.clone(),
            ip: observed_ip,
            http_port: announce.http_port,
            capabilities: announce.capabilities.clone(),
        };
        peers.upsert(info);
    }
}
