use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::perf_metrics::PerfMetrics;

pub struct PerfMetricsOverlay<'a> {
    pub metrics: &'a PerfMetrics,
}

impl Widget for PerfMetricsOverlay<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        Clear.render(area, buf);

        let lines = self.metrics.summary_lines();
        let width = lines
            .iter()
            .map(|l| u16::try_from(l.len()).unwrap_or(30))
            .max()
            .unwrap_or(30)
            + 4;
        let height = u16::try_from(lines.len()).unwrap_or(10) + 2;
        let popup_width = width.min(area.width);
        let popup_height = height.min(area.height);
        let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
        let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Performance ")
            .style(Style::default().fg(Color::Cyan));

        let spans: Vec<Line> = lines
            .iter()
            .map(|l| Line::from(Span::styled(l, Style::default().fg(Color::Gray))))
            .collect();

        let paragraph = Paragraph::new(spans).block(block);
        Widget::render(paragraph, popup_area, buf);
    }
}
