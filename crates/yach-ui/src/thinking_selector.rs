use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::thinking_level::ThinkingLevel;

pub struct ThinkingLevelSelector<'a> {
    pub levels: &'a [ThinkingLevel],
    pub current_level: ThinkingLevel,
    pub selected_index: usize,
}

impl Widget for ThinkingLevelSelector<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        Clear.render(area, buf);

        let width = 28;
        let height = u16::try_from(self.levels.len()).unwrap_or(7) + 2;
        let popup_area = centered_rect(area, width, height);

        let title = " Thinking Level ";
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .style(Style::default().fg(Color::Cyan));

        let lines: Vec<Line> = self
            .levels
            .iter()
            .enumerate()
            .map(|(i, level)| {
                let is_selected = i == self.selected_index;
                let is_current = *level == self.current_level;
                let indicator = if is_current { "● " } else { "  " };
                let arrow = if is_selected { "▸ " } else { "  " };
                let style = if is_selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else if is_current {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Gray)
                };
                let text = format!("{arrow}{indicator}{}", level.as_str());
                Line::from(Span::styled(text, style))
            })
            .collect();

        let paragraph = Paragraph::new(lines).block(block);
        Widget::render(paragraph, popup_area, buf);
    }
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let popup_width = width.min(area.width);
    let popup_height = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    Rect::new(x, y, popup_width, popup_height)
}
