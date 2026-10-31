//! The background thread that keeps the session's blurb up to date. All of its decisions are
//! made by code with its own tests (`snapshot`, `refresh`, `provider`); this only gathers the
//! inputs on a timer and stores the result.

use std::collections::HashMap;
use std::thread;
use std::time::Duration;

use zellij_utils::channels::SenderWithContext;
use zellij_utils::consts::session_info_folder_for_session;
use zellij_utils::data::{ClientId, PaneScrollbackResponse};
use zellij_utils::input::ai_summary::{AiProvider, AiSummaryConfig};
use zellij_utils::session_blurb::write_blurb;

use super::fm::run_fm;
use super::http::send_request;
use super::provider::{parse_response, ProviderRequest};
use super::redact::{redact, truncate_bytes};
use super::refresh::{refresh_blurb, RefreshOutcome};
use super::snapshot::{screen_text_from_viewport, snapshot_from_session_info, terminal_pane_ids};
use crate::background_jobs::session_scan_state;
use crate::panes::PaneId;
use crate::screen::ScreenInstruction;

/// Gives a new session time to open its panes before the first blurb is written.
const FIRST_REFRESH_DELAY: Duration = Duration::from_secs(20);
const PANE_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// However short the configured interval, never poll the provider faster than this.
const MIN_INTERVAL_SECS: u64 = 30;
/// Pane contents are requested on behalf of this client; it only matters for selection state.
const READ_AS_CLIENT: ClientId = 1;

/// The API key to use, or `None` when blurbs can't run: disabled, or the key's variable is
/// unset or blank. `lookup` reads an environment variable by name (a parameter so tests don't
/// have to touch the real environment). The on-device provider has no key, so it gets an empty
/// one and never consults `lookup`.
fn resolve_api_key(
    config: &AiSummaryConfig,
    lookup: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    if !config.is_enabled() {
        return None;
    }
    if !config.needs_api_key() {
        return Some(String::new());
    }
    let api_key_env = config.effective_api_key_env()?;
    let key = lookup(&api_key_env)?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    Some(key.to_owned())
}

/// Starts the thread, unless blurbs are disabled or the API key variable isn't set (the key is
/// read once, here, and never logged).
pub fn spawn_ai_summary_service(
    config: AiSummaryConfig,
    session_id: String,
    to_screen: SenderWithContext<ScreenInstruction>,
) -> Option<thread::JoinHandle<()>> {
    if !config.is_enabled() {
        return None;
    }
    let Some(api_key) = resolve_api_key(&config, |name| std::env::var(name).ok()) else {
        log::info!(
            "ai_summary: ${} is not set, session blurbs are disabled",
            config.effective_api_key_env().unwrap_or_default()
        );
        return None;
    };
    log::info!(
        "ai_summary: enabled ({:?}, model {}, every {}s, screen text {})",
        config.provider,
        config.effective_model().unwrap_or_default(),
        config.effective_interval_secs().max(MIN_INTERVAL_SECS),
        if config.effective_include_screen_text() {
            "on"
        } else {
            "off"
        },
    );
    thread::Builder::new()
        .name("ai_summary".to_owned())
        .spawn(move || run(config, api_key, session_id, to_screen))
        .ok()
}

/// Sends `request` over the provider's transport, with failures logged: `summarize`
/// deliberately swallows them. The on-device provider runs `fm` with the request body as its
/// prompt; every other provider makes an HTTP call. A response that can't be used is logged as
/// a short, redacted snippet to make it diagnosable.
fn send_and_log(
    provider: AiProvider,
    model: &str,
    request: &ProviderRequest,
) -> Result<String, String> {
    let result = match provider {
        AiProvider::Apple => run_fm(model, &request.body),
        AiProvider::Claude | AiProvider::DeepSeek => send_request(request),
    };
    match &result {
        Err(e) => log::warn!("ai_summary: request failed: {}", e),
        Ok(body) if parse_response(provider, body).is_none() => log::warn!(
            "ai_summary: unusable response: {}",
            truncate_bytes(&redact(body), 400)
        ),
        Ok(_) => {},
    }
    result
}

fn run(
    config: AiSummaryConfig,
    api_key: String,
    session_id: String,
    to_screen: SenderWithContext<ScreenInstruction>,
) {
    let interval = Duration::from_secs(config.effective_interval_secs().max(MIN_INTERVAL_SECS));
    let mut last_fingerprint = None;
    thread::sleep(FIRST_REFRESH_DELAY);
    loop {
        refresh_once(
            &config,
            &api_key,
            &session_id,
            &to_screen,
            &mut last_fingerprint,
        );
        thread::sleep(interval);
    }
}

