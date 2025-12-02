// Command stubs will need async once implemented - allow for now
#![allow(clippy::unused_async)]

use crate::cli::Cli;
use anyhow::Result;

pub async fn run(_cli: &Cli) -> Result<()> {
    todo!("Implement diagnose")
}
