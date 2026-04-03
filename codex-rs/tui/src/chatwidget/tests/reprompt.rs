use super::*;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::time::Duration;
use tokio::sync::broadcast;

fn sample_reprompt_result(was_substantive_change: bool) -> crate::reprompt::RepromptResult {
    crate::reprompt::RepromptResult {
        refined_prompt: "Refined prompt".to_string(),
        applied_rules: vec!["Make implicit requirements explicit".to_string()],
        reasoning: "Clarified the requested change".to_string(),
        task_type: crate::reprompt::TaskType::Analysis,
        was_substantive_change,
        tips: vec![],
    }
}

#[tokio::test]
async fn substantive_reprompt_result_clears_loading_without_committing_it() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.active_cell = Some(Box::new(crate::history_cell::new_reprompt_loading(
        /*animations_enabled*/ false,
    )));
    chat.bump_active_cell_revision();

    chat.on_reprompt_refinement_result(
        "Original prompt".to_string(),
        sample_reprompt_result(/*was_substantive_change*/ true),
        chat.reprompt_generation,
    );

    assert!(chat.active_cell.is_none(), "expected loader to be cleared");
    assert!(
        chat.reprompt_overlay.is_some(),
        "expected overlay to replace the loader"
    );
    assert!(
        rx.try_recv().is_err(),
        "expected loader to be removed, not committed into history"
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn reprompt_loading_render_schedules_animation_frame() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let (draw_tx, mut draw_rx) = broadcast::channel(8);
    chat.frame_requester = FrameRequester::new(draw_tx);
    chat.active_cell = Some(Box::new(crate::history_cell::new_reprompt_loading(
        /*animations_enabled*/ true,
    )));

    let area = Rect::new(0, 0, 80, 6);
    let mut buf = Buffer::empty(area);
    chat.render(area, &mut buf);

    tokio::time::advance(crate::tui::TARGET_FRAME_INTERVAL + Duration::from_millis(1)).await;

    let draw = tokio::time::timeout(Duration::from_millis(20), draw_rx.recv()).await;
    assert!(
        draw.is_ok(),
        "expected render to schedule a follow-up frame"
    );
    assert!(
        draw.expect("draw result").is_ok(),
        "draw channel should stay open"
    );
}