fn refresh_once(
    config: &AiSummaryConfig,
    api_key: &str,
    session_id: &str,
    to_screen: &SenderWithContext<ScreenInstruction>,
    last_fingerprint: &mut Option<u64>,
) {
    let Some(session_info) = session_scan_state().and_then(|state| {
        state
            .current_session_info
            .lock()
            .ok()
            .map(|info| info.clone())
    }) else {
        return;
    };
    if session_info.tabs.is_empty() {
        return;
    }
    let screens = if config.effective_include_screen_text() {
        read_screens(to_screen, &terminal_pane_ids(&session_info))
    } else {
        HashMap::new()
    };
    let snapshot = snapshot_from_session_info(&session_info, &screens);
    let Some(provider) = config.provider else {
        return;
    };
    let Some(model) = config.effective_model() else {
        return;
    };
    let send = |request: &ProviderRequest| send_and_log(provider, &model, request);
    match refresh_blurb(config, api_key, &snapshot, *last_fingerprint, send) {
        RefreshOutcome::Updated { blurb, fingerprint } => {
            match write_blurb(&session_info_folder_for_session(session_id), &blurb) {
                Ok(()) => {
                    log::info!("ai_summary: updated the session blurb");
                    *last_fingerprint = Some(fingerprint);
                },
                Err(e) => log::warn!("ai_summary: could not store the session blurb: {}", e),
            }
        },
        RefreshOutcome::Unchanged => log::debug!("ai_summary: nothing changed"),
        RefreshOutcome::Failed => log::warn!("ai_summary: could not refresh the session blurb"),
    }
}

/// The visible text of each given terminal pane. Panes that don't answer in time are left out.
fn read_screens(
    to_screen: &SenderWithContext<ScreenInstruction>,
    pane_ids: &[u32],
) -> HashMap<u32, String> {
    let mut screens = HashMap::new();
    for &pane_id in pane_ids {
        let (response_sender, response_receiver) = crossbeam::channel::bounded(1);
        let request = ScreenInstruction::GetPaneScrollback {
            pane_id: PaneId::Terminal(pane_id),
            client_id: READ_AS_CLIENT,
            get_full_scrollback: false,
            response_channel: response_sender,
        };
        if to_screen.send(request).is_err() {
            break; // the screen thread is gone: the session is shutting down
        }
        if let Ok(PaneScrollbackResponse::Ok(contents)) =
            response_receiver.recv_timeout(PANE_READ_TIMEOUT)
        {
            screens.insert(pane_id, screen_text_from_viewport(&contents.viewport));
        }
    }
    screens
}

#[cfg(test)]
mod tests {
    use super::*;
    use zellij_utils::input::ai_summary::AiProvider;

    fn config_for(provider: AiProvider) -> AiSummaryConfig {
        AiSummaryConfig {
            provider: Some(provider),
            ..Default::default()
        }
    }

    #[test]
    fn the_on_device_provider_starts_without_any_key() {
        // no environment variable is consulted at all
        let key = resolve_api_key(&config_for(AiProvider::Apple), |_| None);
        assert_eq!(key.as_deref(), Some(""));
    }

    #[test]
    fn a_network_provider_uses_the_key_from_its_variable() {
        let key = resolve_api_key(&config_for(AiProvider::Claude), |name| {
            assert_eq!(name, "ANTHROPIC_API_KEY");
            Some("  sk-test  ".to_owned())
        });
        assert_eq!(key.as_deref(), Some("sk-test"));
    }

    #[test]
    fn a_configured_variable_name_wins_over_the_default() {
        let mut config = config_for(AiProvider::DeepSeek);
        config.api_key_env = Some("MY_KEY".to_owned());
        let key = resolve_api_key(&config, |name| {
            assert_eq!(name, "MY_KEY");
            Some("k".to_owned())
        });
        assert_eq!(key.as_deref(), Some("k"));
    }

    #[test]
    fn a_network_provider_stays_off_without_a_usable_key() {
        let claude = config_for(AiProvider::Claude);
        assert_eq!(resolve_api_key(&claude, |_| None), None);
        assert_eq!(resolve_api_key(&claude, |_| Some("  \n".to_owned())), None);
    }

    #[test]
    fn a_disabled_config_has_no_key() {
        let key = resolve_api_key(&AiSummaryConfig::default(), |_| Some("k".to_owned()));
        assert_eq!(key, None);
    }
}
