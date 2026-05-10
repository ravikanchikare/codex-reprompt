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
//!   Enter/Space/a = accept refined prompt
//!   i             = iterate on the refined prompt in the composer
//!   s      = skip refinement, send original
//!   r      = show reasoning
//!   Esc/c  = cancel submission entirely
//!   Up/k   = scroll up
//!   Down/j = scroll down
//!   PageUp/PageDown = scroll by page
//!   Home/End = jump to top/bottom

use std::cell::Cell;
use std::time::Duration;
use std::time::Instant;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Alignment;
use ratatui::layout::Constraint;
use ratatui::layout::Direction;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use super::RepromptOverlayAction;
use super::RepromptOverlayData;
use super::RepromptOverlayState;
use crate::wrapping::RtOptions;
use crate::wrapping::word_wrap_lines;

/// Maximum height the overlay will claim (in terminal rows).
const MAX_OVERLAY_HEIGHT: u16 = 80;
const TIP_ROTATION_INTERVAL: Duration = Duration::from_secs(5);

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
    /// Height of the visible content area from the last render.
    last_content_height: Cell<Option<u16>>,
    /// Currently visible tip from `result.tips`.
    tip_index: usize,
    /// When the active tip was last rotated.
    last_tip_rotate: Instant,
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
            last_content_height: Cell::new(None),
            tip_index: 0,
            last_tip_rotate: Instant::now(),
        }
    }

    /// Return the action the user chose, if the overlay completed.
    pub(crate) fn take_action(&mut self) -> Option<RepromptOverlayAction> {
        self.action.take()
    }

    /// Access the overlay data (for persistence and insights).
    pub(crate) fn data(&self) -> &RepromptOverlayData {
        &self.data
    }

    /// Handle a key event and return the resulting action.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> RepromptOverlayAction {
        if key.kind != KeyEventKind::Press {
            return RepromptOverlayAction::None;
        }

        let code = key.code;

        if matches!(self.data.state, RepromptOverlayState::Editing(_)) {
            if self.handle_scroll_key(code) {
                return RepromptOverlayAction::None;
            }
            return match code {
                KeyCode::Char('c' | 'C') | KeyCode::Esc => {
                    self.data.state = RepromptOverlayState::Reviewing;
                    RepromptOverlayAction::None
                }
                KeyCode::Char('a' | 'A') | KeyCode::Enter | KeyCode::Char(' ') => {
                    let edited = match &self.data.state {
                        RepromptOverlayState::Editing(text) => text.clone(),
                        _ => unreachable!("editing state should still be active"),
                    };
                    self.data.state = RepromptOverlayState::Accepted(edited.clone());
                    let action = RepromptOverlayAction::Accept(edited);
                    self.action = Some(action.clone());
                    self.done = true;
                    action
                }
                _ => RepromptOverlayAction::None,
            };
        }

        if !matches!(self.data.state, RepromptOverlayState::Reviewing) {
            return RepromptOverlayAction::None;
        }

        if self.handle_scroll_key(code) {
            return RepromptOverlayAction::None;
        }

        match code {
            KeyCode::Char('a' | 'A') | KeyCode::Enter | KeyCode::Char(' ') => {
                let text = self.data.result.refined_prompt.clone();
                self.data.state = RepromptOverlayState::Accepted(text.clone());
                let action = RepromptOverlayAction::Accept(text);
                self.action = Some(action.clone());
                self.done = true;
                action
            }
            KeyCode::Char('i' | 'I') => {
                let text = self.data.result.refined_prompt.clone();
                self.data.state = RepromptOverlayState::Accepted(text.clone());
                let action = RepromptOverlayAction::Iterate(text);
                self.action = Some(action.clone());
                self.done = true;
                action
            }
            KeyCode::Char('s' | 'S') => {
                self.data.state = RepromptOverlayState::Skipped;
                let action = RepromptOverlayAction::Skip;
                self.action = Some(action.clone());
                self.done = true;
                action
            }
            KeyCode::Char('r' | 'R') => {
                self.show_reasoning = !self.show_reasoning;
                // Reset scroll when toggling reasoning visibility.
                self.scroll_offset = 0;
                RepromptOverlayAction::ShowReasoning
            }
            KeyCode::Char('c' | 'C') | KeyCode::Esc => {
                self.data.state = RepromptOverlayState::Cancelled;
                let action = RepromptOverlayAction::Cancel;
                self.action = Some(action.clone());
                self.done = true;
                action
            }
            _ => RepromptOverlayAction::None,
        }
    }

    /// Check for auto-accept timer expiry.
    pub(crate) fn tick(&mut self) -> Option<RepromptOverlayAction> {
        self.rotate_tip_if_needed();
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

    fn rotate_tip_if_needed(&mut self) {
        if self.data.result.tips.len() <= 1 {
            return;
        }

        let rotation_count =
            self.last_tip_rotate.elapsed().as_secs() / TIP_ROTATION_INTERVAL.as_secs();
        if rotation_count == 0 {
            return;
        }

        self.tip_index = (self.tip_index + rotation_count as usize) % self.data.result.tips.len();
        self.last_tip_rotate = Instant::now();
    }

    fn wrap_body_lines<I>(lines: I, width: u16) -> Vec<Line<'static>>
    where
        I: IntoIterator<Item = Line<'static>>,
    {
        word_wrap_lines(
            lines,
            RtOptions::new(width.max(1) as usize)
                .initial_indent(Line::from("  "))
                .subsequent_indent(Line::from("  ")),
        )
    }

    /// Build the content lines (everything above the footer).
    fn build_content_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        // Original prompt
        lines.push(Line::from("  Original Prompt:".bold().dim()));
        lines.extend(Self::wrap_body_lines(
            self.data
                .original
                .split('\n')
                .map(|line| Line::from(line.to_string()).dim()),
            width,
        ));
        lines.push(Line::from(""));

        // Refined prompt
        lines.push(Line::from("  Did You Mean:".bold().green()));
        lines.extend(Self::wrap_body_lines(
            self.data
                .result
                .refined_prompt
                .split('\n')
                .map(|line| Line::from(line.to_string()).green()),
            width,
        ));
        lines.push(Line::from(""));

        // Applied rules
        if !self.data.result.applied_rules.is_empty() {
            let rules_text = self.data.result.applied_rules.join(", ");
            lines.push(Line::from("  Rules Applied:".bold()));
            lines.extend(Self::wrap_body_lines(
                std::iter::once(Line::from(rules_text).cyan()),
                width,
            ));
            lines.push(Line::from(""));
        }

        // Reasoning (toggled by 'r')
        if self.show_reasoning {
            lines.push(Line::from("  Reasoning:".bold()));
            lines.extend(Self::wrap_body_lines(
                self.data
                    .result
                    .reasoning
                    .split('\n')
                    .map(|line| Line::from(line.to_string()).dim()),
                width,
            ));
        }

        lines
    }

    fn build_content_paragraph(&self, width: u16) -> Paragraph<'static> {
        Paragraph::new(self.build_content_lines(width))
    }

    fn content_height(&self, width: u16) -> u16 {
        if width == 0 {
            return 0;
        }
        self.build_content_lines(width)
            .len()
            .try_into()
            .unwrap_or(u16::MAX)
    }

    fn handle_scroll_key(&mut self, code: KeyCode) -> bool {
        let page_height = self
            .last_content_height
            .get()
            .unwrap_or(MAX_OVERLAY_HEIGHT.saturating_sub(4))
            .max(1);
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
                true
            }
            KeyCode::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(page_height);
                true
            }
            KeyCode::PageDown => {
                self.scroll_offset = self.scroll_offset.saturating_add(page_height);
                true
            }
            KeyCode::Home => {
                self.scroll_offset = 0;
                true
            }
            KeyCode::End => {
                self.scroll_offset = u16::MAX;
                true
            }
            _ => false,
        }
    }

    fn scroll_indicator(clamped_scroll: u16, max_scroll: u16) -> Option<&'static str> {
        if max_scroll == 0 {
            return None;
        }

        Some(match (clamped_scroll > 0, clamped_scroll < max_scroll) {
            (false, true) => "\u{25bc}", // ▼
            (true, false) => "\u{25b2}", // ▲
            (true, true) => "\u{2195}",  // ↕
            (false, false) => return None,
        })
    }

    fn build_action_hint_line(&self) -> Line<'static> {
        let mut spans: Vec<Span<'static>> = vec!["  ".into()];
        let separators = " · ";
        let hints = [
            ('A', "ccept"),
            ('I', "terate"),
            ('S', "kip"),
            ('R', "easoning"),
            ('C', "ancel"),
        ];

        for (idx, (key, label)) in hints.into_iter().enumerate() {
            if idx > 0 {
                spans.push(separators.into());
            }
            spans.push(format!("[{key}]").cyan().bold());
            spans.push(label.into());
        }

        Line::from(spans)
    }

    fn navigation_hint_line(&self) -> Line<'static> {
        let mut text = "↑/↓ to scroll · Space, Enter, or Escape to dismiss".to_string();
        if let Some(remaining) = self.data.auto_accept_remaining()
            && remaining > 0
        {
            text.push_str(&format!("  ({remaining}s)"));
        }
        Line::from(format!("  {text}").dim())
    }

    fn build_tip_line(&self) -> Option<Line<'static>> {
        let total_tips = self.data.result.tips.len();
        if total_tips == 0 {
            return None;
        }

        let tip = self
            .data
            .result
            .tips
            .get(self.tip_index % total_tips)
            .cloned()
            .unwrap_or_default();
        Some(Line::from(
            format!("  Tip {}/{total_tips}: {tip}", self.tip_index + 1).dim(),
        ))
    }

    /// Build the footer lines (separator + key hints) — always pinned to bottom.
    fn build_footer_lines(&self, width: u16) -> Vec<Line<'static>> {
        let inner_width = width.saturating_sub(2) as usize;
        let separator = "─".repeat(inner_width).dim();
        let mut lines = vec![Line::from(separator.clone()), self.build_action_hint_line()];
        if let Some(tip_line) = self.build_tip_line() {
            lines.push(Line::from(separator));
            lines.push(tip_line);
            lines.push(Line::default());
        }
        lines.push(self.navigation_hint_line());
        lines
    }

    /// Whether the overlay has completed (user made a decision).
    pub(crate) fn is_complete(&self) -> bool {
        self.done
    }

    /// Whether the overlay needs periodic redraws for the auto-accept
    /// countdown or rotating tip strip.
    pub(crate) fn has_active_countdown(&self) -> bool {
        let countdown_active = matches!(self.data.state, RepromptOverlayState::Reviewing)
            && self
                .data
                .auto_accept_remaining()
                .is_some_and(|secs| secs > 0);
        let rotating_tips_active = self.data.result.tips.len() > 1 && !self.done;

        countdown_active || rotating_tips_active
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
        let content_width = width.saturating_sub(2);
        let content_lines = self.content_height(content_width);
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
            .border_style(Style::default().cyan())
            .title(" REPROMPT ".bold());
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let content_paragraph = self.build_content_paragraph(inner.width);
        let total_content = self.content_height(inner.width);
        let footer_lines = self.build_footer_lines(area.width);
        let footer_height = footer_lines.len() as u16;

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(footer_height)])
            .split(inner);

        let content_area = chunks[0];
        let footer_area = chunks[1];
        self.last_content_height.set(Some(content_area.height));

        let footer_lines = self.build_footer_lines(area.width);

        // Clamp scroll offset: can't scroll past the content.
        let max_scroll = total_content.saturating_sub(content_area.height);
        let clamped_scroll = self.scroll_offset.min(max_scroll);

        content_paragraph
            .scroll((clamped_scroll, 0))
            .render(content_area, buf);

        if let Some(indicator) = Self::scroll_indicator(clamped_scroll, max_scroll) {
            let indicator_line =
                Line::from(format!(" {indicator} ").dim()).alignment(Alignment::Right);
            let indicator_area = Rect::new(
                content_area.right().saturating_sub(4),
                content_area.y,
                4.min(content_area.width),
                1,
            );
            Paragraph::new(vec![indicator_line]).render(indicator_area, buf);
        }

        let footer_paragraph = Paragraph::new(footer_lines);
        footer_paragraph.render(footer_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reprompt::config::RepromptResult;
    use crate::reprompt::config::TaskType;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;
    use std::time::Duration;

    fn sample_result() -> RepromptResult {
        RepromptResult {
            refined_prompt: "Apply the JWT fix to payments".to_string(),
            applied_rules: vec!["regression test required".to_string()],
            reasoning: "Expanded vague reference".to_string(),
            task_type: TaskType::Bugfix,
            was_substantive_change: true,
            tips: vec![
                "Add a file path to skip discovery".to_string(),
                "Name the exact bug instead of 'it'".to_string(),
                "Add a verification step like tests".to_string(),
            ],
        }
    }

    fn make_overlay() -> RepromptOverlay {
        make_overlay_with(
            "fix it in payments too".to_string(),
            "Apply the JWT fix to payments".to_string(),
        )
    }

    fn make_overlay_with(original: String, refined_prompt: String) -> RepromptOverlay {
        let data = RepromptOverlayData::new(
            original,
            RepromptResult {
                refined_prompt,
                ..sample_result()
            },
            Duration::from_secs(5),
        );
        RepromptOverlay::new(data)
    }

    fn render_overlay_to_string(overlay: &RepromptOverlay, area: Rect) -> String {
        let mut buf = Buffer::empty(area);
        overlay.render_widget(area, &mut buf);
        (area.y..area.bottom())
            .map(|row| {
                (area.x..area.right())
                    .map(|col| buf[(col, row)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
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
    fn space_accepts_refined_prompt() {
        let mut overlay = make_overlay();
        let action = overlay.handle_key(KeyEvent::new(
            KeyCode::Char(' '),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(
            matches!(action, RepromptOverlayAction::Accept(ref t) if t == "Apply the JWT fix to payments")
        );
        assert!(overlay.is_complete());
    }

    #[test]
    fn i_iterates_refined_prompt() {
        let mut overlay = make_overlay();
        let action = overlay.handle_key(KeyEvent::new(
            KeyCode::Char('i'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(
            matches!(action, RepromptOverlayAction::Iterate(ref t) if t == "Apply the JWT fix to payments")
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
    fn editing_enter_accepts() {
        let mut overlay = make_overlay();
        overlay.data.state =
            RepromptOverlayState::Editing("Apply the JWT fix to payments".to_string());
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
    fn editing_esc_returns_to_reviewing() {
        let mut overlay = make_overlay();
        overlay.data.state =
            RepromptOverlayState::Editing("Apply the JWT fix to payments".to_string());
        assert!(matches!(
            overlay.data.state,
            RepromptOverlayState::Editing(_)
        ));
        overlay.handle_key(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(matches!(
            overlay.data.state,
            RepromptOverlayState::Reviewing
        ));
        assert!(!overlay.is_complete());
    }

    #[test]
    fn keys_ignored_in_terminal_states() {
        let mut overlay = make_overlay();
        // Accept, then verify further keys are ignored.
        overlay.handle_key(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(overlay.is_complete());
        let action = overlay.handle_key(KeyEvent::new(
            KeyCode::Char('s'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(action, RepromptOverlayAction::None);
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
    fn tick_rotates_visible_tip_every_five_seconds() {
        let data =
            RepromptOverlayData::new("original".to_string(), sample_result(), Duration::ZERO);
        let mut overlay = RepromptOverlay::new(data);
        overlay.last_tip_rotate = Instant::now() - Duration::from_secs(5);

        let action = overlay.tick();

        assert_eq!(action, None);
        assert_eq!(overlay.tip_index, 1);
    }

    #[test]
    fn rotating_tips_request_periodic_redraws() {
        let data =
            RepromptOverlayData::new("original".to_string(), sample_result(), Duration::ZERO);
        let overlay = RepromptOverlay::new(data);

        assert!(overlay.has_active_countdown());
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
    fn editing_scroll_keys_match_reviewing() {
        let mut overlay = make_overlay();
        overlay.data.state =
            RepromptOverlayState::Editing("Apply the JWT fix to payments".to_string());
        overlay.handle_key(KeyEvent::new(
            KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(overlay.scroll_offset, 1);
        overlay.handle_key(KeyEvent::new(
            KeyCode::Up,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(overlay.scroll_offset, 0);
    }

    #[test]
    fn page_navigation_uses_visible_content_height() {
        let long_text = "abcdefghijklmnopqrstuvwxyz ".repeat(30);
        let mut overlay = make_overlay_with("short".to_string(), long_text);
        let area = Rect::new(0, 0, 36, 10);
        let _ = render_overlay_to_string(&overlay, area);
        let page_height = overlay.last_content_height.get().unwrap_or_default();
        assert!(
            page_height > 0,
            "expected render to record visible content height"
        );

        overlay.handle_key(KeyEvent::new(
            KeyCode::PageDown,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(overlay.scroll_offset, page_height);

        overlay.handle_key(KeyEvent::new(
            KeyCode::PageUp,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(overlay.scroll_offset, 0);

        overlay.handle_key(KeyEvent::new(
            KeyCode::End,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(overlay.scroll_offset, u16::MAX);

        overlay.handle_key(KeyEvent::new(
            KeyCode::Home,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(overlay.scroll_offset, 0);
    }

    #[test]
    fn desired_height_counts_wrapped_lines() {
        let long_text = "abcdefghijklmnopqrstuvwxyz ".repeat(20);
        let overlay = make_overlay_with("short".to_string(), long_text);

        let wide = overlay.desired_height(100);
        let narrow = overlay.desired_height(32);

        assert!(
            narrow > wide,
            "expected wrapped height to grow at narrower widths"
        );
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
            tips: vec![],
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
        let rendered = render_overlay_to_string(&overlay, Rect::new(0, 0, width, height));
        assert!(
            rendered.contains("REPROMPT"),
            "expected REPROMPT border title"
        );
        assert!(
            rendered.contains("Original Prompt:"),
            "expected Original Prompt section"
        );
        assert!(
            rendered.contains("Did You Mean:"),
            "expected Did You Mean section"
        );
        assert!(rendered.contains("[A]ccept"), "expected accept key hint");
        assert!(rendered.contains("[I]terate"), "expected iterate key hint");
        assert!(rendered.contains("Tip 1/3:"), "expected rotating tip strip");
        assert!(
            rendered.contains("↑/↓ to scroll · Space, Enter, or Escape to dismiss"),
            "expected dismiss footer"
        );
    }

    #[test]
    fn render_at_nonzero_y_offset() {
        let overlay = make_overlay();
        let width = 100;
        let height = overlay.desired_height(width);
        // Simulate an inline viewport starting at y=50.
        let area = Rect::new(0, 50, width, height);
        let rendered = render_overlay_to_string(&overlay, area);
        assert!(
            rendered.contains("REPROMPT"),
            "expected REPROMPT at non-zero y"
        );
    }

    #[test]
    fn long_wrapped_content_scrolls_rendered_output() {
        let long_text = "abcdefghijklmnopqrstuvwxyz ".repeat(40);
        let mut overlay = make_overlay_with("short".to_string(), long_text);
        let area = Rect::new(0, 0, 36, 10);

        let initial = render_overlay_to_string(&overlay, area);
        overlay.handle_key(KeyEvent::new(
            KeyCode::End,
            crossterm::event::KeyModifiers::NONE,
        ));
        let scrolled = render_overlay_to_string(&overlay, area);

        assert_ne!(
            initial, scrolled,
            "expected scroll to change rendered output"
        );
    }

    #[test]
    fn wrapped_body_continuations_keep_two_space_indent() {
        let overlay = make_overlay_with(
            "Please update the auth flow to handle sandboxed sessions cleanly".to_string(),
            "Please update the auth flow to handle sandboxed sessions cleanly, preserve the existing OAuth fallback, and add a focused regression test for the macOS proxy path.".to_string(),
        );
        let rendered = render_overlay_to_string(&overlay, Rect::new(0, 0, 44, 13));

        assert!(
            rendered
                .lines()
                .any(|line| line.contains("│  sandboxed sessions cleanly")),
            "expected wrapped continuation lines to preserve the body indent"
        );
    }

    #[test]
    fn footer_leaves_blank_line_before_navigation_hint() {
        let overlay = make_overlay();
        let width = 60;
        let rendered = render_overlay_to_string(
            &overlay,
            Rect::new(0, 0, width, overlay.desired_height(width)),
        );
        let lines = rendered.lines().collect::<Vec<_>>();
        let tip_idx = lines
            .iter()
            .position(|line| line.contains("Tip 1/3:"))
            .expect("expected tip line in footer");

        assert_eq!(
            lines[tip_idx + 1],
            "│                                                          │"
        );
        assert!(
            lines[tip_idx + 2].contains("↑/↓ to scroll · Space, Enter, or Escape to dismiss"),
            "expected updated navigation hint after spacer line"
        );
    }

    #[test]
    fn reprompt_overlay_short_content_snapshot() {
        let overlay = make_overlay();
        let width = 60;
        let rendered = render_overlay_to_string(
            &overlay,
            Rect::new(0, 0, width, overlay.desired_height(width)),
        );
        assert_snapshot!("reprompt_overlay_short_content", rendered);
    }

    #[test]
    fn reprompt_overlay_long_wrapped_content_snapshot() {
        let overlay = make_overlay_with(
            "Please update the auth flow to handle sandboxed sessions cleanly".to_string(),
            "Please update the auth flow to handle sandboxed sessions cleanly, preserve the existing OAuth fallback, and add a focused regression test for the macOS proxy path.".to_string(),
        );
        let rendered = render_overlay_to_string(&overlay, Rect::new(0, 0, 44, 13));
        assert_snapshot!("reprompt_overlay_long_wrapped_content", rendered);
    }
}
