//! Diagnostic provider smokes kept separate from the runtime Rig Adapter Interface.

pub use crate::rig_adapter::{
    OpenAiCompatibleHttpSmokeReport, RigAnthropicSmokeConfig, RigChatGptSubscriptionSmokeConfig,
    RigOpenAiCompatibleSmokeConfig, RigOpenAiCompatibleSmokeReport, RigOpenAiSmokeConfig,
    run_anthropic_smoke, run_chatgpt_subscription_smoke, run_openai_compatible_http_smoke,
    run_openai_compatible_smoke, run_openai_smoke,
};
