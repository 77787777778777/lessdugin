// SPDX-License-Identifier: AGPL-3.0-or-later
//! `lessdugin` — command line entry point.

use clap::Parser;

/// A serverless markdown and media forum on atproto.
#[derive(Debug, Parser)]
#[command(name = "lessdugin", version, about, long_about = None)]
struct Cli {}

fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    println!(
        "lessdugin {} — under construction",
        env!("CARGO_PKG_VERSION")
    );
    Ok(())
}
