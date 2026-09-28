# OmO Evidence Gallery

Run `bun server.mjs` to browse reviewed previews and the publish-only `~/.omo/evidence/gallery-public` directory on port 17678. The server binds to `0.0.0.0` by default, so use the machine's Tailscale address when browsing from another device. There is no authentication: only put non-sensitive, reviewable artifacts into `gallery-public`. All other evidence and other files in `~/.omo` stay outside the server's file root. To publish future work, put a reviewed artifact in `gallery-public` or configure a narrowly scoped preview in the ignored `local-previews.json`.

For a Mac login service, copy the `.plist.example` file to `~/Library/LaunchAgents/com.example.omo-evidence-gallery.plist`, replace `YOUR_USER` with your local username, create its log directory, and load it with `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.example.omo-evidence-gallery.plist`. Copy `local-previews.example.json` to `local-previews.json` for machine-local previews, and add only entries whose sources are safe to expose. Reload a changed LaunchAgent with `launchctl bootout gui/$(id -u)/com.example.omo-evidence-gallery` followed by the bootstrap command.

## Main page and MCP registration

The main page at `/` lists all public evidence folders and available configured local previews by project, then by newest date. Missing local preview sources are omitted from the list. Existing folders need no migration: the gallery derives a project from the first part of their name (or a shared two-part prefix when several folders have one) and a date from a `YYYYMMDD` name suffix (otherwise the folder modification date). For more accurate labels, register a reviewed folder with the MCP tool. The gallery reads `.gallery.json` from that folder on each request; no restart is needed. Configured previews can set `project` and `date` in the ignored `local-previews.json`; otherwise their slug and source modification date supply fallback labels.

Run `bun mcp.mjs` as a **local stdio MCP server**. Copy `mcp.example.json` into your MCP client's server configuration and replace the placeholder with this checkout's absolute `mcp.mjs` path. The tool `register_evidence` accepts:

```json
{"slug":"my-project-review-20260928","project":"My Project","date":"2026-09-28","title":"Homepage review"}
```

First copy only reviewed, non-sensitive assets into `~/.omo/evidence/gallery-public/my-project-review-20260928/`, then call the tool. It labels that existing directory and returns `/evidence/my-project-review-20260928/`; it does **not** accept a source path or copy a private tree. Each folder may contain an `index.html` and relative assets. Open its `index.html` explicitly for a page; the directory URL shows a file listing. The HTTP server binds to all interfaces by default and has no authentication, so anything in the public root is already reachable even before registration. Do not put credentials, private material, or machine-specific paths there.
