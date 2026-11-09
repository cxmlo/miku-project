use crate::feed::*;
use crate::prelude::*;

fn test_feed_gatekeeper_engine() -> GatekeeperEngine {
    let profile = ProjectProfile::new("miku-feed-test", "C:/workspace/miku", 30_000)
        .with_rule(
            Capability::Network,
            "https://api.github.com/*",
            PolicyEffect::Allow,
            false,
        )
        .with_rule(
            Capability::Network,
            "https://blog.rust-lang.org/*",
            PolicyEffect::Allow,
            false,
        )
        .with_rule(
            Capability::Browser,
            "https://github.com/*",
            PolicyEffect::Allow,
            false,
        )
        .with_rule(
            Capability::Browser,
            "https://blog.rust-lang.org/*",
            PolicyEffect::Allow,
            false,
        )
        .with_rule(
            Capability::Browser,
            "https://untrusted.example.com/*",
            PolicyEffect::Deny,
            true,
        );
    GatekeeperEngine::new(profile).expect("valid gatekeeper engine")
}

fn test_overview_scene_controller() -> OverviewSceneController {
    let avatar_dir = resolve_avatar_assets_dir();
    OverviewSceneController::new(&avatar_dir, false).expect("init overview scene controller")
}

// ============================================================================
// LEDGER-3.11-1: 304 Not Modified conserva el contenido en caché y evita
//                reprocesar y despertar la UI.
// ============================================================================
#[test]
fn test_http_304_not_modified_preserves_cache_without_reparse_or_ui_wake() {
    let mut gk = test_feed_gatekeeper_engine();
    let mut overview = test_overview_scene_controller();
    let mut bridge = WebView2IpcBridge::new().unwrap();
    let mut transport = MockFeedTransport::new();
    let mut controller = IslandTechFeedController::new();

    controller.register_source(TechFeedSourceConfig::new(
        "gh-prs",
        "GitHub Pull Requests",
        TechFeedSourceKind::PullRequests,
        "https://api.github.com/repos/miku-org/miku/pulls",
        true,
        2_000,
    ));

    // 1. Initial 200 OK response populates cache and wakes UI once
    transport.enqueue_response(
        "gh-prs",
        Ok(FeedHttpResponse::ok_200(
            r#"[{"id":101,"title":"feat(skills): transactional hot-reload","html_url":"https://github.com/miku-org/miku/pull/101","summary":"Ready for review"}]"#,
            Some("\"etag-pr-v1\""),
            Some("Wed, 09 Nov 2026 01:00:00 GMT"),
            Some(4999),
            Some(90_000),
        )),
    );

    let first_sync = controller
        .sync_all_sources(&mut transport, &mut gk, &mut overview, &mut bridge, 1_000)
        .unwrap();
    assert_eq!(first_sync.http_200_count, 1);
    assert_eq!(first_sync.http_304_count, 0);
    assert_eq!(first_sync.reparse_delta, 1);
    assert_eq!(first_sync.ui_wake_delta, 1);
    assert_eq!(controller.merged_items().len(), 1);
    let dom_updates_after_first = overview.dom_updates_count();

    // 2. Subsequent 304 Not Modified preserves cached items, performs 0 reparses, and 0 UI wakes
    transport.enqueue_response(
        "gh-prs",
        Ok(FeedHttpResponse::not_modified_304(
            Some("\"etag-pr-v1\""),
            Some("Wed, 09 Nov 2026 01:00:00 GMT"),
            Some(4998),
            Some(90_000),
        )),
    );

    let second_sync = controller
        .sync_all_sources(&mut transport, &mut gk, &mut overview, &mut bridge, 2_000)
        .unwrap();
    assert_eq!(second_sync.http_200_count, 0);
    assert_eq!(second_sync.http_304_count, 1);
    assert_eq!(second_sync.reparse_delta, 0);
    assert_eq!(second_sync.ui_wake_delta, 0);
    assert_eq!(controller.reparse_count(), 1);
    assert_eq!(controller.ui_wake_count(), 1);
    assert_eq!(overview.dom_updates_count(), dom_updates_after_first);
    assert_eq!(controller.merged_items().len(), 1);
    assert_eq!(
        controller.merged_items()[0].title,
        "feat(skills): transactional hot-reload"
    );
}

