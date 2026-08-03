//! Generate the baked catalog from a downloaded models.dev api.json.
//! Usage: snapshot <api.json path> <output path> <snapshot-date>
//! The allowlist+policy transform lives in the library
//! (`yach_catalog::transform_models_dev`), shared with the runtime fetch
//! layer — this bin is just argument handling and file I/O around it.

// Build-time CLI tool, not part of the runtime: progress output to
// stdout is the intended UX, not a stray print left over from debugging.
#[expect(clippy::print_stdout)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output), Some(date)) = (args.next(), args.next(), args.next()) else {
        return Err("usage: snapshot <api.json> <output> <snapshot-date>".into());
    };
    let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&input)?)?;
    let catalog = yach_catalog::transform_models_dev(&raw, &date);
    std::fs::write(&output, serde_json::to_string_pretty(&catalog)?)?;
    println!(
        "wrote {output}: {} providers",
        catalog["providers"]
            .as_object()
            .map_or(0, serde_json::Map::len)
    );
    Ok(())
}
