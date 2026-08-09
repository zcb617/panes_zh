import type { ActiveView } from "../stores/uiStore";

interface GitPanelLayoutInput {
  activeView: ActiveView;
  activeWorkspaceId: string | null;
  showGitPanel: boolean;
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
}: GitPanelLayoutInput): GitPanelLayoutState {
  const settingsActive = activeView === "settings";
  const workspaceLayoutIntegrated = activeView === "chat" && activeWorkspaceId !== null;
  const gitPanelDocked = showGitPanel && !settingsActive;
  const gitPanelDockedInWorkspace = gitPanelDocked && workspaceLayoutIntegrated;

  return {
    workspaceLayoutIntegrated,
    gitPanelDocked,
    gitPanelDockedInWorkspace,
    showWorkspaceHeaderToggle: workspaceLayoutIntegrated,
    showEdgeReveal: !showGitPanel && !settingsActive && !workspaceLayoutIntegrated,
  };
}