// ============================================================================
// LEDGER-3.11-2: Una fuente RSS con error o timeout no impide cargar y mostrar
//                el resto de fuentes.
// ============================================================================
#[test]
fn test_rss_source_timeout_or_error_isolates_failure_and_renders_healthy_sources() {
    let mut gk = test_feed_gatekeeper_engine();
    let mut overview = test_overview_scene_controller();
    let mut bridge = WebView2IpcBridge::new().unwrap();
    let mut transport = MockFeedTransport::new();
    let mut controller = IslandTechFeedController::new();

    controller.register_source(TechFeedSourceConfig::new(
        "rust-blog-rss",
        "Rust Blog RSS",
        TechFeedSourceKind::RssFeed,
        "https://blog.rust-lang.org/feed.xml",
        false,
        1_500,
    ));
    controller.register_source(TechFeedSourceConfig::new(
        "gh-ci",
        "GitHub CI Runs",
        TechFeedSourceKind::CiStatus,
        "https://api.github.com/repos/miku-org/miku/actions/runs",
        true,
        1_500,
    ));

    // RSS times out, while GitHub CI returns 200 OK
    transport.enqueue_response(
        "rust-blog-rss",
        Err(FeedTransportError::Timeout {
            source_id: "rust-blog-rss".to_string(),
            timeout_ms: 1_500,
        }),
    );
    transport.enqueue_response(
        "gh-ci",
        Ok(FeedHttpResponse::ok_200(
            r#"[{"id":501,"name":"CI Workspace Windows 11","html_url":"https://github.com/miku-org/miku/actions/runs/501","conclusion":"success"}]"#,
            Some("\"etag-ci-1\""),
            None,
            Some(4990),
            Some(90_000),
        )),
    );

    let summary = controller
        .sync_all_sources(&mut transport, &mut gk, &mut overview, &mut bridge, 1_000)
        .unwrap();
    assert_eq!(summary.source_errors, 1);
    assert_eq!(summary.http_200_count, 1);
    assert_eq!(summary.total_items, 1);
    assert_eq!(
        controller.merged_items()[0].title,
        "CI Workspace Windows 11"
    );
    assert_eq!(controller.diagnostics().len(), 1);
    assert!(controller.diagnostics()[0].contains("timed out"));
}

