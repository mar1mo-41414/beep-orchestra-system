//! beepd: HTTP API + (Phase3以降で) LAN 同期を担うデーモン。

mod api;
mod backend;
mod config;
mod discovery;
mod engine;
mod orchestra;
mod state;
mod static_peers;
mod updater;

use anyhow::{Context, Result};
use beep_common::node::NodeInfo;
use clap::Parser;
use std::net::IpAddr;
use std::sync::Arc;
use tracing::info;

use crate::config::DaemonConfig;
use crate::engine::Engine;
use crate::state::AppState;

#[derive(Debug, Parser)]
#[command(name = "beepd", version, about = "beep-orchestra daemon", long_about = None)]
struct Args {
    /// 設定ファイル (TOML)
    #[arg(short, long)]
    config: Option<std::path::PathBuf>,
    /// 待受ポートを CLI から上書き
    #[arg(long)]
    port: Option<u16>,
    /// 実際の beep を叩かず dry-run
    #[arg(long)]
    dry_run: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let mut cfg = if let Some(p) = &args.config {
        DaemonConfig::from_file(p).with_context(|| format!("load config: {}", p.display()))?
    } else {
        DaemonConfig::default()
    };
    if let Some(p) = args.port {
        cfg.listen_port = p;
    }
    if args.dry_run {
        cfg.backend.dry_run = true;
    }
    let cfg = Arc::new(cfg);

    let engine = Engine::spawn(cfg.backend.clone());

    let listen_ip: IpAddr = cfg.listen_addr.parse().context("parse listen_addr")?;
    let bind_addr = std::net::SocketAddr::new(listen_ip, cfg.listen_port);

    let self_info = Arc::new(NodeInfo::new(&cfg.node_name, listen_ip, cfg.listen_port));

    let peers = Arc::new(state::parking_lot_ish::PeerTable::default());

    // LAN discovery を起動 (disabled 時はすぐ返る)
    discovery::spawn(cfg.discovery.clone(), self_info.clone(), peers.clone())?;

    // 別サブネット/Tailscale/WireGuard 用の明示ピア (poll based)
    static_peers::spawn(
        cfg.static_peers.clone(),
        cfg.static_peer_poll_ms,
        self_info.uuid,
        peers.clone(),
    );

    // TTL による peer 掃除 (discovery/static_peers どちらの経路でも共通)
    discovery::spawn_gc(peers.clone());

    let app_state = AppState {
        config: cfg.clone(),
        self_info: self_info.clone(),
        engine,
        peers,
    };

    let router = api::router(app_state);

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("bind {}", bind_addr))?;
    info!(
        addr = %bind_addr,
        node = %cfg.node_name,
        uuid = %self_info.uuid,
        master_mode = cfg.master_mode,
        dry_run = cfg.backend.dry_run,
        "beepd listening"
    );
    // OTA アップデート後の初回起動を可視化するためのバナー
    info!(banner = "beepd v0.2.0 (OTA-enabled)", "startup complete");

    axum::serve(listener, router)
        .await
        .context("axum serve")?;
    Ok(())
}
