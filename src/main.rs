mod cli;
mod client;
mod commands;
mod config;
mod paths;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands, CacheCommands, CertsCommands, SettingsCommands};
use tracing::Level;
use tracing_subscriber::{fmt, EnvFilter};

fn init_tracing(verbose: u8, quiet: bool) {
    let level = if quiet {
        Level::ERROR
    } else {
        match verbose {
            0 => Level::WARN,
            1 => Level::INFO,
            2 => Level::DEBUG,
            _ => Level::TRACE,
        }
    };

    let filter = EnvFilter::from_default_env()
        .add_directive(format!("rh={level}").parse().unwrap_or_else(|e| {
            eprintln!("Warning: failed to parse log directive: {e}");
            level.into()
        }))
        .add_directive(format!("ranch_hand={level}").parse().unwrap_or_else(|e| {
            eprintln!("Warning: failed to parse log directive: {e}");
            level.into()
        }));

    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(true)
        .init();
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose, cli.quiet);

    match &cli.command {
        Commands::Cache { command } => match command {
            CacheCommands::List => commands::cache::list(&cli).await,
            CacheCommands::Populate { version } => commands::cache::populate(&cli, version).await,
        },
        Commands::Diagnose => commands::diagnose::run(&cli).await,
        Commands::Api {
            endpoint,
            method,
            body,
            input,
            raw,
        } => commands::api::run(&cli, endpoint, method.clone(), body.clone(), input.clone(), *raw).await,
        Commands::Certs { command } => match command {
            CertsCommands::Check => commands::certs::check(&cli).await,
        },
        Commands::Version => commands::version::run(&cli).await,
        Commands::Start => commands::backend::start(&cli).await,
        Commands::Stop => commands::backend::stop(&cli).await,
        Commands::Restart => commands::backend::restart(&cli).await,
        Commands::Status => commands::backend::status(&cli).await,
        Commands::Settings { command } => match command {
            None => commands::settings::show_all(&cli).await,
            Some(SettingsCommands::Get { path }) => commands::settings::get(&cli, path).await,
            Some(SettingsCommands::Set { path, value }) => {
                commands::settings::set(&cli, path, value).await
            }
            Some(SettingsCommands::Reset) => commands::settings::reset(&cli).await,
        },
    }
}
