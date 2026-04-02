//! RepromptOverlay — a Ratatui widget that renders the refinement preview.
//!
//! This overlay is shown as a modal in the bottom pane after `/reprompt`
//! refines the user's input. It displays the original and refined prompts,
//! applied rules, and key bindings for accept/edit/skip/cancel.
//!
//! The content area scrolls when it exceeds the available height. The footer
//! is always pinned to the bottom.
//!
//! Key handling:
//!   Enter  = accept refined prompt
//!   e      = edit refined prompt (placeholder — transitions state)
//!   s      = skip refinement, send original
//!   r      = show reasoning
//!   Esc    = cancel submission entirely
//!   Up/k   = scroll up
//!   Down/j = scroll down

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Direction;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

use super::RepromptOverlayAction;
use super::RepromptOverlayData;
use super::RepromptOverlayState;

/// Maximum height the overlay will claim (in terminal rows).
const MAX_OVERLAY_HEIGHT: u16 = 80;

/// Modal overlay that presents the `/reprompt` refinement result to the user.
///
/// The overlay handles its own key events. When the user makes a decision,
/// the overlay marks itself as complete and the parent (`ChatWidget`) inspects
/// the final [`RepromptOverlayAction`] to decide what to do.
pub(crate) struct RepromptOverlay {
    data: RepromptOverlayData,
    action: Option<RepromptOverlayAction>,
    show_reasoning: bool,
    done: bool,
    /// Vertical scroll offset for the content area (lines scrolled past top).
    scroll_offset: u16,
}

#[allow(dead_code)]
impl RepromptOverlay {
    pub(crate) fn new(data: RepromptOverlayData) -> Self {
        Self {
            data,
            action: None,
            show_reasoning: false,
            done: false,
            scroll_offset: 0,
        }
    }

    /// Return the action the user chose, if the overlay completed.
    pub(crate) fn take_action(&mut self) -> Option<RepromptOverlayAction> {
        self.action.take()
    }

    /// Handle a key event and return the resulting action.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> RepromptOverlayAction {
        if key.kind != KeyEventKind::Press {
            return RepromptOverlayAction::None;
        }

        if !matches!(self.data.state, RepromptOverlayState::Reviewing) {
            return RepromptOverlayAction::None;
        }

