# OmO Evidence Gallery

## Windows upload server

The Windows service is deployed by the `evidence` profile in `deployment-ci` and served at `https://gallery.example.com` through Cloudflare Tunnel. Cloudflare Access allows verified `@example.org` identities. Additional email addresses can later be added as Include / Email rules in the gallery's Allow policy; Include rules are alternatives, not additional requirements. Keep the origin on the private Docker network and enable tunnel-side Access JWT enforcement for this application's audience.

Set `EVIDENCE_ROOT` to the persistent evidence directory, `UPLOAD_TOKEN` to a random secret, `RETENTION_DAYS` (default `30`), and `CLEANUP_INTERVAL_HOURS` (default `24`). The server cleans expired uploads at startup and periodically. Only entries carrying server-generated `uploadedAt` metadata are removed; older manually copied evidence is preserved. `bun storage.mjs` runs the same cleanup once. `/health` reports readiness. Requests are limited to 100 MiB and 200 files.

Upload reviewed files using multipart `metadata` JSON and repeated `files` fields. Relative file names may include subdirectories, preserving HTML assets. Unsafe paths, case-insensitive duplicates, and Windows reserved names are rejected. Each upload receives a unique directory and becomes visible only after all files and metadata are written.

```sh
curl --fail-with-body https://gallery.example.com/api/evidence \
  -H "Authorization: Bearer $UPLOAD_TOKEN" \
  -H "CF-Access-Client-Id: $CF_ACCESS_CLIENT_ID" \
  -H "CF-Access-Client-Secret: $CF_ACCESS_CLIENT_SECRET" \
  -F 'metadata={"slug":"homepage-review","title":"Homepage review","repository":"https://github.com/example/project","labels":["reviewed"],"categories":["UI"]}' \
  -F 'files=@index.html;filename=index.html' \
  -F 'files=@screenshot.png;filename=assets/screenshot.png'
```

For automation, create a dedicated Cloudflare Access service token and a Service Auth policy for the gallery. These headers do not grant access until that policy exists. Human accounts continue to use the domain Allow policy. Store upload and service-token secrets outside Git. A successful upload returns HTTP 201 with its gallery `url`; missing or incorrect upload authorization returns 401, and malformed metadata or paths return 400. Without `UPLOAD_TOKEN`, uploads are disabled (503).

Run `bun server.mjs` to browse reviewed previews and the publish-only `~/.omo/evidence/gallery-public` directory on port 17678. The server binds to `0.0.0.0` by default, so use the machine's Tailscale address when browsing from another device. There is no authentication: only put non-sensitive, reviewable artifacts into `gallery-public`. All other evidence and other files in `~/.omo` stay outside the server's file root. To publish future work, put a reviewed artifact in `gallery-public` or configure a narrowly scoped preview in the ignored `local-previews.json`.

For a Mac login service, copy the `.plist.example` file to `~/Library/LaunchAgents/com.example.omo-evidence-gallery.plist`, replace `YOUR_USER` with your local username, create its log directory, and load it with `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.example.omo-evidence-gallery.plist`. Copy `local-previews.example.json` to `local-previews.json` for machine-local previews, and add only entries whose sources are safe to expose. Reload a changed LaunchAgent with `launchctl bootout gui/$(id -u)/com.example.omo-evidence-gallery` followed by the bootstrap command.

## Main page and MCP registration

The main page at `/` groups registered public evidence by Git repository name (the final part of its remote URL) and sorts projects and entries newest-created first. Missing local preview sources are omitted. Register a reviewed folder with the MCP tool to record its Git remote, title, labels, and categories as typed tags. The gallery reads `.gallery.json` on each request; no restart is needed. Older folders without registration still use their folder-name project and date, while configured previews can set `repository` and `date` in the ignored `local-previews.json` (or fall back to `project`/slug and source modification time). Registered evidence uses its first registration time (unchanged by re-registration); older evidence and previews use filesystem creation time, falling back to modification time when unavailable. The displayed date remains the publication date.

Evidence folder pages display images, videos, and PDFs in filename order with previous/next buttons and left/right arrow keys. `?page=3` links directly to the third item, and browser Back returns to the previous selection. The file list remains available below the viewer for HTML pages and downloads. Configured multi-image previews use the same navigation.

Run `bun mcp.mjs` as a **local stdio MCP server**. Copy `mcp.example.json` into your MCP client's server configuration and replace the placeholder with this checkout's absolute `mcp.mjs` path. The tool `register_evidence` accepts:

```json
{"slug":"my-project-review-20260928","repository":"https://github.com/example/my-project.git","title":"Homepage review","labels":["desktop","reviewed"],"categories":["UI"]}
```

First copy only reviewed, non-sensitive assets into `~/.omo/evidence/gallery-public/my-project-review-20260928/`, then call the tool. HTTPS and `git@host:owner/repo.git` SSH remotes for the same Git repository share one project group. `date` is optional and defaults to today; labels and categories may be empty arrays. The tool labels that existing directory and returns `/evidence/my-project-review-20260928/`; it does **not** accept a source path or copy a private tree. Each folder may contain an `index.html` and relative assets. Open its `index.html` explicitly for a page; the directory URL shows a file listing. The HTTP server binds to all interfaces by default and has no authentication, so anything in the public root is already reachable even before registration. Do not put credentials, private material, or machine-specific paths there.
