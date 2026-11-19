use std::collections::HashMap;

use crate::api::schema::AgentStatus;
use crate::protocol::{ClientShellAgent, ClientShellSnapshot};

/// One pane in a workspace's status strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PaneDot {
    pub pane_id: String,
    pub status: Option<AgentStatus>,
    pub winner: bool,
}

/// The agent that owns a space row's status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WinnerAgent {
    pub pane_id: String,
    pub agent: Option<String>,
    pub label: String,
    pub status: AgentStatus,
    pub text: Option<String>,
}

/// Per-workspace agent facts used by the Spaces sidebar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceAgentSummary {
    pub winner: Option<WinnerAgent>,
    pub pane_dots: Vec<PaneDot>,
    pub other_tabs_attention: Option<AgentStatus>,
}

pub(crate) fn workspace_summaries(
    snapshot: &ClientShellSnapshot,
) -> HashMap<String, WorkspaceAgentSummary> {
    let mut summaries = snapshot
        .workspaces
        .iter()
        .map(|workspace| {
            (
                workspace.workspace_id.clone(),
                WorkspaceAgentSummary::default(),
            )
        })
        .collect::<HashMap<_, _>>();
    let active_tabs = snapshot
        .workspaces
        .iter()
        .map(|workspace| {
            (
                workspace.workspace_id.as_str(),
                workspace.active_tab_id.as_str(),
            )
        })
        .collect::<HashMap<_, _>>();
    let pane_order = snapshot
        .panes
        .iter()
        .enumerate()
        .map(|(index, pane)| (pane.pane_id.as_str(), index))
        .collect::<HashMap<_, _>>();

    let mut winners = HashMap::<&str, Candidate<'_>>::new();
    for agent in &snapshot.agents {
        let Some(active_tab) = active_tabs.get(agent.workspace_id.as_str()).copied() else {
            continue;
        };
        let Some(summary) = summaries.get_mut(agent.workspace_id.as_str()) else {
            continue;
        };
        let in_active_tab = agent.tab_id == active_tab;
        if !in_active_tab
            && summary.other_tabs_attention.is_none_or(|current| {
                super::status_priority(agent.agent_status) > super::status_priority(current)
            })
        {
            summary.other_tabs_attention = Some(agent.agent_status);
        }
        let candidate = Candidate {
            agent,
            rank: super::status_priority(agent.agent_status),
            focused: agent.focused,
            in_active_tab,
            pane_index: pane_order
                .get(agent.pane_id.as_str())
                .copied()
                .unwrap_or(usize::MAX),
        };
        match winners.get(agent.workspace_id.as_str()) {
            Some(current) if !candidate.outranks(current) => {}
            _ => {
                winners.insert(agent.workspace_id.as_str(), candidate);
            }
        }
    }

    for (workspace_id, candidate) in winners {
        let label = candidate.label(snapshot);
        let Some(summary) = summaries.get_mut(workspace_id) else {
            continue;
        };
        summary.winner = Some(WinnerAgent {
            pane_id: candidate.agent.pane_id.clone(),
            agent: candidate.agent.agent.clone(),
            label,
            status: candidate.agent.agent_status,
            text: crate::api::schema::common::normalize_status_message_preview(
                candidate.agent.status_message.as_deref(),
            ),
        });
    }

    let agent_statuses = snapshot
        .agents
        .iter()
        .map(|agent| (agent.pane_id.as_str(), agent.agent_status))
        .collect::<HashMap<_, _>>();
    for pane in &snapshot.panes {
        let Some(summary) = summaries.get_mut(pane.workspace_id.as_str()) else {
            continue;
        };
        let winner = summary
            .winner
            .as_ref()
            .is_some_and(|winner| winner.pane_id == pane.pane_id);
        summary.pane_dots.push(PaneDot {
            pane_id: pane.pane_id.clone(),
            status: agent_statuses.get(pane.pane_id.as_str()).copied(),
            winner,
        });
    }

    summaries
}

struct Candidate<'a> {
    agent: &'a ClientShellAgent,
    rank: u8,
    focused: bool,
    in_active_tab: bool,
    pane_index: usize,
}

