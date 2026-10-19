//! beep-player: 単体でスコアを再生する CLI。

mod backend;
mod config;
mod player;

use anyhow::{Context, Result};
use beep_common::{midi, SelectPolicy};
use clap::{Parser, ValueEnum};
use std::path::PathBuf;
use tracing::info;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PolicyArg {
    Highest,
    Lowest,
    First,
    Last,
}

impl From<PolicyArg> for SelectPolicy {
    fn from(v: PolicyArg) -> Self {
        match v {
            PolicyArg::Highest => SelectPolicy::Highest,
            PolicyArg::Lowest => SelectPolicy::Lowest,
            PolicyArg::First => SelectPolicy::First,
            PolicyArg::Last => SelectPolicy::Last,
        }
    }
}

/// MIDI ファイルを beep で単体再生する
#[derive(Debug, Parser)]
#[command(name = "beep-player", version, about, long_about = None)]
struct Args {
    /// 再生する MIDI (.mid) ファイル
    file: PathBuf,

    /// 設定ファイル (TOML)。未指定ならデフォルト設定。
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// 選音アルゴリズム
    #[arg(long, value_enum)]
    policy: Option<PolicyArg>,

    /// 実際には beep を呼ばず、鳴らす予定の音をログにだけ出す
    #[arg(long)]
    dry_run: bool,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    let mut cfg = if let Some(p) = &args.config {
        config::PlayerConfig::from_file(p)
            .with_context(|| format!("load config: {}", p.display()))?
    } else {
        config::PlayerConfig::default()
    };
    if args.dry_run {
        cfg.backend.dry_run = true;
    }
    if let Some(p) = args.policy {
        cfg.priority_algorithm = p.into();
    }

    info!(
        file = %args.file.display(),
        policy = ?cfg.priority_algorithm,
        dry_run = cfg.backend.dry_run,
        "loading score"
    );
    let score = midi::load_from_path(&args.file)
        .with_context(|| format!("load MIDI: {}", args.file.display()))?;
    info!(
        notes = score.notes.len(),
        duration = score.duration,
        title = score.title.as_deref().unwrap_or(""),
        "score loaded"
    );

    let backend = backend::build(&cfg.backend)?;
    let mut player = player::MonoPlayer::new(backend, cfg.priority_algorithm);
    player.play(&score)?;
    Ok(())
}