// ============================================================================
// LEDGER-3.11-3: La caché condicional envía If-None-Match y/o If-Modified-Since
//                con los validadores almacenados y los actualiza tras 200 OK.
// ============================================================================
#[test]
fn test_conditional_cache_sends_if_none_match_and_if_modified_since_and_updates_validators() {
    let mut gk = test_feed_gatekeeper_engine();
    let mut overview = test_overview_scene_controller();
    let mut bridge = WebView2IpcBridge::new().unwrap();
    let mut transport = MockFeedTransport::new();
    let mut controller = IslandTechFeedController::new();

    controller.register_source(TechFeedSourceConfig::new(
        "rust-rss",
        "Rust Blog RSS",
        TechFeedSourceKind::RssFeed,
        "https://blog.rust-lang.org/feed.xml",
        false,
        2_000,
    ));

    let rss_v1 = r#"<rss version="2.0"><channel><item><guid>post-1</guid><title>Rust 1.85 Released</title><link>https://blog.rust-lang.org/2026/11/01/Rust-1.85.html</link><description>Async closures and edition updates</description></item></channel></rss>"#;
    let rss_v2 = r#"<rss version="2.0"><channel><item><guid>post-2</guid><title>Rust 1.86 Released</title><link>https://blog.rust-lang.org/2026/11/09/Rust-1.86.html</link><description>Faster compiler and Cargo improvements</description></item></channel></rss>"#;

    transport.enqueue_response(
        "rust-rss",
        Ok(FeedHttpResponse::ok_200(
            rss_v1,
            Some("W/\"rss-etag-1\""),
            Some("Sun, 01 Nov 2026 12:00:00 GMT"),
            None,
            None,
        )),
    );
    transport.enqueue_response(
        "rust-rss",
        Ok(FeedHttpResponse::ok_200(
            rss_v2,
            Some("W/\"rss-etag-2\""),
            Some("Mon, 09 Nov 2026 12:00:00 GMT"),
            None,
            None,
        )),
    );

    // First sync: no validators stored yet
    controller
        .sync_all_sources(&mut transport, &mut gk, &mut overview, &mut bridge, 1_000)
        .unwrap();
    let val_after_1 = controller.validators_for("rust-rss").unwrap();
    assert_eq!(val_after_1.etag.as_deref(), Some("W/\"rss-etag-1\""));
    assert_eq!(
        val_after_1.last_modified.as_deref(),
        Some("Sun, 01 Nov 2026 12:00:00 GMT")
    );

    // Second sync: sends If-None-Match and If-Modified-Since from first 200 OK, then updates to v2
    controller
        .sync_all_sources(&mut transport, &mut gk, &mut overview, &mut bridge, 2_000)
        .unwrap();

    let reqs = transport.requests_for_source("rust-rss");
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].if_none_match, None);
    assert_eq!(reqs[0].if_modified_since, None);
    assert_eq!(reqs[1].if_none_match.as_deref(), Some("W/\"rss-etag-1\""));
    assert_eq!(
        reqs[1].if_modified_since.as_deref(),
        Some("Sun, 01 Nov 2026 12:00:00 GMT")
    );

    let val_after_2 = controller.validators_for("rust-rss").unwrap();
    assert_eq!(val_after_2.etag.as_deref(), Some("W/\"rss-etag-2\""));
    assert_eq!(
        val_after_2.last_modified.as_deref(),
        Some("Mon, 09 Nov 2026 12:00:00 GMT")
    );
}