impl Candidate<'_> {
    fn outranks(&self, other: &Self) -> bool {
        (
            self.rank,
            self.focused,
            self.in_active_tab,
            std::cmp::Reverse(self.pane_index),
            std::cmp::Reverse(self.agent.pane_id.as_str()),
        ) > (
            other.rank,
            other.focused,
            other.in_active_tab,
            std::cmp::Reverse(other.pane_index),
            std::cmp::Reverse(other.agent.pane_id.as_str()),
        )
    }

    fn label(&self, snapshot: &ClientShellSnapshot) -> String {
        self.agent
            .display_agent
            .as_deref()
            .or(self.agent.name.as_deref())
            .or(self.agent.agent.as_deref())
            .or(self.agent.title.as_deref())
            .or_else(|| {
                snapshot
                    .panes
                    .iter()
                    .find(|pane| pane.pane_id == self.agent.pane_id)
                    .and_then(|pane| pane.label.as_deref())
            })
            .unwrap_or(&self.agent.pane_id)
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::shell::tests::snapshot;
    use crate::protocol::ClientShellPane;

    fn pane(id: &str, workspace: &str, tab: &str) -> ClientShellPane {
        ClientShellPane {
            pane_id: id.into(),
            workspace_id: workspace.into(),
            tab_id: tab.into(),
            label: None,
            cwd: Some("/repo".into()),
            foreground_cwd: Some("/repo".into()),
            focused: false,
            right_click_passthrough: false,
        }
    }

    fn agent(
        id: &str,
        workspace: &str,
        tab: &str,
        status: AgentStatus,
        focused: bool,
        message: Option<&str>,
        display: Option<&str>,
    ) -> ClientShellAgent {
        ClientShellAgent {
            pane_id: id.into(),
            workspace_id: workspace.into(),
            tab_id: tab.into(),
            name: None,
            display_agent: display.map(str::to_string),
            agent: Some("omp".into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: status,
            state_change_seq: 0,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused,
            status_message: message.map(str::to_string),
        }
    }

    #[test]
    fn winner_follows_status_priority() {
        let candidates = [
            (AgentStatus::Unknown, "pane_unknown"),
            (AgentStatus::Idle, "pane_idle"),
            (AgentStatus::Working, "pane_working"),
            (AgentStatus::Done, "pane_done"),
            (AgentStatus::Blocked, "pane_blocked"),
        ];
        for start in 0..candidates.len() {
            let mut snapshot = snapshot();
            snapshot.panes = candidates
                .iter()
                .map(|(_, id)| pane(id, "ws_1", "tab_1"))
                .collect();
            snapshot.agents = candidates[start..]
                .iter()
                .chain(candidates[..start].iter())
                .map(|(status, id)| agent(id, "ws_1", "tab_1", *status, false, None, None))
                .collect();

            let summaries = workspace_summaries(&snapshot);
            let winner = summaries["ws_1"].winner.as_ref().unwrap();

            assert_eq!(winner.pane_id, "pane_blocked", "rotation {start}");
            assert_eq!(winner.status, AgentStatus::Blocked);
        }
    }

    #[test]
    fn winner_ties_use_focus_then_tab_then_pane_order_then_pane_id() {
        let mut snapshot = snapshot();
        snapshot.workspaces[0].active_tab_id = "tab_1".into();
        snapshot.tabs.push(crate::protocol::ClientShellTab {
            tab_id: "tab_2".into(),
            workspace_id: "ws_1".into(),
            number: 2,
            label: "2".into(),
            custom_label: false,
            zoomed: false,
            focused: false,
            agent_status: AgentStatus::Idle,
        });
        snapshot.panes = vec![
            pane("pane_a", "ws_1", "tab_1"),
            pane("pane_b", "ws_1", "tab_1"),
            pane("pane_c", "ws_1", "tab_2"),
        ];
        snapshot.agents = vec![
            agent(
                "pane_c",
                "ws_1",
                "tab_2",
                AgentStatus::Working,
                false,
                None,
                None,
            ),
            agent(
                "pane_b",
                "ws_1",
                "tab_1",
                AgentStatus::Working,
                false,
                None,
                None,
            ),
            agent(
                "pane_a",
                "ws_1",
                "tab_1",
                AgentStatus::Working,
                false,
                None,
                None,
            ),
        ];

        let winner = workspace_summaries(&snapshot)["ws_1"]
            .winner
            .clone()
            .unwrap();
        assert_eq!(winner.pane_id, "pane_a");

        snapshot.agents[0].focused = true;
        let winner = workspace_summaries(&snapshot)["ws_1"]
            .winner
            .clone()
            .unwrap();
        assert_eq!(winner.pane_id, "pane_c");

        snapshot.agents[0].focused = false;
        snapshot.agents[1].focused = true;
        let winner = workspace_summaries(&snapshot)["ws_1"]
            .winner
            .clone()
            .unwrap();
        assert_eq!(winner.pane_id, "pane_b");

        snapshot.agents[1].focused = false;
        snapshot.agents.swap(1, 2);
        let winner = workspace_summaries(&snapshot)["ws_1"]
            .winner
            .clone()
            .unwrap();
        assert_eq!(
            winner.pane_id, "pane_a",
            "agent list order must not change the pane-order winner"
        );
    }

    #[test]
    fn winner_text_is_never_substituted() {
        let mut snapshot = snapshot();
        snapshot.panes = vec![
            pane("pane_blocked", "ws_1", "tab_1"),
            pane("pane_working", "ws_1", "tab_1"),
        ];
        snapshot.agents = vec![
            agent(
                "pane_blocked",
                "ws_1",
                "tab_1",
                AgentStatus::Blocked,
                false,
                None,
                Some("omp"),
            ),
            agent(
                "pane_working",
                "ws_1",
                "tab_1",
                AgentStatus::Working,
                false,
                Some("Run tests"),
                Some("codex"),
            ),
        ];

        let winner = workspace_summaries(&snapshot)["ws_1"]
            .winner
            .clone()
            .unwrap();

        assert_eq!(winner.pane_id, "pane_blocked");
        assert_eq!(winner.text, None);
        assert_eq!(winner.label, "omp");
    }

    #[test]
    fn dots_cover_every_workspace_tab_and_ignore_the_active_tab() {
        let mut snapshot = snapshot();
        snapshot.workspaces[0].active_tab_id = "tab_1".into();
        snapshot.panes = vec![
            pane("pane_a", "ws_1", "tab_1"),
            pane("pane_bare", "ws_1", "tab_1"),
            pane("pane_c", "ws_1", "tab_2"),
            pane("pane_d", "ws_1", "tab_2"),
        ];
        snapshot.agents = vec![
            agent(
                "pane_a",
                "ws_1",
                "tab_1",
                AgentStatus::Working,
                false,
                Some("Run tests"),
                None,
            ),
            agent(
                "pane_c",
                "ws_1",
                "tab_2",
                AgentStatus::Blocked,
                false,
                None,
                None,
            ),
            agent(
                "pane_d",
                "ws_1",
                "tab_2",
                AgentStatus::Done,
                false,
                None,
                None,
            ),
        ];

        let dots = &workspace_summaries(&snapshot)["ws_1"].pane_dots;

        assert_eq!(dots.len(), 4);
        assert_eq!(dots[0].pane_id, "pane_a");
        assert_eq!(dots[0].status, Some(AgentStatus::Working));
        assert!(!dots[0].winner);
        assert_eq!(dots[1].pane_id, "pane_bare");
        assert_eq!(dots[1].status, None);
        assert!(!dots[1].winner);
        assert_eq!(dots[2].pane_id, "pane_c");
        assert_eq!(dots[2].status, Some(AgentStatus::Blocked));
        assert!(dots[2].winner);
        assert_eq!(dots[3].pane_id, "pane_d");
        assert_eq!(dots[3].status, Some(AgentStatus::Done));
        assert!(!dots[3].winner);

        let dots_before_tab_switch = dots.clone();
        snapshot.workspaces[0].active_tab_id = "tab_2".into();
        assert_eq!(
            workspace_summaries(&snapshot)["ws_1"].pane_dots,
            dots_before_tab_switch,
            "pane indicators stay stable when the active tab changes"
        );
    }

    #[test]
    fn other_tabs_attention_ignores_the_active_tab() {
        let mut snapshot = snapshot();
        snapshot.workspaces[0].active_tab_id = "tab_1".into();
        snapshot.tabs.push(crate::protocol::ClientShellTab {
            tab_id: "tab_2".into(),
            workspace_id: "ws_1".into(),
            number: 2,
            label: "2".into(),
            custom_label: false,
            zoomed: false,
            focused: false,
            agent_status: AgentStatus::Idle,
        });
        snapshot.panes = vec![
            pane("pane_a", "ws_1", "tab_1"),
            pane("pane_c", "ws_1", "tab_2"),
        ];
        snapshot.agents = vec![
            agent(
                "pane_a",
                "ws_1",
                "tab_1",
                AgentStatus::Blocked,
                false,
                None,
                None,
            ),
            agent(
                "pane_c",
                "ws_1",
                "tab_2",
                AgentStatus::Done,
                false,
                None,
                None,
            ),
        ];

        let summary = &workspace_summaries(&snapshot)["ws_1"];

        assert_eq!(summary.winner.as_ref().unwrap().pane_id, "pane_a");
        assert_eq!(summary.other_tabs_attention, Some(AgentStatus::Done));
    }

    #[test]
    fn summaries_never_fold_other_workspaces_into_a_row() {
        let mut snapshot = snapshot();
        snapshot
            .workspaces
            .push(crate::protocol::ClientShellWorkspace {
                workspace_id: "ws_child".into(),
                active_tab_id: "tab_child".into(),
                new_workspace_cwd: "/repo-child".into(),
                number: 2,
                label: "feature".into(),
                custom_label: false,
                branch: Some("feature".into()),
                git_ahead_behind: None,
                tokens: Vec::new(),
                worktree: Some(crate::protocol::ClientShellWorktree {
                    key: "repo-key".into(),
                    label: "repo".into(),
                    is_linked_worktree: true,
                }),
                focused: false,
                agent_status: AgentStatus::Blocked,
            });
        snapshot.panes = vec![
            pane("pane_parent", "ws_1", "tab_1"),
            pane("pane_child", "ws_child", "tab_child"),
        ];
        snapshot.agents = vec![
            agent(
                "pane_child",
                "ws_child",
                "tab_child",
                AgentStatus::Blocked,
                false,
                Some("waiting for approval"),
                Some("omp"),
            ),
            agent(
                "pane_parent",
                "ws_1",
                "tab_1",
                AgentStatus::Working,
                false,
                Some("Run tests"),
                Some("omp"),
            ),
        ];

        let summaries = workspace_summaries(&snapshot);

        assert_eq!(
            summaries["ws_1"].winner.as_ref().unwrap().pane_id,
            "pane_parent"
        );
        assert_eq!(
            summaries["ws_child"].winner.as_ref().unwrap().pane_id,
            "pane_child"
        );
    }
}
