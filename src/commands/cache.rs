// Command stubs will need async once implemented - allow for now
#![allow(clippy::unused_async)]

use crate::cli::Cli;
use anyhow::Result;

pub async fn list(_cli: &Cli) -> Result<()> {
    todo!("Implement cache list")
}

pub async fn populate(_cli: &Cli, _version: &str) -> Result<()> {
    todo!("Implement cache populate")
}
