use crate::cli::Cli;
use anyhow::Result;

pub async fn show_all(_cli: &Cli) -> Result<()> {
    todo!("Implement settings show all")
}

pub async fn get(_cli: &Cli, _path: &str) -> Result<()> {
    todo!("Implement settings get")
}

pub async fn set(_cli: &Cli, _path: &str, _value: &str) -> Result<()> {
    todo!("Implement settings set")
}

pub async fn reset(_cli: &Cli) -> Result<()> {
    todo!("Implement settings reset")
}
