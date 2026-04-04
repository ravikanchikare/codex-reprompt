use super::*;
use pretty_assertions::assert_eq;
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

fn sample_skill() -> SkillMetadata {
    SkillMetadata {
        name: "repo:linter".to_string(),
        description: "Run linters across the repo".to_string(),
        short_description: None,
        interface: Some(codex_core::skills::model::SkillInterface {
            display_name: Some("Repo Linter".to_string()),
            short_description: None,
            icon_small: None,
            icon_large: None,
            brand_color: None,
            default_prompt: None,
        }),
        dependencies: None,
        policy: None,
        path_to_skills_md: PathBuf::from("/tmp/repo:linter/SKILL.md"),
        scope: codex_protocol::protocol::SkillScope::Repo,
    }
}

fn sample_plugin() -> codex_core::plugins::PluginCapabilitySummary {
    codex_core::plugins::PluginCapabilitySummary {
        config_name: "calendar-plugin@debug".to_string(),
        display_name: "Calendar Plugin".to_string(),
        description: Some("Plugin for calendar tasks".to_string()),
        has_skills: true,
        mcp_server_names: vec!["calendar-plugin".to_string()],
        app_connector_ids: Vec::new(),
    }
}

fn sample_app() -> codex_chatgpt::connectors::AppInfo {
    codex_chatgpt::connectors::AppInfo {
        id: "google_calendar".to_string(),
        name: "Google Calendar".to_string(),
        description: Some("Check availability".to_string()),
        logo_url: None,
        logo_url_dark: None,
        distribution_channel: None,
        branding: None,
        app_metadata: None,
        labels: None,
        install_url: None,
        is_accessible: true,
        is_enabled: true,
        plugin_display_names: Vec::new(),
    }
}

fn colliding_plugin() -> codex_core::plugins::PluginCapabilitySummary {
    codex_core::plugins::PluginCapabilitySummary {
        config_name: "google-calendar@debug".to_string(),
        display_name: "Google Calendar".to_string(),
        description: Some("Plugin for calendar tasks".to_string()),
        has_skills: true,
        mcp_server_names: vec!["google-calendar".to_string()],
        app_connector_ids: Vec::new(),
    }
}

fn sample_resolution_context() -> crate::reprompt::RepromptResolutionContext {
    let snapshot = crate::reprompt::ProjectContextSnapshot {
        entries: vec![
            crate::reprompt::project_context::ProjectContextEntry::new_for_test(
                "src/auth/token.rs",
            ),
        ],
    };
    crate::reprompt::relevant_context::build_resolution_context(
        Some(&snapshot),
        &[sample_skill()],
        &[sample_plugin()],
        &[sample_app()],
    )
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

#[tokio::test(flavor = "current_thread")]
async fn reprompt_loading_render_schedules_animation_frame() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let (draw_tx, mut draw_rx) = broadcast::channel(8);
    chat.frame_requester = FrameRequester::new(draw_tx);
    tokio::task::yield_now().await;
    chat.active_cell = Some(Box::new(crate::history_cell::new_reprompt_loading(
        /*animations_enabled*/ true,
    )));

    let area = Rect::new(0, 0, 80, 6);
    let mut buf = Buffer::empty(area);
    chat.render(area, &mut buf);
    tokio::task::yield_now().await;

    let draw = tokio::time::timeout(
        crate::tui::TARGET_FRAME_INTERVAL + Duration::from_millis(100),
        draw_rx.recv(),
    )
    .await;
    assert!(
        draw.is_ok(),
        "expected render to schedule a follow-up frame"
    );
    assert!(
        draw.expect("draw result").is_ok(),
        "draw channel should stay open"
    );
}

#[tokio::test]
async fn non_substantive_reprompt_submits_original_unredacted_text() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.reprompt_original_message = Some(UserMessage {
        text: "use sk-abcdefghijklmnopqrstuvwxyz123456".to_string(),
        local_images: vec![],
        remote_image_urls: vec![],
        text_elements: vec![],
        mention_bindings: vec![],
    });

    chat.on_reprompt_refinement_result(
        "use sk-abcdefghijklmnopqrstuvwxyz123456".to_string(),
        sample_reprompt_result(/*was_substantive_change*/ false),
        chat.reprompt_generation,
    );

    assert_eq!(
        chat.queued_user_messages
            .front()
            .map(|message| message.text.clone()),
        Some("use sk-abcdefghijklmnopqrstuvwxyz123456".to_string())
    );
}

