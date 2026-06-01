use anyhow::Context;
use backend::{
    api::router,
    config::AppConfig,
    index::{
        LogSearchIndex, finish_cli_progress_line, rebuild_index_storage, set_cli_progress_started,
    },
    server::build_app_state,
    watcher::{IndexJob, reconcile_jobs},
};
use clap::{Parser, Subcommand};
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Instant};
use tracing::info;

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long, default_value = "config.toml")]
    config: String,
    #[arg(long, default_value = "frontend")]
    static_dir: PathBuf,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    ClearIndex,
    RebuildIndex,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let default_filter = match args.command {
        Some(Command::RebuildIndex) | Some(Command::ClearIndex) => "backend=warn,tower_http=warn",
        None => "backend=info,tower_http=info",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_filter.into()),
        )
        .init();

    let config = Arc::new(AppConfig::load(&args.config).context("failed to load config")?);

    match args.command {
        Some(Command::ClearIndex) => {
            rebuild_index_storage(&config.index.dir)?;
            println!("index storage cleared: {}", config.index.dir.display());
            info!(dir = %config.index.dir.display(), "index.storage_cleared");
            return Ok(());
        }
        Some(Command::RebuildIndex) => {
            let started = Instant::now();
            set_cli_progress_started(started);
            rebuild_index_storage(&config.index.dir)?;
            let index = LogSearchIndex::open_or_create(&config.index.dir)?;
            let mut total_lines = 0_usize;
            println!(
                "Rebuilding index: {} files -> {}",
                config.files.len(),
                config.index.dir.display()
            );
            for job in reconcile_jobs(&config) {
                let job_started = Instant::now();
                let (source_id, path, kind, lines) = match job {
                    IndexJob::Hot { source_id, path } => {
                        println!("  • {} ({})", display_file_name(&path), source_id);
                        let lines = index.sync_file(&source_id, &path)?;
                        (source_id, path, "hot", lines)
                    }
                    IndexJob::Compressed {
                        source_id,
                        path,
                        kind,
                    } => {
                        let kind_label = kind.as_str();
                        println!(
                            "  • {} ({}, {})",
                            display_file_name(&path),
                            source_id,
                            kind_label
                        );
                        let lines = index.sync_compressed_file(&source_id, &path, kind_label)?;
                        (source_id, path, kind_label, lines)
                    }
                };
                total_lines += lines;
                finish_cli_progress_line();
                println!(
                    "    done: {} lines in {}",
                    lines,
                    format_duration(job_started.elapsed())
                );
                let _ = (source_id, path, kind);
            }
            println!(
                "Done: {} lines indexed in {}",
                total_lines,
                format_duration(started.elapsed())
            );
            info!(
                dir = %config.index.dir.display(),
                total_lines,
                "index.storage_rebuilt"
            );
            return Ok(());
        }
        None => {}
    }

    let index = Arc::new(LogSearchIndex::open_or_create(&config.index.dir)?);

    let addr: SocketAddr = config.server.addr.parse()?;
    let state = build_app_state(config, index);
    let app = router(state, args.static_dir);
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(%addr, "app.serving");
    axum::serve(listener, app).await?;
    Ok(())
}

fn display_file_name(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

fn format_duration(duration: std::time::Duration) -> String {
    let total_secs = duration.as_secs();
    let millis = duration.subsec_millis();
    if total_secs < 60 {
        return format!("{}.{:03}s", total_secs, millis);
    }

    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    format!("{minutes}m{seconds:02}s")
}
