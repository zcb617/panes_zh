import { describe, expect, it } from "vitest";
import type { ContentBlock } from "../../types";
import { buildBlockSegments } from "./MessageBlocks";

describe("buildBlockSegments", () => {
  it("keeps non-Codex hooks at their stream positions", () => {
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

    const segments = buildBlockSegments(blocks, true, "claude");

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

  it("merges Codex hooks through the streaming tail of the same reply", () => {
    const hookKinds = [
      "hook_started_prefix_1",
      "hook_completed_prefix_1",
      "hook_started_prefix_2",
      "hook_completed_prefix_2",
      "hook_started_prefix_3",
      "hook_completed_prefix_3",
      "hook_started_reply_1",
      "hook_completed_reply_1",
      "hook_started_reply_2",
      "hook_completed_reply_2",
      "hook_started_reply_3",
      "hook_completed_reply_3",
      "hook_started_reply_4",
      "hook_completed_reply_4",
      "hook_started_reply_5",
      "hook_completed_reply_5",
      "hook_started_reply_6",
      "hook_completed_reply_6",
      "hook_started_reply_7",
    ];
    const hookBlocks: ContentBlock[] = hookKinds.map((kind) => ({
      type: "notice",
      kind,
      level: "info",
      title: "Hook",
      message: kind,
    }));
    const blocks: ContentBlock[] = [
      ...hookBlocks.slice(0, 6),
      { type: "text", content: "reply 1" },
      ...hookBlocks.slice(6, 8),
      {
        type: "action",
        actionId: "action-1",
        actionType: "other",
        summary: "Tool call 1",
        details: {},
        outputChunks: [],
        status: "done",
      },
      ...hookBlocks.slice(8, 12),
      {
        type: "action",
        actionId: "action-2",
        actionType: "other",
        summary: "Tool call 2",
        details: {},
        outputChunks: [],
        status: "done",
      },
      ...hookBlocks.slice(12, 16),
      {
        type: "action",
        actionId: "action-3",
        actionType: "other",
        summary: "Tool call 3",
        details: {},
        outputChunks: [],
        status: "done",
      },
      ...hookBlocks.slice(16),
    ];

    const segments = buildBlockSegments(blocks, true, "codex");

    expect(segments.map((segment) => segment.kind)).toEqual([
      "hook-group",
      "single",
      "hook-group",
      "action-card",
    ]);
    expect(segments[0]).toMatchObject({
      kind: "hook-group",
      indices: [0, 1, 2, 3, 4, 5],
    });
    expect(segments[1]).toMatchObject({
      kind: "single",
      index: 6,
      block: { type: "text", content: "reply 1" },
    });
    expect(segments[2]).toMatchObject({
      kind: "hook-group",
      indices: [7, 8, 10, 11, 12, 13, 15, 16, 17, 18, 20, 21, 22],
      blocks: { length: 13 },
    });
    expect(segments[3]).toMatchObject({
      kind: "action-card",
    });
    const actionCard = segments[3];
    if (actionCard.kind === "action-card") {
      const actionIds = actionCard.segments.flatMap((segment) => {
        if (segment.kind === "action-group") {
          return segment.blocks.map((block) => block.actionId);
        }
        if (segment.kind === "single" && segment.block.type === "action") {
          return [segment.block.actionId];
        }
        return [];
      });
      expect(actionIds).toEqual(["action-1", "action-2", "action-3"]);
    }
  });

  it("groups subagent actions and marked hooks into separate activity cards", () => {
    const blocks: ContentBlock[] = [
      { type: "text", content: "主代理进度" },
      {
        type: "notice",
        kind: "hook_started_regular",
        level: "info",
        title: "Hook started",
        message: "普通 Hook",
      },
      {
        type: "action",
        actionId: "activity-child-1",
        actionType: "other",
        summary: "子代理已开始",
        details: {
          subagentThreadId: "child-1",
          subagentActivity: "started",
          agentPath: "编码任务",
        },
        outputChunks: [],
        status: "running",
      },
      {
        type: "action",
        actionId: "command-child-1",
        actionType: "command",
        summary: "echo test",
        details: { subagentThreadId: "child-1" },
        outputChunks: [],
        status: "done",
      },
      {
        type: "notice",
        kind: "hook_completed_1::subagent::child-1",
        level: "info",
        title: "Hook completed",
        message: "子代理 Hook",
      },
      {
        type: "action",
        actionId: "command-child-2",
        actionType: "command",
        summary: "echo second",
        details: { subagentThreadId: "child-2" },
        outputChunks: [],
        status: "done",
      },
      {
        type: "action",
        actionId: "ordinary-action",
        actionType: "other",
        summary: "普通动作",
        details: {},
        outputChunks: [],
        result: {
          success: true,
          output: "仅完成结果输出",
          durationMs: 1,
        },
        status: "done",
      },
    ];

    const segments = buildBlockSegments(blocks, true, "claude");
    const subagentCards = segments.filter((segment) => segment.kind === "subagent-card");
    expect(subagentCards).toHaveLength(2);
    expect(segments.map((segment) => segment.kind)).toEqual([
      "single",
      "hook-group",
      "subagent-card",
      "subagent-card",
      "action-card",
    ]);
    expect(subagentCards[0]).toMatchObject({
      threadId: "child-1",
      indices: [2, 3, 4],
      blocks: [{ actionId: "activity-child-1" }, { actionId: "command-child-1" }, { kind: "hook_completed_1::subagent::child-1" }],
    });
    expect(subagentCards[1]).toMatchObject({
      threadId: "child-2",
      indices: [5],
    });
    expect(segments.at(-1)).toMatchObject({
      kind: "action-card",
      segments: [{ kind: "single", block: { actionId: "ordinary-action" } }],
    });
  });

  it("keeps malformed subagent metadata in ordinary segments and retains result output", () => {
    const blocks: ContentBlock[] = [
      {
        type: "action",
        actionId: "ordinary-result",
        actionType: "other",
        summary: "普通结果",
        details: { subagentThreadId: 42 },
        outputChunks: [],
        result: { success: true, output: "可展开输出", durationMs: 0 },
        status: "done",
      },
      {
        type: "notice",
        kind: "hook_completed_plain",
        level: "info",
        title: "Hook completed",
        message: "普通 Hook",
      },
    ];

    const segments = buildBlockSegments(blocks, true, "codex");
    expect(segments.some((segment) => segment.kind === "subagent-card")).toBe(false);
    expect(segments[0]).toMatchObject({
      kind: "action-card",
      segments: [{ kind: "single", block: { actionId: "ordinary-result", result: { output: "可展开输出" } } }],
    });
    expect(segments[1]).toMatchObject({ kind: "hook-group", indices: [1] });
  });
});
