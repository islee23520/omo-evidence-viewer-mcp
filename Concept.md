# OmO Evidence Viewer MCP

A small Bun HTTP gallery for showing reviewed, non-sensitive visual evidence in a browser. Its main page groups public evidence and narrowly scoped local previews by project and date. A local stdio MCP tool registers existing public folders without copying private source files. A local Bun process, optional macOS login service, or persistent Docker service can host the HTTP gallery.

Stack: Bun 1.4+ and JavaScript, with no additional runtime dependencies. The server accepts token-authenticated multipart uploads into a configurable persistent root and periodically removes expired uploads. A self-hosted Cloudflare Tunnel and Access application can protect a network-facing hostname. The operator configures its domain, policies, and secrets. Local evidence registration remains supported; a remote MCP publisher can upload reviewed staged files when configured.
