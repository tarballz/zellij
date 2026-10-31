//! Turns the live `SessionInfo` into the `SessionSnapshot` the prompt is built from.

use std::collections::HashMap;
use zellij_utils::data::{PaneInfo, SessionInfo, TabInfo};

use super::prompt::{PaneSnapshot, SessionSnapshot, TabSnapshot};

/// The tabs in display order, each with the panes that say something about the work being
/// done: plugin panes (tab bar, status bar) and suppressed panes don't.
fn tabs_with_terminal_panes(info: &SessionInfo) -> Vec<(&TabInfo, Vec<&PaneInfo>)> {
    let mut tabs: Vec<&TabInfo> = info.tabs.iter().collect();
    tabs.sort_by_key(|tab| tab.position);
    tabs.into_iter()
        .map(|tab| {
            let panes = info
                .panes
                .panes
                .get(&tab.position)
                .map(|panes| {
                    panes
                        .iter()
                        .filter(|pane| !pane.is_plugin && !pane.is_suppressed)
                        .collect()
                })
                .unwrap_or_default();
            (tab, panes)
        })
        .collect()
}

/// Ids of the terminal panes whose screen text is worth reading.
pub fn terminal_pane_ids(info: &SessionInfo) -> Vec<u32> {
    tabs_with_terminal_panes(info)
        .into_iter()
        .flat_map(|(_, panes)| panes.into_iter().map(|pane| pane.id))
        .collect()
}

/// `screens` maps a terminal pane id to its recent screen text.
pub fn snapshot_from_session_info(
    info: &SessionInfo,
    screens: &HashMap<u32, String>,
) -> SessionSnapshot {
    SessionSnapshot {
        session_name: info.name.clone(),
        tabs: tabs_with_terminal_panes(info)
            .into_iter()
            .map(|(tab, panes)| TabSnapshot {
                name: tab.name.clone(),
                panes: panes
                    .into_iter()
                    .map(|pane| PaneSnapshot {
                        title: pane.title.clone(),
                        command: pane.terminal_command.clone(),
                        cwd: None, // `PaneInfo` doesn't carry a working directory
                        screen_text: screens.get(&pane.id).cloned(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// The visible text of a pane, without the blank rows below the last line of output. A pane's
/// viewport is as tall as the pane, so a shell prompt near the top leaves it mostly empty.
pub fn screen_text_from_viewport(viewport: &[String]) -> String {
    let mut rows: Vec<&str> = viewport.iter().map(|row| row.trim_end()).collect();
    while rows.last().map_or(false, |row| row.is_empty()) {
        rows.pop();
    }
    rows.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use zellij_utils::data::{PaneInfo, PaneManifest, TabInfo};

    fn pane(id: u32, title: &str) -> PaneInfo {
        PaneInfo {
            id,
            title: title.to_owned(),
            ..Default::default()
        }
    }

    fn tab(position: usize, name: &str) -> TabInfo {
        TabInfo {
            position,
            name: name.to_owned(),
            ..Default::default()
        }
    }

    fn session(tabs: Vec<TabInfo>, panes: Vec<(usize, Vec<PaneInfo>)>) -> SessionInfo {
        SessionInfo {
            name: "rusty-owl".to_owned(),
            tabs,
            panes: PaneManifest {
                panes: panes.into_iter().collect(),
            },
            ..Default::default()
        }
    }

    #[test]
    fn plugin_and_suppressed_panes_are_skipped() {
        let info = session(
            vec![tab(0, "build")],
            vec![(
                0,
                vec![
                    pane(1, "shell"),
                    PaneInfo {
                        is_plugin: true,
                        ..pane(2, "tab-bar")
                    },
                    PaneInfo {
                        is_suppressed: true,
                        ..pane(3, "hidden")
                    },
                ],
            )],
        );
        assert_eq!(terminal_pane_ids(&info), vec![1]);
        let snapshot = snapshot_from_session_info(&info, &HashMap::new());
        let titles: Vec<_> = snapshot.tabs[0]
            .panes
            .iter()
            .map(|p| p.title.as_str())
            .collect();
        assert_eq!(titles, vec!["shell"]);
    }

    #[test]
    fn tabs_are_ordered_by_position_and_keep_their_names() {
        let info = session(
            vec![tab(1, "second"), tab(0, "first")],
            vec![(0, vec![pane(1, "a")]), (1, vec![pane(2, "b")])],
        );
        let snapshot = snapshot_from_session_info(&info, &HashMap::new());
        assert_eq!(snapshot.session_name, "rusty-owl");
        let names: Vec<_> = snapshot.tabs.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["first", "second"]);
        assert_eq!(snapshot.tabs[0].panes[0].title, "a");
        assert_eq!(snapshot.tabs[1].panes[0].title, "b");
        assert_eq!(terminal_pane_ids(&info), vec![1, 2]);
    }

    #[test]
    fn panes_carry_their_command_and_screen_text() {
        let info = session(
            vec![tab(0, "build")],
            vec![(
                0,
                vec![PaneInfo {
                    terminal_command: Some("cargo watch".to_owned()),
                    ..pane(7, "watcher")
                }],
            )],
        );
        let screens = HashMap::from([(7, "Compiling zellij".to_owned())]);
        let snapshot = snapshot_from_session_info(&info, &screens);
        let pane = &snapshot.tabs[0].panes[0];
        assert_eq!(pane.title, "watcher");
        assert_eq!(pane.command.as_deref(), Some("cargo watch"));
        assert_eq!(pane.screen_text.as_deref(), Some("Compiling zellij"));
        // `PaneInfo` doesn't carry a working directory
        assert_eq!(pane.cwd, None);
    }

    #[test]
    fn panes_without_screen_text_have_none() {
        let info = session(vec![tab(0, "t")], vec![(0, vec![pane(1, "a")])]);
        let snapshot = snapshot_from_session_info(&info, &HashMap::new());
        assert_eq!(snapshot.tabs[0].panes[0].screen_text, None);
    }

    #[test]
    fn a_tab_without_panes_is_still_listed() {
        let info = session(vec![tab(0, "empty")], vec![]);
        let snapshot = snapshot_from_session_info(&info, &HashMap::new());
        assert_eq!(snapshot.tabs.len(), 1);
        assert!(snapshot.tabs[0].panes.is_empty());
    }

    fn rows(rows: &[&str]) -> Vec<String> {
        rows.iter().map(|row| row.to_string()).collect()
    }

    #[test]
    fn blank_rows_below_the_output_are_dropped() {
        assert_eq!(
            screen_text_from_viewport(&rows(&["$ cargo build", "Compiling", "", "  ", ""])),
            "$ cargo build\nCompiling"
        );
    }

    #[test]
    fn blank_rows_inside_the_output_are_kept() {
        assert_eq!(
            screen_text_from_viewport(&rows(&["one", "", "two", ""])),
            "one\n\ntwo"
        );
    }

    #[test]
    fn an_empty_viewport_has_no_text() {
        assert_eq!(screen_text_from_viewport(&[]), "");
        assert_eq!(screen_text_from_viewport(&rows(&["", " "])), "");
    }

    #[test]
    fn trailing_whitespace_on_rows_is_trimmed() {
        assert_eq!(
            screen_text_from_viewport(&rows(&["abc   ", "def "])),
            "abc\ndef"
        );
    }
}
