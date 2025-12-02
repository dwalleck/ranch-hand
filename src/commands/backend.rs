// Command stubs will need async once implemented - allow for now
#![allow(clippy::unused_async)]

use crate::cli::Cli;
use anyhow::Result;

pub async fn start(_cli: &Cli) -> Result<()> {
    todo!("Implement start")
}

pub async fn stop(_cli: &Cli) -> Result<()> {
    todo!("Implement stop")
}

pub async fn restart(_cli: &Cli) -> Result<()> {
    todo!("Implement restart")
}

pub async fn status(_cli: &Cli) -> Result<()> {
    todo!("Implement status")
}