        match key.code {
            KeyCode::Enter => {
                let text = self.data.result.refined_prompt.clone();
                self.data.state = RepromptOverlayState::Accepted(text.clone());
                let action = RepromptOverlayAction::Accept(text);
                self.action = Some(action.clone());
                self.done = true;
                action
            }
            KeyCode::Char('e') => {
                let text = self.data.result.refined_prompt.clone();
                self.data.state = RepromptOverlayState::Editing(text);
                RepromptOverlayAction::None
            }
            KeyCode::Char('s') => {
                self.data.state = RepromptOverlayState::Skipped;
                let action = RepromptOverlayAction::Skip;
                self.action = Some(action.clone());
                self.done = true;
                action
            }
            KeyCode::Char('r') => {
                self.show_reasoning = !self.show_reasoning;
                // Reset scroll when toggling reasoning visibility.
                self.scroll_offset = 0;
                RepromptOverlayAction::ShowReasoning
            }
            KeyCode::Esc => {
                self.data.state = RepromptOverlayState::Cancelled;
                let action = RepromptOverlayAction::Cancel;
                self.action = Some(action.clone());
                self.done = true;
                action
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                RepromptOverlayAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
                RepromptOverlayAction::None
            }
            _ => RepromptOverlayAction::None,
        }
    }

    /// Check for auto-accept timer expiry.
    pub(crate) fn tick(&mut self) -> Option<RepromptOverlayAction> {
        if let Some(action) = self.data.tick() {
            self.data.state =
                RepromptOverlayState::Accepted(self.data.result.refined_prompt.clone());
            self.action = Some(action.clone());
            self.done = true;
            Some(action)
        } else {
            None
        }
    }

    /// Build the content lines (everything above the footer).
    fn build_content_lines(&self) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        // Original prompt
        lines.push(Line::from(vec![Span::styled(
            "  Original:",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )]));
        for l in self.data.original.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {l}"),
                Style::default().fg(Color::DarkGray),
            )));
        }
        lines.push(Line::from(""));

        // Refined prompt
        lines.push(Line::from(vec![Span::styled(
            "  Refined:",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )]));
        for l in self.data.result.refined_prompt.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {l}"),
                Style::default().fg(Color::Green),
            )));
        }
        lines.push(Line::from(""));

        // Applied rules
        if !self.data.result.applied_rules.is_empty() {
            let rules_text = self.data.result.applied_rules.join(", ");
            lines.push(Line::from(vec![
                Span::styled("  Rules: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(rules_text, Style::default().fg(Color::Cyan)),
            ]));
        }

        // Reasoning (toggled by 'r')
        if self.show_reasoning {
            lines.push(Line::from(vec![Span::styled(
                "  Reasoning:",
                Style::default().add_modifier(Modifier::BOLD),
            )]));
            for l in self.data.result.reasoning.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  {l}"),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }

        lines
    }

    /// Build the footer lines (separator + key hints) — always pinned to bottom.
    fn build_footer_lines(&self, width: u16) -> Vec<Line<'static>> {
        let inner_width = width.saturating_sub(4) as usize;
        let mut lines: Vec<Line<'static>> = Vec::new();

        let sep: String = "\u{2500}".repeat(inner_width);
        lines.push(Line::from(Span::styled(
            format!("  {sep}"),
            Style::default().fg(Color::DarkGray),
        )));

        let mut footer_spans = vec![
            Span::styled(
                "  enter",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" accept  "),
            Span::styled(
                "s",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" skip (use original)  "),
            Span::styled(
                "r",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(if self.show_reasoning {
                " hide reasoning  "
            } else {
                " reasoning  "
            }),
            Span::styled(
                "esc",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" cancel"),
        ];

        if let Some(remaining) = self.data.auto_accept_remaining()
            && remaining > 0
        {
            footer_spans.push(Span::styled(
                format!("  ({remaining}s)"),
                Style::default().fg(Color::DarkGray),
            ));
        }

        lines.push(Line::from(footer_spans));

        lines
    }

    /// Whether the overlay has completed (user made a decision).
    pub(crate) fn is_complete(&self) -> bool {
        self.done
    }

    /// Handle Ctrl-C as a cancel action.
    pub(crate) fn on_ctrl_c(&mut self) {
        if self.done {
            return;
        }
        self.data.state = RepromptOverlayState::Cancelled;
        self.action = Some(RepromptOverlayAction::Cancel);
        self.done = true;
    }

    /// Desired height for rendering, capped at [`MAX_OVERLAY_HEIGHT`].
    pub(crate) fn desired_height(&self, width: u16) -> u16 {
        let content_lines = self.build_content_lines().len() as u16;
        let footer_lines = self.build_footer_lines(width).len() as u16;
        // +2 for border top/bottom
        (content_lines + footer_lines + 2).min(MAX_OVERLAY_HEIGHT)
    }

    /// Render the overlay into a buffer area.
    ///
    /// The footer (separator + key hints) is always pinned to the bottom of
    /// the overlay so it stays visible even when content is long. The content
    /// area scrolls via `Paragraph::scroll()` when it overflows.
    pub(crate) fn render_widget(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(Span::styled(
                " REPROMPT ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let footer_lines = self.build_footer_lines(area.width);
        let footer_height = footer_lines.len() as u16;

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(footer_height)])
            .split(inner);

        let content_area = chunks[0];
        let footer_area = chunks[1];

        let content_lines = self.build_content_lines();
        let total_content = content_lines.len() as u16;

        // Clamp scroll offset: can't scroll past the content.
        let max_scroll = total_content.saturating_sub(content_area.height);
        let clamped_scroll = self.scroll_offset.min(max_scroll);

        let content_paragraph = Paragraph::new(content_lines)
            .wrap(Wrap { trim: false })
            .scroll((clamped_scroll, 0));

        // Render a scroll indicator when content overflows.
        if total_content > content_area.height {
            let indicator = if clamped_scroll < max_scroll {
                "\u{25bc}" // ▼
            } else {
                "\u{25b2}" // ▲
            };
            let indicator_span = Span::styled(
                format!(" {indicator} "),
                Style::default().fg(Color::DarkGray),
            );
            let indicator_line =
                Line::from(indicator_span).alignment(ratatui::layout::Alignment::Right);
            // Place the scroll indicator in the top-right corner of content area.
            let indicator_area = Rect::new(
                content_area.right().saturating_sub(4),
                content_area.y,
                4.min(content_area.width),
                1,
            );
            Paragraph::new(vec![indicator_line]).render(indicator_area, buf);
        }

        content_paragraph.render(content_area, buf);

        let footer_paragraph = Paragraph::new(footer_lines);
        footer_paragraph.render(footer_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reprompt::config::RepromptResult;
    use crate::reprompt::config::TaskType;
    use std::time::Duration;

    fn sample_result() -> RepromptResult {
        RepromptResult {
            refined_prompt: "Apply the JWT fix to payments".to_string(),
            applied_rules: vec!["regression test required".to_string()],
            reasoning: "Expanded vague reference".to_string(),
            task_type: TaskType::Bugfix,
            was_substantive_change: true,
        }
    }

    fn make_overlay() -> RepromptOverlay {
        let data = RepromptOverlayData::new(
            "fix it in payments too".to_string(),
            sample_result(),
            Duration::from_secs(5),
        );
        RepromptOverlay::new(data)
    }

    #[test]
    fn enter_accepts_refined_prompt() {
        let mut overlay = make_overlay();
        let action = overlay.handle_key(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(
            matches!(action, RepromptOverlayAction::Accept(ref t) if t == "Apply the JWT fix to payments")
        );
        assert!(overlay.is_complete());
    }

    #[test]
    fn s_skips_refinement() {
        let mut overlay = make_overlay();
        let action = overlay.handle_key(KeyEvent::new(
            KeyCode::Char('s'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(action, RepromptOverlayAction::Skip);
        assert!(overlay.is_complete());
    }

    #[test]
    fn esc_cancels() {
        let mut overlay = make_overlay();
        let action = overlay.handle_key(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(action, RepromptOverlayAction::Cancel);
        assert!(overlay.is_complete());
    }

    #[test]
    fn r_toggles_reasoning() {
        let mut overlay = make_overlay();
        assert!(!overlay.show_reasoning);
        let action = overlay.handle_key(KeyEvent::new(
            KeyCode::Char('r'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(action, RepromptOverlayAction::ShowReasoning);
        assert!(overlay.show_reasoning);
        assert!(!overlay.is_complete());

        overlay.handle_key(KeyEvent::new(
            KeyCode::Char('r'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(!overlay.show_reasoning);
    }

    #[test]
    fn r_resets_scroll_offset() {
        let mut overlay = make_overlay();
        overlay.scroll_offset = 5;
        overlay.handle_key(KeyEvent::new(
            KeyCode::Char('r'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(overlay.scroll_offset, 0);
    }

    #[test]
    fn e_transitions_to_editing_state() {
        let mut overlay = make_overlay();
        let action = overlay.handle_key(KeyEvent::new(
            KeyCode::Char('e'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(action, RepromptOverlayAction::None);
        assert!(!overlay.is_complete());
        assert!(matches!(
            overlay.data.state,
            RepromptOverlayState::Editing(_)
        ));
    }

    #[test]
    fn keys_ignored_when_not_reviewing() {
        let mut overlay = make_overlay();
        overlay.handle_key(KeyEvent::new(
            KeyCode::Char('e'),
            crossterm::event::KeyModifiers::NONE,
        ));
        let action = overlay.handle_key(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(action, RepromptOverlayAction::None);
        assert!(!overlay.is_complete());
    }

    #[test]
    fn ctrl_c_cancels() {
        let mut overlay = make_overlay();
        overlay.on_ctrl_c();
        assert!(overlay.is_complete());
        assert!(matches!(
            overlay.action,
            Some(RepromptOverlayAction::Cancel)
        ));
    }

    #[test]
    fn unknown_key_is_noop() {
        let mut overlay = make_overlay();
        let action = overlay.handle_key(KeyEvent::new(
            KeyCode::Char('z'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(action, RepromptOverlayAction::None);
        assert!(!overlay.is_complete());
    }

    #[test]
    fn release_events_are_ignored() {
        let mut overlay = make_overlay();
        let action = overlay.handle_key(KeyEvent::new_with_kind(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
            KeyEventKind::Release,
        ));
        assert_eq!(action, RepromptOverlayAction::None);
        assert!(!overlay.is_complete());
    }

    #[test]
    fn auto_accept_timer_fires() {
        let data = RepromptOverlayData::new(
            "original".to_string(),
            sample_result(),
            Duration::from_millis(1),
        );
        let mut overlay = RepromptOverlay::new(data);
        std::thread::sleep(Duration::from_millis(5));
        let action = overlay.tick();
        assert!(matches!(action, Some(RepromptOverlayAction::Accept(_))));
        assert!(overlay.is_complete());
    }

    #[test]
    fn scroll_keys_adjust_offset() {
        let mut overlay = make_overlay();
        assert_eq!(overlay.scroll_offset, 0);

        overlay.handle_key(KeyEvent::new(
            KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(overlay.scroll_offset, 1);

        overlay.handle_key(KeyEvent::new(
            KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(overlay.scroll_offset, 2);

        overlay.handle_key(KeyEvent::new(
            KeyCode::Up,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(overlay.scroll_offset, 1);

        // Can't go below 0.
        overlay.handle_key(KeyEvent::new(
            KeyCode::Up,
            crossterm::event::KeyModifiers::NONE,
        ));
        overlay.handle_key(KeyEvent::new(
            KeyCode::Up,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(overlay.scroll_offset, 0);
    }

    #[test]
    fn j_k_scroll_like_arrows() {
        let mut overlay = make_overlay();
        overlay.handle_key(KeyEvent::new(
            KeyCode::Char('j'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(overlay.scroll_offset, 1);

        overlay.handle_key(KeyEvent::new(
            KeyCode::Char('k'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(overlay.scroll_offset, 0);
    }

    #[test]
    fn desired_height_capped_at_max() {
        let long_text = "line\n".repeat(100);
        let result = RepromptResult {
            refined_prompt: long_text,
            applied_rules: vec![],
            reasoning: String::new(),
            task_type: TaskType::Analysis,
            was_substantive_change: true,
        };
        let data = RepromptOverlayData::new("short".to_string(), result, Duration::from_secs(5));
        let overlay = RepromptOverlay::new(data);
        let height = overlay.desired_height(100);
        assert_eq!(height, MAX_OVERLAY_HEIGHT);
    }

    #[test]
    fn render_produces_output() {
        let overlay = make_overlay();
        let width = 100;
        let height = overlay.desired_height(width);
        assert!(height > 0);
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        overlay.render_widget(area, &mut buf);

        let rendered: String = (0..buf.area.height)
            .map(|row| {
                (0..buf.area.width)
                    .map(|col| buf[(col, row)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            rendered.contains("REPROMPT"),
            "expected REPROMPT border title"
        );
        assert!(rendered.contains("Original"), "expected Original section");
        assert!(rendered.contains("Refined"), "expected Refined section");
        assert!(rendered.contains("accept"), "expected accept key hint");
        assert!(rendered.contains("skip"), "expected skip key hint");
        assert!(rendered.contains("cancel"), "expected cancel key hint");
    }

    #[test]
    fn render_at_nonzero_y_offset() {
        let overlay = make_overlay();
        let width = 100;
        let height = overlay.desired_height(width);
        // Simulate an inline viewport starting at y=50.
        let area = Rect::new(0, 50, width, height);
        let mut buf = Buffer::empty(area);
        overlay.render_widget(area, &mut buf);

        let rendered: String = (area.y..area.bottom())
            .map(|row| {
                (0..buf.area.width)
                    .map(|col| buf[(col, row)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            rendered.contains("REPROMPT"),
            "expected REPROMPT at non-zero y"
        );
    }
}
