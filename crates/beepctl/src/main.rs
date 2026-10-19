//! beepctl: 複数の beepd ノードを一括操作する CLI。
//!
//! 例:
//!   beepctl status http://nodeA:8642 http://nodeB:8642 http://nodeC:8642
//!   beepctl update -t $TOKEN target/release/beepd \
//!       http://nodeA:8642 http://nodeB:8642 http://nodeC:8642
//!   beepctl play song.mid http://nodeA:8642 --start-in 3.0
//!   beepctl orchestra song.mid http://nodeA:8642 --include-self --lead 3

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures::future::join_all;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Parser)]
#[command(name = "beepctl", version, about = "control multiple beepd nodes", long_about = None)]
struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// GET /status を並列に叩いて要約表示
    Status {
        /// 対象 URL (http://host:port)
        #[arg(required = true)]
        hosts: Vec<String>,
    },
    /// GET /nodes を並列に叩いて peer 一覧を要約表示
    Nodes {
        #[arg(required = true)]
        hosts: Vec<String>,
    },
    /// バイナリを配布して各ノードを再起動 (OTA)
    Update {
        /// 送信するバイナリ (例: target/release/beepd)
        #[arg(short, long)]
        binary: PathBuf,
        /// X-Update-Token
        #[arg(short = 't', long, env = "BEEP_UPDATE_TOKEN")]
        token: String,
        /// 更新後の再起動待ちタイムアウト秒 (0 なら待たない)
        #[arg(long, default_value_t = 30.0)]
        wait_ready: f64,
        #[arg(required = true)]
        hosts: Vec<String>,
    },
    /// MIDI をアップロードして単ノード演奏
    Play {
        file: PathBuf,
        /// 開始までの余裕秒 (現在時刻 + start_in を start_at に)。0 なら即時
        #[arg(long, default_value_t = 0.0)]
        start_in: f64,
        #[arg(long, default_value = "highest")]
        policy: String,
        #[arg(required = true)]
        hosts: Vec<String>,
    },
    /// MIDI をアップロードしてオーケストラ演奏 (master 1 台へ)
    Orchestra {
        file: PathBuf,
        /// マスターとして叩く URL (1 個だけ)
        master: String,
        /// マスター自身も 1 ボイスとして参加 (--nodes 未指定時のみ有効)
        #[arg(long, default_value_t = true)]
        include_self: bool,
        /// lead_time_sec
        #[arg(long, default_value_t = 2.0)]
        lead: f64,
        /// 明示的なノード順。UUID もしくは node_name をカンマ区切り。
        /// 例: --nodes nodeB,nodeC,nodeA
        /// voice 0 が最初のノード、以下順次。
        #[arg(long, value_delimiter = ',')]
        nodes: Option<Vec<String>>,
        /// パート分配戦略: pitch (デフォルト、高音優先) / track (SMFトラック順)
        #[arg(long, default_value = "pitch")]
        voicing: String,
    },
    /// 全ノードに /stop を投げる
    Stop {
        #[arg(required = true)]
        hosts: Vec<String>,
    },
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
    match args.cmd {
        Cmd::Status { hosts } => cmd_status(hosts).await,
        Cmd::Nodes { hosts } => cmd_nodes(hosts).await,
        Cmd::Update {
            binary,
            token,
            wait_ready,
            hosts,
        } => cmd_update(binary, token, wait_ready, hosts).await,
        Cmd::Play {
            file,
            start_in,
            policy,
            hosts,
        } => cmd_play(file, start_in, policy, hosts).await,
        Cmd::Orchestra {
            file,
            master,
            include_self,
            lead,
            nodes,
            voicing,
        } => cmd_orchestra(file, master, include_self, lead, nodes, voicing).await,
        Cmd::Stop { hosts } => cmd_stop(hosts).await,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("build client")
}

fn normalize_url(s: &str) -> String {
    if s.starts_with("http://") || s.starts_with("https://") {
        s.trim_end_matches('/').to_string()
    } else {
        format!("http://{}", s.trim_end_matches('/'))
    }
}

async fn cmd_status(hosts: Vec<String>) -> Result<()> {
    let c = client();
    let fs = hosts.iter().map(|h| {
        let c = c.clone();
        let url = format!("{}/status", normalize_url(h));
        async move {
            let r = c.get(&url).send().await;
            (url, r)
        }
    });
    for (url, r) in join_all(fs).await {
        match r {
            Ok(resp) => {
                let text = resp.text().await.unwrap_or_default();
                println!("{}\n  {}", url, text);
            }
            Err(e) => println!("{}\n  ERR {e}", url),
        }
    }
    Ok(())
}