// ============================================================================
// LEDGER-3.11-4: La sincronización se pausa en modo gaming y sin red, y al
//                reanudarse no pierde ni duplica elementos.
// ============================================================================
#[test]
fn test_sync_pauses_in_gaming_mode_and_offline_and_resumes_without_loss_or_duplicates() {
    let mut gk = test_feed_gatekeeper_engine();
    let mut overview = test_overview_scene_controller();
    let mut bridge = WebView2IpcBridge::new().unwrap();
    let mut transport = MockFeedTransport::new();
    let mut controller = IslandTechFeedController::new();

    controller.register_source(TechFeedSourceConfig::new(
        "gh-radar",
        "GitHub Repo Radar",
        TechFeedSourceKind::RepoRadar,
        "https://api.github.com/repos/miku-org/miku/releases",
        true,
        2_000,
    ));

    let radar_payload = r#"[
        {"id":"rel-1","name":"v0.3.10","html_url":"https://github.com/miku-org/miku/releases/tag/v0.3.10","description":"Skills hot-reload"},
        {"id":"rel-2","name":"v0.3.11","html_url":"https://github.com/miku-org/miku/releases/tag/v0.3.11","description":"Tech feed ETag"}
    ]"#;

    transport.enqueue_response(
        "gh-radar",
        Ok(FeedHttpResponse::ok_200(
            radar_payload,
            Some("\"etag-radar-1\""),
            None,
            Some(60),
            Some(90_000),
        )),
    );
    transport.enqueue_response(
        "gh-radar",
        Ok(FeedHttpResponse::ok_200(
            radar_payload,
            Some("\"etag-radar-2\""),
            None,
            Some(59),
            Some(90_000),
        )),
    );

    // 1. Initial online sync loads 2 items
    let s1 = controller
        .sync_all_sources(&mut transport, &mut gk, &mut overview, &mut bridge, 1_000)
        .unwrap();
    assert_eq!(s1.paused_reason, None);
    assert_eq!(controller.merged_items().len(), 2);

    // 2. Enter Gaming Mode: sync pauses, 0 network requests sent, 2 items preserved
    controller.set_gaming_mode_active(true);
    let s_gaming = controller
        .sync_all_sources(&mut transport, &mut gk, &mut overview, &mut bridge, 2_000)
        .unwrap();
    assert_eq!(
        s_gaming.paused_reason,
        Some(FeedPauseReason::GamingModeActive)
    );
    assert_eq!(transport.recorded_requests().len(), 1);
    assert_eq!(controller.merged_items().len(), 2);

    // 3. Exit Gaming Mode, go Offline: sync pauses for Offline, still 0 new network requests
    controller.set_gaming_mode_active(false);
    controller.set_network_online(false);
    let s_offline = controller
        .sync_all_sources(&mut transport, &mut gk, &mut overview, &mut bridge, 3_000)
        .unwrap();
    assert_eq!(s_offline.paused_reason, Some(FeedPauseReason::Offline));
    assert_eq!(transport.recorded_requests().len(), 1);
    assert_eq!(controller.merged_items().len(), 2);

    // 4. Resume online: sync executes without losing or duplicating items
    controller.set_network_online(true);
    let s_resumed = controller
        .sync_all_sources(&mut transport, &mut gk, &mut overview, &mut bridge, 4_000)
        .unwrap();
    assert_eq!(s_resumed.paused_reason, None);
    assert_eq!(transport.recorded_requests().len(), 2);
    assert_eq!(controller.merged_items().len(), 2);
}

// ============================================================================
// LEDGER-3.11-5: El cliente de GitHub respeta X-RateLimit-Remaining y la caché
//                condicional.
// ============================================================================
#[test]
fn test_github_client_respects_x_ratelimit_remaining_and_conditional_cache() {
    let mut gk = test_feed_gatekeeper_engine();
    let mut overview = test_overview_scene_controller();
    let mut bridge = WebView2IpcBridge::new().unwrap();
    let mut transport = MockFeedTransport::new();
    let mut controller = IslandTechFeedController::new();

    controller.register_source(TechFeedSourceConfig::new(
        "gh-prs",
        "GitHub PRs",
        TechFeedSourceKind::PullRequests,
        "https://api.github.com/repos/miku-org/miku/pulls",
        true,
        2_000,
    ));

    // First response returns X-RateLimit-Remaining = 0 with reset at t = 60_000 ms
    transport.enqueue_response(
        "gh-prs",
        Ok(FeedHttpResponse::ok_200(
            r#"[{"id":42,"title":"fix(gatekeeper): strict browser policy","html_url":"https://github.com/miku-org/miku/pull/42","summary":"Approved"}]"#,
            Some("\"etag-gh-42\""),
            None,
            Some(0),
            Some(60_000),
        )),
    );
    // Response queued for after rate-limit reset (304 Not Modified)
    transport.enqueue_response(
        "gh-prs",
        Ok(FeedHttpResponse::not_modified_304(
            Some("\"etag-gh-42\""),
            None,
            Some(4999),
            Some(120_000),
        )),
    );

    let s1 = controller
        .sync_all_sources(&mut transport, &mut gk, &mut overview, &mut bridge, 10_000)
        .unwrap();
    assert_eq!(s1.http_200_count, 1);
    assert_eq!(transport.recorded_requests().len(), 1);

    // At t = 30_000 ms (< 60_000 ms reset), client skips network fetch due to X-RateLimit-Remaining == 0
    let s2 = controller
        .sync_all_sources(&mut transport, &mut gk, &mut overview, &mut bridge, 30_000)
        .unwrap();
    assert_eq!(s2.rate_limited_skips, 1);
    assert_eq!(s2.sources_checked, 0);
    assert_eq!(transport.recorded_requests().len(), 1);
    assert_eq!(controller.merged_items().len(), 1);

    // At t = 60_001 ms (>= 60_000 ms reset), client resumes with If-None-Match conditional header
    let s3 = controller
        .sync_all_sources(&mut transport, &mut gk, &mut overview, &mut bridge, 60_001)
        .unwrap();
    assert_eq!(s3.rate_limited_skips, 0);
    assert_eq!(s3.http_304_count, 1);
    assert_eq!(transport.recorded_requests().len(), 2);
    assert_eq!(
        transport.recorded_requests()[1].if_none_match.as_deref(),
        Some("\"etag-gh-42\"")
    );
}

