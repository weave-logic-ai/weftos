import { useEffect, useCallback } from "react";
import { useCanvasStore } from "../stores/canvas-store";
import { CanvasRenderer } from "../components/canvas/canvas-renderer";
import { CanvasToolbar } from "../components/canvas/canvas-toolbar";
import { wsClient } from "../lib/ws-client";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import type {
  CanvasCommandData,
  CanvasInteractionData,
} from "../lib/types";

export function CanvasPage() {
  const { addElement, updateElement, removeElement, reset, elements } =
    useCanvasStore();

  // Subscribe to canvas commands from the WebSocket broadcaster.
  //
  // WEFT-306: the gateway publishes validated CanvasCommands on the
  // `canvas` topic. The WS handler envelopes them as
  // `{type:"event", topic:"canvas", data:{type:"canvas_command",
  // command, id, element, ...}}`, so we listen on the generic `event`
  // channel and filter on topic + data.type — the same pattern
  // voice-chat.ts uses for `sessions:*`.
  useEffect(() => {
    wsClient.subscribe("canvas");

    const off = wsClient.on("event", (raw: unknown) => {
      const msg = raw as {
        type?: string;
        topic?: string;
        data?: CanvasCommandData & { type?: string };
      };

      if (msg.topic !== "canvas" || msg.data?.type !== "canvas_command") {
        return;
      }

      const cmd = msg.data;

      switch (cmd.command) {
        case "render":
          if (cmd.id && cmd.element) {
            addElement(cmd.id, { id: cmd.id, ...cmd.element });
          }
          break;
        case "update":
          if (cmd.id && cmd.element) {
            updateElement(cmd.id, { id: cmd.id, ...cmd.element });
          }
          break;
        case "remove":
          if (cmd.id) {
            removeElement(cmd.id);
          }
          break;
        case "reset":
          reset();
          break;
        case "batch":
          // For batch, the server should send individual sub-commands.
          // This is a fallback for flat batch delivery.
          break;
      }
    });

    return () => {
      off();
      wsClient.unsubscribe("canvas");
    };
  }, [addElement, updateElement, removeElement, reset]);

  // Send interactions back to the server
  const handleInteraction = useCallback(
    (interaction: { type: string; element_id: string; [key: string]: unknown }) => {
      const { type: interactionType, ...rest } = interaction;
      const payload: CanvasInteractionData = {
        interaction: interactionType,
        ...rest,
      };
      wsClient.send({ type: "canvas_interaction", ...payload });
    },
    [],
  );

  const handleReset = useCallback(() => {
    reset();
  }, [reset]);

  return (
    <div className="flex-1 overflow-auto p-6 space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-gray-900 dark:text-gray-100">
            Canvas
          </h1>
          <p className="text-sm text-gray-500 dark:text-gray-400">
            Live canvas workspace rendered by agents
          </p>
        </div>
        <div className="flex items-center gap-3">
          <CanvasToolbar />
          <Badge variant="secondary">{elements.size} elements</Badge>
          <Button variant="ghost" size="sm" onClick={handleReset}>
            Clear
          </Button>
        </div>
      </div>

      <CanvasRenderer onInteraction={handleInteraction} />
    </div>
  );
}