async fn cmd_nodes(hosts: Vec<String>) -> Result<()> {
    let c = client();
    let fs = hosts.iter().map(|h| {
        let c = c.clone();
        let url = format!("{}/nodes", normalize_url(h));
        async move {
            let r = c.get(&url).send().await;
            (url, r)
        }
    });
    for (url, r) in join_all(fs).await {
        match r {
            Ok(resp) => {
                let v: serde_json::Value = match resp.json().await {
                    Ok(v) => v,
                    Err(e) => {
                        println!("{}\n  ERR parse: {e}", url);
                        continue;
                    }
                };
                let self_name = v["self_node"]["name"].as_str().unwrap_or("?");
                let self_uuid = v["self_node"]["uuid"].as_str().unwrap_or("?");
                let peers = v["peers"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|p| {
                                format!(
                                    "{}@{}:{}",
                                    p["name"].as_str().unwrap_or("?"),
                                    p["ip"].as_str().unwrap_or("?"),
                                    p["http_port"].as_u64().unwrap_or(0)
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                println!(
                    "{}\n  self: {} ({})\n  peers ({}): {}",
                    url,
                    self_name,
                    self_uuid,
                    peers.len(),
                    peers.join(", ")
                );
            }
            Err(e) => println!("{}\n  ERR {e}", url),
        }
    }
    Ok(())
}

async fn cmd_update(
    binary: PathBuf,
    token: String,
    wait_ready: f64,
    hosts: Vec<String>,
) -> Result<()> {
    let bytes = std::fs::read(&binary).with_context(|| format!("read {}", binary.display()))?;
    println!(
        "-- OTA update: {} bytes to {} host(s) --",
        bytes.len(),
        hosts.len()
    );
    let c = client();
    let fs = hosts.iter().map(|h| {
        let c = c.clone();
        let url = format!("{}/update", normalize_url(h));
        let host_url = normalize_url(h);
        let token = token.clone();
        let bytes = bytes.clone();
        async move {
            let form = reqwest::multipart::Form::new().part(
                "binary",
                reqwest::multipart::Part::bytes(bytes).file_name("beepd"),
            );
            let started = Instant::now();
            let r = c
                .post(&url)
                .header("X-Update-Token", token)
                .multipart(form)
                .send()
                .await;
            let elapsed = started.elapsed();
            (url, host_url, r, elapsed)
        }
    });
    let results = join_all(fs).await;
    let mut ok_hosts: Vec<String> = Vec::new();
    for (url, host_url, r, elapsed) in results {
        match r {
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if status.is_success() {
                    println!("{}  OK  ({} ms)\n  {}", url, elapsed.as_millis(), text);
                    ok_hosts.push(host_url);
                } else {
                    println!("{}  FAIL {}  ({} ms)\n  {}", url, status, elapsed.as_millis(), text);
                }
            }
            Err(e) => println!("{}  ERR ({} ms): {e}", url, elapsed.as_millis()),
        }
    }

    // 再起動待ち: /status が 200 で返るまでポーリング
    if wait_ready > 0.0 && !ok_hosts.is_empty() {
        println!("-- waiting up to {}s for restart --", wait_ready);
        let deadline = Instant::now() + Duration::from_secs_f64(wait_ready);
        let mut pending: Vec<String> = ok_hosts.clone();
        while Instant::now() < deadline && !pending.is_empty() {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let c = client();
            let checks = pending.iter().map(|h| {
                let c = c.clone();
                let url = format!("{}/status", h);
                let host = h.clone();
                async move {
                    let r = tokio::time::timeout(Duration::from_secs(2), c.get(&url).send()).await;
                    let ok = matches!(r, Ok(Ok(resp)) if resp.status().is_success());
                    (host, ok)
                }
            });
            let checks_done = join_all(checks).await;
            let still_pending: Vec<String> = checks_done
                .into_iter()
                .filter_map(|(h, ok)| if ok { None } else { Some(h) })
                .collect();
            let now_ready: Vec<String> = pending
                .iter()
                .filter(|h| !still_pending.contains(h))
                .cloned()
                .collect();
            for h in &now_ready {
                println!("  ready: {}", h);
            }
            pending = still_pending;
        }
        for h in pending {
            println!("  STILL NOT READY: {}", h);
        }
    }

    Ok(())
}

async fn cmd_play(
    file: PathBuf,
    start_in: f64,
    policy: String,
    hosts: Vec<String>,
) -> Result<()> {
    let bytes = std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
    let start_at = if start_in > 0.0 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs_f64();
        Some(now + start_in)
    } else {
        None
    };
    let c = client();
    let fs = hosts.iter().map(|h| {
        let c = c.clone();
        let url = format!("{}/play", normalize_url(h));
        let bytes = bytes.clone();
        let policy = policy.clone();
        let filename = file
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("upload.mid")
            .to_string();
        async move {
            let mut form = reqwest::multipart::Form::new()
                .part(
                    "file",
                    reqwest::multipart::Part::bytes(bytes).file_name(filename),
                )
                .text("policy", policy);
            if let Some(sa) = start_at {
                form = form.text("start_at", format!("{sa}"));
            }
            let r = c.post(&url).multipart(form).send().await;
            (url, r)
        }
    });
    for (url, r) in join_all(fs).await {
        match r {
            Ok(resp) => {
                let t = resp.text().await.unwrap_or_default();
                println!("{}\n  {}", url, t);
            }
            Err(e) => println!("{}\n  ERR {e}", url),
        }
    }
    Ok(())
}

async fn cmd_orchestra(
    file: PathBuf,
    master: String,
    include_self: bool,
    lead: f64,
    nodes: Option<Vec<String>>,
    voicing: String,
) -> Result<()> {
    let bytes = std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
    let url = format!("{}/orchestra/play", normalize_url(&master));
    let filename = file
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("upload.mid")
        .to_string();
    let mut form = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(bytes).file_name(filename),
        )
        .text("lead_time_sec", format!("{lead}"))
        .text("include_self", if include_self { "true" } else { "false" })
        .text("voicing", voicing.clone());
    if let Some(order) = nodes {
        form = form.text("node_order", order.join(","));
    }
    let c = client();
    let r = c.post(&url).multipart(form).send().await?;
    let status = r.status();
    let t = r.text().await.unwrap_or_default();
    println!("{}\n  {}\n  {}", url, status, t);
    Ok(())
}

async fn cmd_stop(hosts: Vec<String>) -> Result<()> {
    let c = client();
    let fs = hosts.iter().map(|h| {
        let c = c.clone();
        let url = format!("{}/stop", normalize_url(h));
        async move {
            let r = c.post(&url).send().await;
            (url, r)
        }
    });
    for (url, r) in join_all(fs).await {
        match r {
            Ok(resp) => println!("{}  {}", url, resp.status()),
            Err(e) => println!("{}  ERR {e}", url),
        }
    }
    Ok(())
}
