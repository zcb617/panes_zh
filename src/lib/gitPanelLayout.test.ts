import { describe, expect, it } from "vitest";
import { getGitPanelLayoutState } from "./gitPanelLayout";

describe("getGitPanelLayoutState", () => {
  it("docks Git inside the shared workspace layout", () => {
    expect(
      getGitPanelLayoutState({
        activeView: "chat",
        activeWorkspaceId: "workspace-1",
        showGitPanel: true,
        gitPanelPinned: true,
      }),
    ).toEqual({
      workspaceLayoutIntegrated: true,
      gitPanelDocked: true,
      gitPanelDockedInWorkspace: true,
      showWorkspaceHeaderToggle: true,
      showEdgeReveal: false,
    });
  });

  it("keeps the Git toggle in the workspace header when Git is hidden", () => {
    expect(
      getGitPanelLayoutState({
        activeView: "chat",
        activeWorkspaceId: "workspace-1",
        showGitPanel: false,
        gitPanelPinned: true,
      }),
    ).toMatchObject({
      workspaceLayoutIntegrated: true,
      showWorkspaceHeaderToggle: true,
      showEdgeReveal: false,
    });
  });

  it("keeps the edge restore control for views without a workspace header", () => {
    expect(
      getGitPanelLayoutState({
        activeView: "chat",
        activeWorkspaceId: null,
        showGitPanel: false,
        gitPanelPinned: true,
      }),
    ).toMatchObject({
      workspaceLayoutIntegrated: false,
      showWorkspaceHeaderToggle: false,
      showEdgeReveal: true,
    });
  });

  it("does not render Git controls on settings", () => {
    expect(
      getGitPanelLayoutState({
        activeView: "settings",
        activeWorkspaceId: "workspace-1",
        showGitPanel: false,
        gitPanelPinned: true,
      }),
    ).toMatchObject({
      workspaceLayoutIntegrated: false,
      gitPanelDocked: false,
      showWorkspaceHeaderToggle: false,
      showEdgeReveal: false,
    });
  });
});
