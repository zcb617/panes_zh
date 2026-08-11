import { describe, expect, it } from "vitest";
import type { ContentBlock } from "../../types";
import { buildBlockSegments } from "./MessageBlocks";

describe("buildBlockSegments", () => {
  it("keeps consecutive hook groups at their stream positions", () => {
    const blocks: ContentBlock[] = [
      { type: "text", content: "before hooks" },
      {
        type: "notice",
        kind: "hook_started_first",
        level: "info",
        title: "Hook started",
        message: "first hook started",
      },
      {
        type: "notice",
        kind: "hook_completed_first",
        level: "info",
        title: "Hook completed",
        message: "first hook completed",
      },
      {
        type: "action",
        actionId: "action-1",
        actionType: "other",
        summary: "Tool call",
        details: {},
        outputChunks: [],
        status: "running",
      },
      {
        type: "notice",
        kind: "codex_hook_started_second",
        level: "info",
        title: "Hook started",
        message: "second hook started",
      },
      { type: "text", content: "after hooks" },
    ];

    const segments = buildBlockSegments(blocks, true);

    expect(segments.map((segment) => segment.kind)).toEqual([
      "single",
      "hook-group",
      "action-card",
      "hook-group",
      "single",
    ]);
    expect(segments[1]).toMatchObject({
      kind: "hook-group",
      indices: [1, 2],
      blocks: [
        { kind: "hook_started_first" },
        { kind: "hook_completed_first" },
      ],
    });
    expect(segments[3]).toMatchObject({
      kind: "hook-group",
      indices: [4],
      blocks: [{ kind: "codex_hook_started_second" }],
    });
  });
});
