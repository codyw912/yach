//! Generate the baked catalog from a downloaded models.dev api.json.
//! Usage: snapshot <api.json path> <output path> <snapshot-date>
//! Providers are allowlisted to what yach can drive; the schema is the
//! catalog's own, not models.dev's, so upstream drift breaks THIS tool
//! loudly instead of the runtime quietly.

use std::collections::BTreeMap;

const PROVIDER_ALLOWLIST: &[&str] = &[
    "anthropic",
    "openai",
    "alibaba",
    "deepseek",
    "nvidia",
    "fireworks-ai",
];

// Build-time CLI tool, not part of the runtime: progress output to
// stdout is the intended UX, not a stray print left over from debugging.
#[expect(clippy::print_stdout)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output), Some(date)) = (args.next(), args.next(), args.next()) else {
        return Err("usage: snapshot <api.json> <output> <snapshot-date>".into());
    };
    let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&input)?)?;
    let mut providers = BTreeMap::new();
    for name in PROVIDER_ALLOWLIST {
        let Some(models_in) = raw
            .get(name)
            .and_then(|p| p.get("models"))
            .and_then(|m| m.as_object())
        else {
            continue;
        };
        let mut models = BTreeMap::new();
        for (id, model) in models_in {
            let entry = serde_json::json!({
                "context_window": model.pointer("/limit/context"),
                "output_ceiling": model.pointer("/limit/output"),
                "display_name": model.get("name"),
                "cost": model.get("cost").map(|cost| serde_json::json!({
                    "input": cost.get("input"),
                    "output": cost.get("output"),
                    "cache_read": cost.get("cache_read"),
                    "cache_write": cost.get("cache_write"),
                })),
            });
            models.insert(id.clone(), entry);
        }
        providers.insert((*name).to_owned(), serde_json::json!({ "models": models }));
    }
    let catalog = serde_json::json!({
        "snapshot_date": date,
        "source": "models.dev api.json",
        "providers": providers,
    });
    std::fs::write(&output, serde_json::to_string_pretty(&catalog)?)?;
    println!(
        "wrote {output}: {} providers",
        catalog["providers"]
            .as_object()
            .map_or(0, serde_json::Map::len)
    );
    Ok(())
}
