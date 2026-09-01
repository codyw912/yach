use crate::theme::Theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use yach_proto::ModelInfo;

pub trait ModelRow {
    fn model(&self) -> &ModelInfo;
}

impl ModelRow for ModelInfo {
    fn model(&self) -> &ModelInfo {
        self
    }
}

impl ModelRow for &ModelInfo {
    fn model(&self) -> &ModelInfo {
        self
    }
}

pub struct ModelSelector<'a, M: ModelRow = ModelInfo> {
    pub models: &'a [M],
    pub current_model: &'a str,
    pub current_connection_id: Option<&'a str>,
    pub default_target: Option<(&'a str, Option<&'a str>)>,
    pub selected_index: usize,
    pub save_default_selected: bool,
    pub query: &'a str,
    pub theme: &'a Theme,
}

impl<M: ModelRow> Widget for ModelSelector<'_, M> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let popup_area = centered_rect(70, 60, area);
        Clear.render(popup_area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(self.theme.colors.border))
            .title("Select Model")
            .title_style(
                Style::new()
                    .fg(self.theme.colors.accent)
                    .add_modifier(Modifier::BOLD),
            );

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let lines = if self.models.is_empty() {
            if self.query.is_empty() {
                vec![
                    Line::from(Span::styled(
                        "Available models are loading from the backend...",
                        Style::new().fg(self.theme.colors.warning),
                    )),
                    Line::raw(""),
                    Line::from("Close with Esc and try again shortly."),
                ]
            } else {
                vec![
                    Line::from(Span::styled(
                        format!("No models match \"{}\".", self.query),
                        Style::new().fg(self.theme.colors.warning),
                    )),
                    Line::raw(""),
                    Line::from(Span::styled(
                        "Backspace edits the search; Esc closes.",
                        Style::new().fg(self.theme.colors.dim),
                    )),
                ]
            }
        } else {
            let height = usize::from(inner.height);
            // Header degrades with height: the blank separator and the help
            // line are luxuries; the search line remains while there is room
            // for at least one row beneath it.
            let show_help = height >= 7;
            let show_search = height >= 2;
            let show_blank = height >= 5;
            let mut lines = Vec::new();
            if show_help {
                lines.push(Line::from(Span::styled(
                    "Type to search all discovered models. Arrows move; Enter requests a model change; Esc closes.",
                    Style::new().fg(self.theme.colors.dim),
                )));
            }
            if show_search {
                lines.push(Line::from(vec![
                    Span::styled("Search: ", Style::new().fg(self.theme.colors.dim)),
                    Span::styled(self.query, Style::new().fg(self.theme.colors.text)),
                ]));
            }
            if show_blank {
                lines.push(Line::raw(""));
            }
            // Scroll-indicator slots are reserved before choosing the model
            // window; each reservation shrinks capacity, which can move the
            // window and expose more hidden rows, so iterate to a fixed
            // point. Reservations are add-only and each requires capacity >
            // 1, so this terminates with at least one model row visible and
            // every indicator shown honestly (an indicator always means rows
            // are actually hidden on that side). The selected row is never
            // displaced, because Enter activates it whether or not it
            // renders.
            let row_area = height.saturating_sub(lines.len());
            if row_area > 0 {
                let selected = self.selected_index.min(self.models.len().saturating_sub(1));
                let mut show_up = false;
                let mut show_down = false;
                loop {
                    let capacity = row_area - usize::from(show_up) - usize::from(show_down);
                    let start = selected.saturating_sub(capacity.saturating_sub(1));
                    if !show_up && start > 0 && capacity > 1 {
                        show_up = true;
                    } else if !show_down && start + capacity < self.models.len() && capacity > 1 {
                        show_down = true;
                    } else {
                        break;
                    }
                }
                let capacity = row_area - usize::from(show_up) - usize::from(show_down);
                let start = selected.saturating_sub(capacity.saturating_sub(1));
                if show_up {
                    lines.push(Line::from(Span::styled(
                        "  ↑ more",
                        Style::new().fg(self.theme.colors.dim),
                    )));
                }
                lines.extend(
                    self.models
                        .iter()
                        .enumerate()
                        .skip(start)
                        .take(capacity)
                        .map(|(i, model)| {
                            let model = M::model(model);
                            let is_selected = i == selected;
                            let is_current = match (
                                model.connection_id.as_deref(),
                                self.current_connection_id,
                            ) {
                                (Some(row_connection_id), Some(current_connection_id)) => {
                                    row_connection_id == current_connection_id
                                        && model.id == self.current_model
                                }
                                (None, None) => {
                                    model.label() == self.current_model
                                        || model.id == self.current_model
                                        || model.name == self.current_model
                                }
                                _ => false,
                            };
                            let is_default = self.default_target.is_some_and(
                                |(default_model, default_connection)| {
                                    default_model == model.id
                                        && default_connection == model.connection_id.as_deref()
                                },
                            );
                            let prefix = if is_selected { "▸ " } else { "  " };
                            let suffix = match (is_current, is_default) {
                                (true, true) => " ● active ◆ default",
                                (true, false) => " ● active",
                                (false, true) => " ◆ default",
                                (false, false) => "",
                            };
                            let style = if is_selected {
                                Style::new()
                                    .fg(self.theme.colors.selected_text)
                                    .add_modifier(Modifier::BOLD)
                            } else if is_current || is_default {
                                Style::new().fg(self.theme.colors.warning)
                            } else {
                                Style::new().fg(self.theme.colors.muted)
                            };
                            let row_label = model.connection_display.as_deref().map_or_else(
                                || model.label(),
                                |connection_display| {
                                    format!("{} [{connection_display}]", model.label())
                                },
                            );
                            Line::from(vec![
                                Span::styled(prefix, style),
                                Span::styled(
                                    format!("{row_label} — {}{suffix}", model.name),
                                    style,
                                ),
                            ])
                        }),
                );
                if show_down {
                    lines.push(Line::from(Span::styled(
                        "  ↓ more",
                        Style::new().fg(self.theme.colors.dim),
                    )));
                }
            }
            let action_style = if self.save_default_selected {
                Style::new()
                    .fg(self.theme.colors.selected_text)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(self.theme.colors.accent)
            };
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                if self.save_default_selected {
                    "▸ Activate and save as default"
                } else {
                    "  Activate and save as default"
                },
                action_style,
            )));
            lines
        };

        let paragraph = Paragraph::new(lines).style(Style::new().fg(self.theme.colors.text));
        Widget::render(paragraph, inner, buf);
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

