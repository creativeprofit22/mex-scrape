//! Writes the API's OpenAPI spec as pretty JSON to the path in argv[1].

use anyhow::Context as _;

fn main() -> anyhow::Result<()> {
    let out = std::env::args_os()
        .nth(1)
        .context("usage: openapi <output.json>")?;
    let mut json = api::openapi()
        .to_pretty_json()
        .context("failed to serialize OpenAPI spec")?;
    json.push('\n');
    std::fs::write(&out, json)
        .with_context(|| format!("failed to write {}", out.to_string_lossy()))?;
    Ok(())
}
