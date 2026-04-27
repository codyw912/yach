use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers};
use yach_proto::BackendEvent;
use yach_ui::{BenchmarkApp, Transcript};

use crate::latency::LatencySummary;

#[derive(Debug, Clone)]
pub enum ReplayStep {
    Backend(BackendEvent),
    Key {
        code: KeyCode,
        modifiers: KeyModifiers,
    },
    PromptText(String),
    Transcript(Transcript),
    ScrollDown(usize),
}

#[derive(Debug, Clone)]
pub struct ReplayResult {
    pub samples: Vec<Duration>,
    pub prompt_text: String,
}

impl ReplayResult {
    #[must_use]
    pub fn summary(&self, label: impl Into<Option<String>>) -> LatencySummary {
        LatencySummary::from_samples(label, &self.samples)
    }
}

#[must_use]
pub fn replay_headless(steps: &[ReplayStep], width: u16, height: u16) -> ReplayResult {
    let mut app = BenchmarkApp::new();
    let mut samples = Vec::with_capacity(steps.len());

    for step in steps {
        let start = Instant::now();
        apply_step(&mut app, step);
        app.render_headless(width, height);
        samples.push(start.elapsed());
    }

    ReplayResult {
        samples,
        prompt_text: app.prompt_text(),
    }
}

fn apply_step(app: &mut BenchmarkApp, step: &ReplayStep) {
    match step {
        ReplayStep::Backend(event) => app.handle_backend_event(event.clone()),
        ReplayStep::Key { code, modifiers } => app.handle_key(*code, *modifiers),
        ReplayStep::PromptText(text) => app.set_prompt_text(text),
        ReplayStep::Transcript(transcript) => app.set_transcript(transcript.clone()),
        ReplayStep::ScrollDown(lines) => app.scroll_down(*lines),
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};

    use crate::fixtures::{
        TranscriptScale, connected_event, prompt_delta_events, transcript_fixture,
    };

    use super::{ReplayStep, replay_headless};

    #[test]
    fn keypress_updates_prompt_and_records_latency() {
        let result = replay_headless(
            &[
                ReplayStep::Backend(connected_event()),
                ReplayStep::Key {
                    code: KeyCode::Char('a'),
                    modifiers: KeyModifiers::empty(),
                },
            ],
            80,
            24,
        );

        assert_eq!(result.prompt_text, "a");
        assert_eq!(result.samples.len(), 2);
        assert!(result.summary(None).has_data());
    }

    #[test]
    fn backend_prompt_delta_renders_without_terminal() {
        let mut steps = vec![ReplayStep::Backend(connected_event())];
        steps.extend(prompt_delta_events(3).into_iter().map(ReplayStep::Backend));

        let result = replay_headless(&steps, 80, 24);

        assert_eq!(result.samples.len(), 4);
    }

    #[test]
    fn empty_render_records_no_panic_sample() {
        let result = replay_headless(&[ReplayStep::PromptText(String::new())], 80, 24);

        assert_eq!(result.samples.len(), 1);
        assert_eq!(result.prompt_text, "");
    }

    #[test]
    fn large_transcript_renders_headlessly() {
        let result = replay_headless(
            &[
                ReplayStep::Backend(connected_event()),
                ReplayStep::Transcript(transcript_fixture(TranscriptScale::Medium)),
                ReplayStep::ScrollDown(10),
            ],
            100,
            32,
        );

        assert_eq!(result.samples.len(), 3);
    }
}
