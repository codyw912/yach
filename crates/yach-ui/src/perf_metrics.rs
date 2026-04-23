use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct PerfMetrics {
    pub render_times: Vec<Duration>,
    pub total_renders: usize,
    session_start: Instant,
}

impl PerfMetrics {
    pub fn new() -> Self {
        Self {
            render_times: Vec::new(),
            total_renders: 0,
            session_start: Instant::now(),
        }
    }

    pub fn record_render(&mut self, duration: Duration) {
        self.render_times.push(duration);
        self.total_renders += 1;
        if self.render_times.len() > 100 {
            self.render_times.drain(..50);
        }
    }

    pub fn avg_render_time(&self) -> Option<Duration> {
        if self.render_times.is_empty() {
            return None;
        }
        let sum: Duration = self.render_times.iter().sum();
        Some(sum / self.render_times.len().try_into().unwrap_or(u32::MAX))
    }

    pub fn session_duration(&self) -> Duration {
        self.session_start.elapsed()
    }

    pub fn summary_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        lines.push(String::from("── Performance Metrics ──"));
        lines.push(format!(
            "Session: {:.1}s",
            self.session_duration().as_secs_f64()
        ));
        lines.push(format!("Total renders: {}", self.total_renders));
        if let Some(avg) = self.avg_render_time() {
            lines.push(format!("Avg render: {:.1}μs", avg.as_micros()));
        }
        lines
    }
}
