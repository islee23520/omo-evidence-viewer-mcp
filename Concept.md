# OmO Evidence Viewer MCP

A small Bun HTTP gallery for showing reviewed, non-sensitive visual evidence in a browser. Its main page groups public evidence and narrowly scoped local previews by project and date. A local stdio MCP tool registers labels for existing public folders without copying private source files. The LaunchAgent keeps the HTTP server available on a personal Mac.

Stack: Bun 1.4+, JavaScript, macOS LaunchAgent or Windows Docker service managed by deployment-ci. No additional runtime dependencies. The Windows service accepts token-authenticated multipart uploads into a configurable persistent root and periodically removes expired uploads. Cloudflare Tunnel and Access protect the public hostname; legacy local evidence remains supported.
