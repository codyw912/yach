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

// OpenAI gpt-5.x: the published figure is the extended window, which the
// API grants only with an explicit opt-in yach does not send, and input
// past 272k doubles session input pricing. Pin the standard window, as
// Codex and omp do. Retire when yach adds a deliberate extended-context
// option. Sources: developers.openai.com/api/docs/models/gpt-5.4;
// omp packages/catalog scripts/generated-policies.ts.
const OPENAI_STANDARD_CONTEXT_WINDOW: u64 = 272_000;

/// Applies the openai gpt-5.x context-window pin (see
/// `OPENAI_STANDARD_CONTEXT_WINDOW` above) and passes everything else
/// through unchanged, including a missing/non-numeric raw value.
fn pinned_context_window(
    provider: &str,
    model_id: &str,
    context: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    let value = context?;
    if provider == "openai"
        && model_id.starts_with("gpt-5")
        && let Some(number) = value.as_u64()
    {
        return Some(serde_json::json!(
            number.min(OPENAI_STANDARD_CONTEXT_WINDOW)
        ));
    }
    Some(value.clone())
}

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
            let context_window = pinned_context_window(name, id, model.pointer("/limit/context"));
            let entry = serde_json::json!({
                "context_window": context_window,
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
