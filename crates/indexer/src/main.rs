// SPDX-License-Identifier: AGPL-3.0-or-later
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use lessdugin_indexer::api;
use lessdugin_indexer::host::PoliteHost;
use lessdugin_indexer::ingest;
use lessdugin_indexer::live;
use lessdugin_indexer::store::Store;
use lessdugin_indexer::{Indexer, IndexerConfig};
use lessdugin_native::config::{Config, NetworkProfile, Paths};
use lessdugin_native::host::NativeHost;
use url::Url;

#[derive(Debug, Parser)]
#[command(
    name = "lessdugin-indexer",
    version,
    about = "Optional native lessdugin indexer"
)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:2584")]
    listen: SocketAddr,
    #[arg(long)]
    data_dir: PathBuf,
    /// Public did:web of this service (metadata only; the indexer has no signing authority).
    #[arg(long, default_value = "did:web:indexer.example.invalid")]
    did: String,
    #[arg(long)]
    poll: Option<Url>,
    #[arg(long, default_value_t = 5_000)]
    poll_interval_ms: u64,
    #[arg(long = "forum")]
    forums: Vec<String>,
    #[arg(long = "relay")]
    relays: Vec<Url>,
    #[arg(long = "jetstream")]
    jetstream: Vec<Url>,
    #[arg(long)]
    blocklist: Option<PathBuf>,
    #[arg(long, default_value = "https://github.com/lessdugin/lessdugin")]
    source_url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args = Args::parse();
    let paths = Paths::under(&args.data_dir);
    paths
        .ensure_dirs()
        .context("create indexer data directories")?;
    let mut config = Config::load(&paths.config_file).context("load indexer config")?;
    if let Some(poll) = &args.poll {
        let plc = match &config.network {
            NetworkProfile::Devnet(d) => d.plc.clone(),
            NetworkProfile::Production => Url::parse("https://plc.directory")?,
        };
        config.network = NetworkProfile::Devnet(lessdugin_native::config::DevnetProfile {
            plc,
            pds: poll.clone(),
            jetstream: None,
        });
    }
    let engine_config = lessdugin_native::engine::engine_config(&config);
    let host = NativeHost::open(&paths, &config).context("open native cache")?;
    let engine = lessdugin_core::engine::Engine::new(PoliteHost::new(host), engine_config);
    let store = Store::open(&paths.state_db()).context("open indexer side tables")?;
    let blocklist = load_blocklist(args.blocklist.as_ref())?;
    let plc = match &config.network {
        NetworkProfile::Devnet(d) => d.plc.clone(),
        NetworkProfile::Production => config.plc.clone(),
    };
    let indexer = Indexer::new(
        engine,
        store,
        IndexerConfig {
            did: args.did,
            source_url: args.source_url,
            plc,
            relays: args.relays,
            jetstream: args.jetstream,
            poll: args.poll,
            poll_interval_ms: args.poll_interval_ms.max(250),
            blocklist: std::sync::Arc::new(blocklist),
        },
    );
    for forum in args.forums {
        indexer.store.add_forum_seed(&forum)?;
    }
    ingest::spawn(indexer.clone());
    live::spawn(indexer.clone());
    let app = api::router(indexer.clone());
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    tracing::info!(address = %listener.local_addr()?, "lessdugin indexer listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}

fn load_blocklist(path: Option<&PathBuf>) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    if let Some(path) = path {
        for line in std::fs::read_to_string(path)
            .with_context(|| format!("read blocklist {}", path.display()))?
            .lines()
        {
            let line = line.trim();
            if !line.is_empty() && !line.starts_with('#') {
                out.insert(line.to_owned());
            }
        }
    }
    Ok(out)
}
