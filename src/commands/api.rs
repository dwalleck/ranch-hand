// Command stubs will need async once implemented - allow for now
#![allow(clippy::unused_async)]

use crate::cli::{Cli, HttpMethod};
use anyhow::Result;
use std::path::PathBuf;

pub async fn run(
    _cli: &Cli,
    _endpoint: &str,
    _method: HttpMethod,
    _body: Option<String>,
    _input: Option<PathBuf>,
    _raw: bool,
) -> Result<()> {
    todo!("Implement api command")
}