#[cfg(test)]
mod tests {
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};
    use yach_proto::ModelInfo;

    use super::ModelSelector;
    use crate::theme::Theme;

    fn duplicate_rows() -> [ModelInfo; 2] {
        [
            ModelInfo {
                id: String::from("gpt-5"),
                name: String::from("GPT-5"),
                provider: String::from("openai-compatible"),
                connection_id: Some(String::from("connection-a")),
                connection_display: Some(String::from("A")),
            },
            ModelInfo {
                id: String::from("gpt-5"),
                name: String::from("GPT-5"),
                provider: String::from("openai-compatible"),
                connection_id: Some(String::from("connection-b")),
                connection_display: Some(String::from("B")),
            },
        ]
    }

    #[test]
    fn model_selector_shows_search_help_and_current_query() {
        let models = duplicate_rows();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 24));

        ModelSelector {
            models: &models,
            current_model: "gpt-5",
            current_connection_id: Some("connection-b"),
            default_target: None,
            selected_index: 0,
            save_default_selected: false,
            query: "gpt",
            theme: &Theme::default(),
        }
        .render(Rect::new(0, 0, 100, 24), &mut buffer);

        let rendered = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("search all discovered models"));
        assert!(rendered.contains("Search: gpt"));
    }

    #[test]
    fn model_selector_shows_no_match_message_when_query_filters_everything() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 24));

        ModelSelector::<ModelInfo> {
            models: &[],
            current_model: "gpt-5",
            current_connection_id: None,
            default_target: None,
            selected_index: 0,
            save_default_selected: false,
            query: "zzz",
            theme: &Theme::default(),
        }
        .render(Rect::new(0, 0, 100, 24), &mut buffer);

        let rendered = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("No models match \"zzz\""));
        assert!(!rendered.contains("loading from the backend"));
    }

    fn many_rows(count: usize) -> Vec<ModelInfo> {
        (0..count)
            .map(|i| ModelInfo {
                id: format!("gpt-{i}"),
                name: format!("GPT-{i}"),
                provider: String::from("openai"),
                connection_id: None,
                connection_display: None,
            })
            .collect()
    }

    #[test]
    fn model_selector_renders_selected_row_with_query_at_nine_line_height() {
        let models = many_rows(8);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 9));

        ModelSelector {
            models: &models,
            current_model: "gpt-0",
            current_connection_id: None,
            default_target: None,
            selected_index: 7,
            save_default_selected: false,
            query: "gpt",
            theme: &Theme::default(),
        }
        .render(Rect::new(0, 0, 100, 9), &mut buffer);

        let rendered = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("openai/gpt-7 — GPT-7"));
    }

    #[test]
    fn model_selector_renders_selected_row_not_only_scroll_hint_at_four_line_inner_height() {
        let models = many_rows(5);
        // Total height 10 yields a 4-line inner area inside the popup border.
        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 10));

        ModelSelector {
            models: &models,
            current_model: "gpt-0",
            current_connection_id: None,
            default_target: None,
            selected_index: 3,
            save_default_selected: false,
            query: "gpt",
            theme: &Theme::default(),
        }
        .render(Rect::new(0, 0, 100, 10), &mut buffer);

        let rendered = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("  ↑ more"));
        assert!(rendered.contains("openai/gpt-3 — GPT-3"));
    }

    #[test]
    fn model_selector_shows_both_scroll_hints_around_selected_row_at_four_line_inner_height() {
        let models = many_rows(5);
        // Total height 10 yields a 4-line inner area inside the popup border.
        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 10));

        ModelSelector {
            models: &models,
            current_model: "gpt-0",
            current_connection_id: None,
            default_target: None,
            selected_index: 3,
            save_default_selected: false,
            query: "gpt",
            theme: &Theme::default(),
        }
        .render(Rect::new(0, 0, 100, 10), &mut buffer);

        let rendered = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("  ↑ more"));
        assert!(rendered.contains("openai/gpt-3 — GPT-3"));
        assert!(rendered.contains("  ↓ more"));
    }

    #[test]
    fn model_selector_scroll_hints_stay_honest_from_start_to_end() {
        let models = many_rows(5);
        for (selected, expect_up, expect_down, row) in [
            (0, false, true, "openai/gpt-0 — GPT-0"),
            (2, true, true, "openai/gpt-2 — GPT-2"),
            (4, true, false, "openai/gpt-4 — GPT-4"),
        ] {
            let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 10));

            ModelSelector {
                models: &models,
                current_model: "gpt-0",
                current_connection_id: None,
                default_target: None,
                selected_index: selected,
                save_default_selected: false,
                query: "gpt",
                theme: &Theme::default(),
            }
            .render(Rect::new(0, 0, 100, 10), &mut buffer);

            let rendered = buffer
                .content()
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>();
            assert_eq!(
                rendered.contains("  ↑ more"),
                expect_up,
                "up hint honesty failed for selected={selected}: {rendered}"
            );
            assert_eq!(
                rendered.contains("  ↓ more"),
                expect_down,
                "down hint honesty failed for selected={selected}: {rendered}"
            );
            assert!(
                rendered.contains(row),
                "selected row missing for selected={selected}: {rendered}"
            );
        }
    }

    #[test]
    fn model_selector_keeps_selected_row_visible_at_tiny_capacity() {
        let models = many_rows(5);
        // Total height 9 yields a 3-line inner area: search line plus two
        // row slots.
        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 9));

        ModelSelector {
            models: &models,
            current_model: "gpt-0",
            current_connection_id: None,
            default_target: None,
            selected_index: 3,
            save_default_selected: false,
            query: "gpt",
            theme: &Theme::default(),
        }
        .render(Rect::new(0, 0, 100, 9), &mut buffer);

        let rendered = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("  ↑ more"));
        assert!(rendered.contains("openai/gpt-3 — GPT-3"));
    }

    #[test]
    fn model_selector_marks_only_exact_connection_current() {
        let models = duplicate_rows();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 24));

        ModelSelector {
            models: &models,
            current_model: "gpt-5",
            current_connection_id: Some("connection-b"),
            default_target: None,
            selected_index: 0,
            save_default_selected: false,
            query: "",
            theme: &Theme::default(),
        }
        .render(Rect::new(0, 0, 100, 24), &mut buffer);

        let rendered = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert_eq!(rendered.matches("● active").count(), 1);
        assert!(rendered.contains("openai-compatible/gpt-5 [B] — GPT-5 ● active"));
        assert!(!rendered.contains("openai-compatible/gpt-5 [A] — GPT-5 ● active"));
    }

    #[test]
    fn model_selector_marks_active_default_and_action_independently() {
        let models = duplicate_rows();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 24));
        ModelSelector {
            models: &models,
            current_model: "gpt-5",
            current_connection_id: Some("connection-b"),
            default_target: Some(("gpt-5", Some("connection-a"))),
            selected_index: 1,
            save_default_selected: true,
            query: "",
            theme: &Theme::default(),
        }
        .render(Rect::new(0, 0, 100, 24), &mut buffer);
        let rendered = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert_eq!(rendered.matches("● active").count(), 1);
        assert_eq!(rendered.matches("◆ default").count(), 1);
        assert!(rendered.contains("▸ Activate and save as default"));
    }

    #[test]
    fn model_selector_keeps_legacy_current_fallback_when_both_ids_are_none() {
        let models = [ModelInfo {
            id: String::from("legacy-model"),
            name: String::from("Legacy Model"),
            provider: String::from("legacy"),
            connection_id: None,
            connection_display: None,
        }];
        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 24));

        ModelSelector {
            models: &models,
            current_model: "legacy-model",
            current_connection_id: None,
            default_target: None,
            selected_index: 0,
            save_default_selected: false,
            query: "",
            theme: &Theme::default(),
        }
        .render(Rect::new(0, 0, 100, 24), &mut buffer);

        let rendered = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert_eq!(rendered.matches("● active").count(), 1);
    }
}
