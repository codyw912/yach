use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::slash_commands::SLASH_COMMANDS;

pub struct HelpOverlay;

impl Widget for HelpOverlay {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let popup_area = centered_rect(74, 68, area);
        Clear.render(popup_area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .title("Yach help")
            .title_style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD));
        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let mut lines = vec![Line::from(Span::styled(
            "Commands",
            Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
        ))];

        lines.extend(SLASH_COMMANDS.iter().map(|command| {
            Line::from(vec![
                Span::styled(
                    format!("  {:<10}", command.name),
                    Style::new().fg(Color::Yellow),
                ),
                Span::raw(command.description),
            ])
        }));

        lines.extend([
            Line::raw(""),
            Line::from(Span::styled(
                "Keys",
                Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
            )),
            Line::from("  Enter       Submit prompt"),
            Line::from("  Ctrl+J      Insert newline"),
            Line::from("  /           Show slash-command completion"),
            Line::from("  Tab         Accept selected slash completion"),
            Line::from("  Alt+M       Model selector"),
            Line::from("  Ctrl+S      Session selector"),
            Line::from("  Ctrl+B      Load branch summary"),
            Line::from("  Ctrl+T      Thinking selector"),
            Line::from("  Ctrl+F      Clone current branch after at least one message"),
            Line::from("  Ctrl+P      Performance overlay"),
            Line::from("  j/k or ↑/↓ Move in selectors"),
            Line::from("  PageUp/Down Scroll transcript"),
            Line::from("  Mouse wheel Scroll transcript"),
            Line::from("  End         Jump transcript to bottom"),
            Line::from("  Ctrl+C      Stop following active stream; quit when idle"),
            Line::raw(""),
            Line::from(Span::styled(
                "Esc, Enter, q, h, or ? closes this overlay.",
                Style::new().fg(Color::DarkGray),
            )),
        ]);

        Widget::render(Paragraph::new(lines), inner, buf);
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
