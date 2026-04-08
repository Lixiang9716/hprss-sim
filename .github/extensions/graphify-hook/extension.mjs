import { existsSync } from "node:fs";
import { join } from "node:path";
import { joinSession } from "@github/copilot-sdk/extension";

const SEARCH_TOOLS = new Set(["glob", "grep"]);

const session = await joinSession({
    hooks: {
        onPreToolUse: async (input) => {
            if (!SEARCH_TOOLS.has(input.toolName)) return;

            const graphPath = join(process.cwd(), "graphify-out", "graph.json");
            if (!existsSync(graphPath)) return;

            return {
                additionalContext:
                    "graphify: Knowledge graph exists. Read graphify-out/GRAPH_REPORT.md for god nodes and community structure before searching raw files.",
            };
        },
    },
    tools: [],
});
