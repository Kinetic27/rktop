mod app;
#[allow(dead_code)]
mod collectors;
#[allow(dead_code)]
mod config;
#[allow(dead_code)]
mod fixtures;
#[allow(dead_code)]
mod model;
mod render;
mod theme;

use anyhow::Result;

fn main() -> Result<()> {
    let cli = app::Cli::parse(std::env::args().skip(1))?;
    app::run(cli)
}
