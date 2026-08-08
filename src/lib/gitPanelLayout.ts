import type { ActiveView } from "../stores/uiStore";

interface GitPanelLayoutInput {
  activeView: ActiveView;
  activeWorkspaceId: string | null;
  showGitPanel: boolean;
  gitPanelPinned: boolean;
}

export interface GitPanelLayoutState {
  workspaceLayoutIntegrated: boolean;
  gitPanelDocked: boolean;
  gitPanelDockedInWorkspace: boolean;
  showWorkspaceHeaderToggle: boolean;
  showEdgeReveal: boolean;
}

export function getGitPanelLayoutState({
  activeView,
  activeWorkspaceId,
  showGitPanel,
  gitPanelPinned,
}: GitPanelLayoutInput): GitPanelLayoutState {
  const settingsActive = activeView === "settings";
  const workspaceLayoutIntegrated = activeView === "chat" && activeWorkspaceId !== null;
  const gitPanelDocked = showGitPanel && gitPanelPinned && !settingsActive;
  const gitPanelDockedInWorkspace = gitPanelDocked && workspaceLayoutIntegrated;

  return {
    workspaceLayoutIntegrated,
    gitPanelDocked,
    gitPanelDockedInWorkspace,
    showWorkspaceHeaderToggle: workspaceLayoutIntegrated,
    showEdgeReveal: !showGitPanel && !settingsActive && !workspaceLayoutIntegrated,
  };
}
