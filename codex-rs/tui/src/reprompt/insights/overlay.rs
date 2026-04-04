//! InsightsOverlay — a scrollable TUI widget for `/reprompt-insights` results.
//!
//! Renders a coaching report showing skill assessment, gap patterns,
//! suggestions, and Reprompt quality evaluation. Follows the same overlay
//! pattern as `reprompt/overlay.rs`.
//!
//! Key handling:
//!   Esc/d   = dismiss overlay
//!   Up/k    = scroll up
//!   Down/j  = scroll down
//!   PageUp/PageDown = scroll by page
//!   Home/End = jump to top/bottom

use std::cell::Cell;

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

use super::InsightsResult;
use super::SkillLevel;
use crate::wrapping::RtOptions;
use crate::wrapping::word_wrap_lines;

/// Maximum height the overlay will claim (in terminal rows).
const MAX_OVERLAY_HEIGHT: u16 = 80;

/// Action emitted by the insights overlay's key-event handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InsightsOverlayAction {
    /// Dismiss the overlay.
    Dismiss,
    /// No actionable event.
    None,
}

/// Modal overlay that presents `/reprompt-insights` results to the user.
pub(crate) struct InsightsOverlay {
    data: InsightsResult,
    done: bool,
    /// Vertical scroll offset for the content area.
    scroll_offset: u16,
    /// Height of the visible content area from the last render.
    last_content_height: Cell<Option<u16>>,
}

#[allow(dead_code)]
impl InsightsOverlay {
    pub(crate) fn new(data: InsightsResult) -> Self {
        Self {
            data,
            done: false,
            scroll_offset: 0,
            last_content_height: Cell::new(None),
        }
    }

    /// Whether the overlay has been dismissed.
    pub(crate) fn is_done(&self) -> bool {
        self.done
    }

    /// Handle a key event and return the resulting action.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> InsightsOverlayAction {
        if key.kind != KeyEventKind::Press {
            return InsightsOverlayAction::None;
        }

        if self.done {
            return InsightsOverlayAction::None;
        }

