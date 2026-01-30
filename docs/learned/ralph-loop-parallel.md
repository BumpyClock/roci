# Ralph Loop Parallel Agents Notes
Date: 2026-01-30

Sources
- https://github.com/0xsourav/continuous-claude
- https://github.com/Mintplex-Labs/awesome-claude
- https://github.com/ruvnet/claude-flow
- https://glama.ai/mcp/servers/claude_parallel

Findings
- Baseline pattern is a tight shell loop around a single CLI invocation (`claude -p`, `npx ...`) with prompt file read each iteration.
- Parallelism is typically done by running multiple independent CLI processes per iteration and aggregating outputs.
- Output prefixing per agent/task helps readability when streams interleave.