// ============================================================================
// LEDGER-3.11-6: Los elementos del feed se renderizan en la Island y la acción
//                de abrir enlace delega al mecanismo autorizado hacia Zen Browser.
// ============================================================================
#[test]
fn test_island_renders_feed_items_safely_and_opens_links_in_zen_browser_via_gatekeeper() {
    let mut gk = test_feed_gatekeeper_engine();
    let mut overview = test_overview_scene_controller();
    let mut bridge = WebView2IpcBridge::new().unwrap();
    let mut transport = MockFeedTransport::new();
    let mut launcher = MockZenBrowserLauncher::new();
    let mut controller = IslandTechFeedController::new();

    controller.register_source(TechFeedSourceConfig::new(
        "gh-prs",
        "GitHub PRs",
        TechFeedSourceKind::PullRequests,
        "https://api.github.com/repos/miku-org/miku/pulls",
        true,
        2_000,
    ));

    transport.enqueue_response(
        "gh-prs",
        Ok(FeedHttpResponse::ok_200(
            r#"[
                {"id":77,"title":"<script>alert(1)</script>Secure Island Feed","html_url":"https://github.com/miku-org/miku/pull/77","summary":"ETag caching"},
                {"id":78,"title":"Blocked External Link","html_url":"https://untrusted.example.com/phishing","summary":"Should be denied"}
            ]"#,
            Some("\"etag-77\""),
            None,
            Some(100),
            Some(90_000),
        )),
    );

    controller
        .sync_all_sources(&mut transport, &mut gk, &mut overview, &mut bridge, 1_000)
        .unwrap();

    // Verify textContent-safe rendering in Island
    let renders = controller
        .render_into_island(&mut overview, &mut bridge)
        .unwrap();
    assert_eq!(renders.len(), 3);
    if let WebViewIpcPayloadV1::DomSafeTextRender {
        ref safe_text,
        csp_enforced,
        ..
    } = renders[1]
    {
        assert!(csp_enforced);
        assert!(!safe_text.contains("<script>"));
        assert!(safe_text.contains("Secure Island Feed"));
    } else {
        panic!("expected DomSafeTextRender payload");
    }

    // Clicking authorized item opens link in Zen Browser
    let allowed_id = &controller.merged_items()[0].item_id.clone();
    let open_ok = controller
        .open_item_in_zen_browser(allowed_id, &mut gk, &mut launcher, 1_100)
        .unwrap();
    assert_eq!(open_ok.url, "https://github.com/miku-org/miku/pull/77");
    assert_eq!(
        open_ok.command_args,
        vec![
            "zen.exe".to_string(),
            "--new-tab".to_string(),
            "https://github.com/miku-org/miku/pull/77".to_string()
        ]
    );
    assert_eq!(launcher.opened_urls().len(), 1);

    // Clicking unauthorized domain item is blocked by Gatekeeper and never invokes Zen Browser
    let denied_id = &controller.merged_items()[1].item_id.clone();
    let open_err = controller.open_item_in_zen_browser(denied_id, &mut gk, &mut launcher, 1_200);
    assert!(open_err.is_err());
    assert_eq!(launcher.opened_urls().len(), 1);
}
