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
      showWorkspaceHeaderReveal: false,
      showEdgeReveal: false,
    });
  });

  it("places the hidden Git restore control in the workspace header", () => {
    expect(
      getGitPanelLayoutState({
        activeView: "chat",
        activeWorkspaceId: "workspace-1",
        showGitPanel: false,
        gitPanelPinned: true,
      }),
    ).toMatchObject({
      workspaceLayoutIntegrated: true,
      showWorkspaceHeaderReveal: true,
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
      showWorkspaceHeaderReveal: false,
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
      showWorkspaceHeaderReveal: false,
      showEdgeReveal: false,
    });
  });
});