#[tokio::test]
async fn accepted_reprompt_submits_structured_file_and_tool_items() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    chat.set_skills(Some(vec![sample_skill()]));
    chat.bottom_pane
        .set_plugin_mentions(Some(vec![sample_plugin()]));
    chat.has_chatgpt_account = true;
    let connectors_snapshot = ConnectorsSnapshot {
        connectors: vec![sample_app()],
    };
    chat.connectors_cache = ConnectorsCacheState::Ready(connectors_snapshot.clone());
    chat.bottom_pane
        .set_connectors_snapshot(Some(connectors_snapshot));
    chat.reprompt_original_message = Some(UserMessage::from("original prompt"));
    chat.reprompt_resolution_context = Some(sample_resolution_context());

    chat.submit_text_after_reprompt(
        "Inspect @src/auth/token.rs with $repo:linter, $calendar-plugin, and $google-calendar."
            .to_string(),
    );

    let Op::UserTurn { items, .. } = next_submit_op(&mut op_rx) else {
        panic!("expected Op::UserTurn");
    };

    assert!(items.iter().any(|item| {
        matches!(
            item,
            UserInput::Text { text, .. }
                if text.contains("@src/auth/token.rs") && text.contains("$repo:linter")
        )
    }));
    assert!(items.iter().any(|item| {
        matches!(
            item,
            UserInput::Skill { name, path }
                if name == "repo:linter" && path == &PathBuf::from("/tmp/repo:linter/SKILL.md")
        )
    }));
    assert!(items.iter().any(|item| {
        matches!(
            item,
            UserInput::Mention { name, path }
                if name == "src/auth/token.rs" && path == "/tmp/project/src/auth/token.rs"
        )
    }));
    assert!(items.iter().any(|item| {
        matches!(
            item,
            UserInput::Mention { name, path }
                if name == "Calendar Plugin" && path == "plugin://calendar-plugin@debug"
        )
    }));
    assert!(items.iter().any(|item| {
        matches!(
            item,
            UserInput::Mention { name, path }
                if name == "Google Calendar" && path == "app://google_calendar"
        )
    }));
}

#[tokio::test]
async fn accepted_reprompt_prefers_explicit_plugin_resolution_over_plain_app_inference() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    chat.bottom_pane
        .set_plugin_mentions(Some(vec![colliding_plugin()]));
    chat.has_chatgpt_account = true;
    let connectors_snapshot = ConnectorsSnapshot {
        connectors: vec![sample_app()],
    };
    chat.connectors_cache = ConnectorsCacheState::Ready(connectors_snapshot.clone());
    chat.bottom_pane
        .set_connectors_snapshot(Some(connectors_snapshot));
    chat.reprompt_original_message = Some(UserMessage::from("original prompt"));
    chat.reprompt_resolution_context =
        Some(crate::reprompt::relevant_context::build_resolution_context(
            None,
            &[],
            &[colliding_plugin()],
            &[sample_app()],
        ));

    chat.submit_text_after_reprompt("Use $google-calendar.".to_string());

    let Op::UserTurn { items, .. } = next_submit_op(&mut op_rx) else {
        panic!("expected Op::UserTurn");
    };

    assert!(items.iter().any(|item| {
        matches!(
            item,
            UserInput::Mention { name, path }
                if name == "Google Calendar" && path == "plugin://google-calendar@debug"
        )
    }));
    assert!(!items.iter().any(|item| {
        matches!(
            item,
            UserInput::Mention { path, .. } if path == "app://google_calendar"
        )
    }));
}

#[tokio::test]
async fn iterated_reprompt_reuses_resolution_context_on_next_submit() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    chat.set_skills(Some(vec![sample_skill()]));
    chat.reprompt_config.enabled = true;
    chat.reprompt_config.min_length = 1;
    chat.reprompt_original_message = Some(UserMessage::from("original prompt"));
    chat.reprompt_resolution_context = Some(sample_resolution_context());
    chat.reprompt_overlay = Some(crate::reprompt::RepromptOverlay::new(
        crate::reprompt::RepromptOverlayData::new(
            "original prompt".to_string(),
            crate::reprompt::RepromptResult {
                refined_prompt: "Run $repo:linter on @src/auth/token.rs".to_string(),
                applied_rules: vec![],
                reasoning: "Resolved the file and skill reference".to_string(),
                task_type: crate::reprompt::TaskType::Analysis,
                was_substantive_change: true,
                tips: vec![],
            },
            Duration::ZERO,
        ),
    ));

    assert!(
        chat.handle_reprompt_overlay_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE,))
    );
    assert_eq!(
        chat.bottom_pane.composer_text(),
        "Run $repo:linter on @src/auth/token.rs"
    );
    assert!(chat.reprompt_resolution_context.is_some());

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let Op::UserTurn { items, .. } = next_submit_op(&mut op_rx) else {
        panic!("expected Op::UserTurn");
    };
    assert!(items.iter().any(|item| {
        matches!(
            item,
            UserInput::Skill { name, .. } if name == "repo:linter"
        )
    }));
    assert!(items.iter().any(|item| {
        matches!(
            item,
            UserInput::Mention { name, path }
                if name == "src/auth/token.rs" && path == "/tmp/project/src/auth/token.rs"
        )
    }));
    assert!(chat.reprompt_resolution_context.is_none());
}
