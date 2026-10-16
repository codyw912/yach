use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::slash_commands::SlashCommand;
use crate::theme::Theme;

pub struct SlashPopup<'a> {
    pub selected: usize,
    pub matches: Vec<SlashCommand>,
    pub theme: &'a Theme,
}

impl Widget for SlashPopup<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        if self.matches.is_empty() {
            return;
        }

        let height = u16::try_from(self.matches.len() + 2).unwrap_or(10).min(10);
        let width = 40;
        let popup_area = bottom_left_rect(width, height, area);
        if popup_area.is_empty() {
            return;
        }

        Clear.render(popup_area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.border(false))
            .title("Commands · Tab accepts")
            .title_style(self.theme.accent().add_modifier(Modifier::BOLD));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let lines: Vec<Line<'_>> = self
            .matches
            .iter()
            .enumerate()
            .map(|(i, cmd)| {
                let is_selected = i == self.selected;
                let style = if is_selected {
                    self.theme.selected_label()
                } else {
                    self.theme.muted()
                };
                let prefix = if is_selected { "▸ " } else { "  " };
                Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(cmd.name, self.theme.accent()),
                    Span::styled(" - ", self.theme.dim()),
                    Span::styled(cmd.description, style),
                ])
            })
            .collect();

        Widget::render(Paragraph::new(lines).style(self.theme.text()), inner, buf);
    }
}

fn bottom_left_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(height),
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    use super::SlashPopup;
    use crate::slash_commands::SLASH_COMMANDS;
    use crate::theme::Theme;

    fn render_matches(area: Rect) -> Buffer {
        let mut buffer = Buffer::empty(area);
        Widget::render(
            SlashPopup {
                selected: 0,
                matches: SLASH_COMMANDS
                    .iter()
                    .filter(|cmd| cmd.name.starts_with('/'))
                    .copied()
                    .collect(),
                theme: &Theme::default(),
            },
            area,
            &mut buffer,
        );
        buffer
    }

    #[test]
    fn slash_popup_renders_several_matches_inside_a_one_row_area() {
        let _buffer = render_matches(Rect::new(0, 0, 140, 1));
    }

    #[test]
    fn slash_popup_renders_several_matches_inside_a_two_row_area() {
        let _buffer = render_matches(Rect::new(0, 0, 140, 2));
    }

    #[test]
    fn slash_popup_shows_matches_when_the_area_fits() {
        let buffer = render_matches(Rect::new(2, 3, 80, 12));
        let rendered = buffer
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("/model"));
        assert!(rendered.contains("Commands"));
    }
}