        if self.handle_scroll_key(key.code) {
            return InsightsOverlayAction::None;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('d' | 'D') | KeyCode::Char('q' | 'Q') => {
                self.done = true;
                InsightsOverlayAction::Dismiss
            }
            _ => InsightsOverlayAction::None,
        }
    }

    /// Handle Ctrl-C as dismiss.
    pub(crate) fn on_ctrl_c(&mut self) {
        if !self.done {
            self.done = true;
        }
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

    fn wrap_body_lines<I>(lines: I, width: u16) -> Vec<Line<'static>>
    where
        I: IntoIterator<Item = Line<'static>>,
    {
        word_wrap_lines(
            lines,
            RtOptions::new(width.max(1) as usize)
                .initial_indent(Line::from("  "))
                .subsequent_indent(Line::from("    ")),
        )
    }

    fn skill_level_span(level: SkillLevel) -> Span<'static> {
        let text = level.to_string();
        match level {
            SkillLevel::Beginner => Span::from(text).dim(),
            SkillLevel::Intermediate => Span::from(text),
            SkillLevel::Advanced | SkillLevel::Expert => Span::from(text).green(),
        }
    }

    /// Build the content lines (everything above the footer).
    fn build_content_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        // Skill assessment
        lines.push(Line::from(vec![
            "  Skill Level: ".bold(),
            Self::skill_level_span(self.data.skill_assessment.level),
        ]));
        lines.extend(Self::wrap_body_lines(
            std::iter::once(Line::from(self.data.skill_assessment.explanation.clone()).dim()),
            width,
        ));
        if !self.data.skill_assessment.top_improvement.is_empty() {
            lines.extend(Self::wrap_body_lines(
                std::iter::once(Line::from(format!(
                    "Top improvement: {}",
                    self.data.skill_assessment.top_improvement
                ))),
                width,
            ));
        }
        lines.push(Line::from(""));

        // Gaps
        if !self.data.gaps.is_empty() {
            let total = self.data.gaps.first().map_or(0, |g| g.total);
            lines.push(Line::from(
                format!("  Common Gaps (across {total} refinements):").bold(),
            ));
            for gap in &self.data.gaps {
                let pct = if gap.total > 0 {
                    gap.count * 100 / gap.total
                } else {
                    0
                };
                let dots = ".".repeat(28usize.saturating_sub(gap.category.len()));
                lines.push(Line::from(vec![
                    format!("  \u{25cf} {}", gap.category).cyan(),
                    format!(" {dots} {}/{} ({pct}%)", gap.count, gap.total).dim(),
                ]));
            }
            lines.push(Line::from(""));
        }

        // Patterns
        if !self.data.patterns.is_empty() {
            lines.push(Line::from("  Patterns:".bold()));
            for pattern in &self.data.patterns {
                lines.extend(Self::wrap_body_lines(
                    std::iter::once(Line::from(format!("\u{25cf} {pattern}"))),
                    width,
                ));
            }
            lines.push(Line::from(""));
        }

        // Suggestions
        if !self.data.suggestions.is_empty() {
            lines.push(Line::from("  Top Suggestions:".bold()));
            for (i, suggestion) in self.data.suggestions.iter().enumerate() {
                lines.extend(Self::wrap_body_lines(
                    std::iter::once(Line::from(format!(
                        "{}. {} \u{2014} {}",
                        i + 1,
                        suggestion.title,
                        suggestion.detail,
                    ))),
                    width,
                ));
                if let Some(example) = &suggestion.example {
                    lines.extend(Self::wrap_body_lines(
                        std::iter::once(Line::from(format!("   e.g. {example}")).dim()),
                        width,
                    ));
                }
            }
            lines.push(Line::from(""));
        }

        // Reprompt quality
        if let Some(quality) = &self.data.reprompt_quality {
            lines.push(Line::from("  Reprompt Performance:".bold()));
            lines.push(Line::from(vec![
                "  \u{2713} ".green(),
                format!(
                    "Intent preserved: {}/{}",
                    quality.intent_preserved_count, quality.total
                )
                .into(),
            ]));
            if quality.scope_creep_count > 0 {
                lines.push(Line::from(vec![
                    "  \u{2717} ".red(),
                    format!(
                        "Scope creep: {}/{}",
                        quality.scope_creep_count, quality.total
                    )
                    .into(),
                ]));
            }
            lines.extend(Self::wrap_body_lines(
                std::iter::once(Line::from(quality.assessment.clone()).dim()),
                width,
            ));
        }

        lines
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

    fn build_footer_lines(&self, width: u16) -> Vec<Line<'static>> {
        let inner_width = width.saturating_sub(2) as usize;
        let separator = "\u{2500}".repeat(inner_width).dim();
        vec![
            Line::from(separator),
            Line::from(vec!["  ".into(), "[Esc]".cyan().bold(), " Dismiss".into()]),
            Line::from("  \u{2191}/\u{2193} to scroll".to_string().dim()),
        ]
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

    /// Desired height for rendering, capped at [`MAX_OVERLAY_HEIGHT`].
    pub(crate) fn desired_height(&self, width: u16) -> u16 {
        let content_width = width.saturating_sub(2);
        let content_lines = self.content_height(content_width);
        let footer_lines = self.build_footer_lines(width).len() as u16;
        (content_lines + footer_lines + 2).min(MAX_OVERLAY_HEIGHT)
    }

    /// Render the overlay into a buffer area.
    pub(crate) fn render_widget(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().cyan())
            .title(" REPROMPT INSIGHTS ".bold());
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let content_paragraph = Paragraph::new(self.build_content_lines(inner.width));
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

        Paragraph::new(footer_lines).render(footer_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reprompt::insights::InsightGap;
    use crate::reprompt::insights::InsightSuggestion;
    use crate::reprompt::insights::RepromptQuality;
    use crate::reprompt::insights::SkillAssessment;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;

    fn sample_insights() -> InsightsResult {
        InsightsResult {
            skill_assessment: SkillAssessment {
                level: SkillLevel::Intermediate,
                explanation: "Clear goals but often omits file paths and verification steps."
                    .to_string(),
                top_improvement: "Include @file paths when you know them.".to_string(),
            },
            gaps: vec![
                InsightGap {
                    category: "missing_path".to_string(),
                    count: 6,
                    total: 8,
                    description: "File paths omitted.".to_string(),
                    example_original: "fix the auth bug in payments".to_string(),
                    example_fix: "fix the JWT bug in @src/payments/auth.rs".to_string(),
                },
                InsightGap {
                    category: "ambiguous_reference".to_string(),
                    count: 4,
                    total: 8,
                    description: "Vague pronoun references.".to_string(),
                    example_original: "fix it too".to_string(),
                    example_fix: "apply the same JWT fix to auth.rs".to_string(),
                },
            ],
            patterns: vec![
                "Frequently uses pronouns instead of specific names.".to_string(),
                "Rarely includes test requirements.".to_string(),
            ],
            suggestions: vec![
                InsightSuggestion {
                    title: "Always include file paths".to_string(),
                    detail: "Specify @file paths to help the agent find code faster.".to_string(),
                    example: Some("fix JWT in @src/payments/auth.rs".to_string()),
                },
                InsightSuggestion {
                    title: "Name specific errors".to_string(),
                    detail: "Replace pronouns with the actual error or bug name.".to_string(),
                    example: None,
                },
            ],
            reprompt_quality: Some(RepromptQuality {
                intent_preserved_count: 7,
                scope_creep_count: 1,
                total: 8,
                assessment: "Good refinement quality with minimal scope creep.".to_string(),
            }),
        }
    }

    fn make_overlay() -> InsightsOverlay {
        InsightsOverlay::new(sample_insights())
    }

    fn render_overlay_to_string(overlay: &InsightsOverlay, area: Rect) -> String {
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
    fn esc_dismisses() {
        let mut overlay = make_overlay();
        let action = overlay.handle_key(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(action, InsightsOverlayAction::Dismiss);
        assert!(overlay.is_done());
    }

    #[test]
    fn d_dismisses() {
        let mut overlay = make_overlay();
        let action = overlay.handle_key(KeyEvent::new(
            KeyCode::Char('d'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(action, InsightsOverlayAction::Dismiss);
        assert!(overlay.is_done());
    }

    #[test]
    fn q_dismisses() {
        let mut overlay = make_overlay();
        let action = overlay.handle_key(KeyEvent::new(
            KeyCode::Char('q'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(action, InsightsOverlayAction::Dismiss);
        assert!(overlay.is_done());
    }

    #[test]
    fn unknown_key_is_noop() {
        let mut overlay = make_overlay();
        let action = overlay.handle_key(KeyEvent::new(
            KeyCode::Char('z'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(action, InsightsOverlayAction::None);
        assert!(!overlay.is_done());
    }

    #[test]
    fn keys_ignored_after_dismiss() {
        let mut overlay = make_overlay();
        overlay.handle_key(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        let action = overlay.handle_key(KeyEvent::new(
            KeyCode::Char('d'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(action, InsightsOverlayAction::None);
    }

    #[test]
    fn release_events_are_ignored() {
        let mut overlay = make_overlay();
        let action = overlay.handle_key(KeyEvent::new_with_kind(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
            KeyEventKind::Release,
        ));
        assert_eq!(action, InsightsOverlayAction::None);
        assert!(!overlay.is_done());
    }

    #[test]
    fn ctrl_c_dismisses() {
        let mut overlay = make_overlay();
        overlay.on_ctrl_c();
        assert!(overlay.is_done());
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
            KeyCode::Up,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(overlay.scroll_offset, 0);

        // Can't go below 0.
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
    fn home_end_navigation() {
        let mut overlay = make_overlay();
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
    fn render_produces_output() {
        let overlay = make_overlay();
        let width = 60;
        let height = overlay.desired_height(width);
        assert!(height > 0);
        let rendered = render_overlay_to_string(&overlay, Rect::new(0, 0, width, height));
        assert!(rendered.contains("REPROMPT INSIGHTS"), "expected title");
        assert!(
            rendered.contains("Skill Level:"),
            "expected skill assessment"
        );
        assert!(rendered.contains("missing_path"), "expected gap category");
        assert!(
            rendered.contains("Top Suggestions:"),
            "expected suggestions section"
        );
        assert!(
            rendered.contains("Reprompt Performance:"),
            "expected quality section"
        );
        assert!(rendered.contains("[Esc]"), "expected dismiss key hint");
    }

    #[test]
    fn desired_height_capped_at_max() {
        let mut insights = sample_insights();
        // Add many gaps to push content beyond max.
        for i in 0..100 {
            insights.gaps.push(InsightGap {
                category: format!("gap_{i}"),
                count: 1,
                total: 100,
                description: "Test gap.".to_string(),
                example_original: "test".to_string(),
                example_fix: "test fixed".to_string(),
            });
        }
        let overlay = InsightsOverlay::new(insights);
        let height = overlay.desired_height(60);
        assert_eq!(height, MAX_OVERLAY_HEIGHT);
    }

    #[test]
    fn insights_overlay_snapshot() {
        let overlay = make_overlay();
        let width = 60;
        let rendered = render_overlay_to_string(
            &overlay,
            Rect::new(0, 0, width, overlay.desired_height(width)),
        );
        assert_snapshot!("insights_overlay_full", rendered);
    }

    #[test]
    fn insights_overlay_no_quality_snapshot() {
        let mut insights = sample_insights();
        insights.reprompt_quality = None;
        let overlay = InsightsOverlay::new(insights);
        let width = 60;
        let rendered = render_overlay_to_string(
            &overlay,
            Rect::new(0, 0, width, overlay.desired_height(width)),
        );
        assert_snapshot!("insights_overlay_no_quality", rendered);
    }
}
