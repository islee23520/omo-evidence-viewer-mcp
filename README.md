# OmO Evidence Gallery

A Bun gallery for reviewed, non-sensitive visual evidence. Keep private evidence outside the published root. The HTTP server has no built-in viewer authentication, so protect any network-facing deployment at the proxy or Cloudflare Access layer.

## Local Bun server

Install Bun 1.4+ and run from this checkout:

```sh
export EVIDENCE_ROOT=/data/gallery-public
export HOST=127.0.0.1
export PORT=17678
bun server.mjs
```

`EVIDENCE_ROOT` is the persistent publish-only directory. Without it, the server uses `~/.omo/evidence/gallery-public`. `HOST` defaults to `0.0.0.0`, so set it explicitly for a local-only service. Check `http://127.0.0.1:17678/health` for readiness. Copy `local-previews.example.json` to the ignored `local-previews.json` only if you need machine-local previews. Review every configured source and asset before exposing it. Never place private evidence in the public root.

For an optional macOS login service, copy `com.example.omo-evidence-gallery.plist.example` to your LaunchAgents directory, replace its generic label and paths with your own, set a persistent `EVIDENCE_ROOT`, and create the log directory before loading it with `launchctl bootstrap gui/$(id -u) /path/to/your.plist`. Keep host-specific paths and secrets out of Git.

## Persistent Docker server

The included `Dockerfile` installs the locked dependencies and runs all tests before creating the runtime image. Use this checkout as its build context (`docker build -t evidence-gallery .`). A minimal custom deployment Dockerfile can use:

```dockerfile
FROM oven/bun:1.4
WORKDIR /app
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile
COPY server.mjs storage.mjs repository.mjs ggui.mjs ggui-client.jsx document-data.mjs ./
ENV HOST=0.0.0.0 PORT=17678 EVIDENCE_ROOT=/data/gallery-public
EXPOSE 17678
CMD ["bun", "server.mjs"]
```

This image runs the HTTP gallery and upload endpoint. If you need configured local previews, supply a reviewed `local-previews.json` in the image or mount it separately. Keep any source files it references private and accessible only where intended. Grant the container's Bun user write access to the persistent volume.

Create a Compose file in that workspace, keeping the token in an ignored `.env` or your secret manager:

```yaml
services:
  gallery:
    build: .
    restart: unless-stopped
    environment:
      HOST: 0.0.0.0
      PORT: 17678
      EVIDENCE_ROOT: /data/gallery-public
      UPLOAD_TOKEN: ${UPLOAD_TOKEN:?set a random upload token}
      RETENTION_DAYS: ${RETENTION_DAYS:-30}
      CLEANUP_INTERVAL_HOURS: ${CLEANUP_INTERVAL_HOURS:-24}
    volumes:
      - gallery-data:/data/gallery-public
    networks:
      - private

volumes:
  gallery-data:

networks:
  private:
```

Start with `docker compose up -d --build`. Keep the origin on a private network, with no published host port. Connect a reverse proxy or Cloudflare Tunnel container to that network and point it at `http://gallery:17678`. If the tunnel runs on the host instead, explicitly bind the origin to loopback rather than publishing it to all interfaces. Mount or back up the persistent volume as needed; rebuilding the image must not remove evidence.

Set `UPLOAD_TOKEN` to a random secret to enable uploads. Without it, the upload endpoint returns 503. `RETENTION_DAYS` defaults to 30 and `CLEANUP_INTERVAL_HOURS` to 24; both must be positive. Cleanup runs on startup and periodically, removing only folders with expired server-generated `uploadedAt` metadata. Manually copied folders aren't subject to upload retention. `bun storage.mjs` runs cleanup once when the storage module is available.

## GGUI document viewers

Install runtime dependencies with `bun install --frozen-lockfile`. Set `GGUI_MCP_URL` to your GGUI Streamable HTTP endpoint (including `/mcp`) and `GGUI_MCP_TOKEN` to a paired bearer token in the gallery server's private environment. `GGUI_TIMEOUT_MS` defaults to 180000. In Docker, `127.0.0.1` refers to the gallery container, not the Windows host: use a reachable private GGUI service address or an authenticated host endpoint. Do not expose the token to browsers or commit it.

When an upload contains `.md`, `.markdown`, `.pdf`, `.csv`, or `.xlsx` files and no root `index.html`, the server calls GGUI handshake/render tools and bundles the returned compiled React viewer with its runtime into a self-contained `index.html`. Markdown is sanitized; CSV preserves quoted and multiline cells; XLSX retains worksheets and displayed cell values; PDFs use the original file for browser preview. Originals remain byte-identical, and nested document paths stay relative. The entry's directory URL opens the generated viewer. Explicitly authored `index.html` files are preserved. Without `GGUI_MCP_URL`, uploads retain the existing file-list behavior.

Generation or compilation failure returns HTTP 502 and removes the staged upload rather than publishing an incomplete viewer. Public/private visibility applies to the generated page and originals together. Existing entries are not regenerated by this configuration. The remote `register_evidence` publisher uses this upload path automatically; a local-only registration does not invoke the remote server.

Server images must include `ggui.mjs`, `ggui-client.jsx`, `document-data.mjs`, `package.json`, `bun.lock`, and installed runtime dependencies alongside the existing server modules. Run `bun test` and `bun run build` before deploying.

## Cloudflare Tunnel and Access

In your own Cloudflare account, route `gallery.example.com` through a Tunnel to the private HTTP origin. Create an Access application for that hostname. Add an Allow policy for the exact email addresses or your organization's email domain, such as `example.org`. Include rules within one policy are alternatives, not cumulative conditions. Use separate policies if your access design needs different groups. Enable Access enforcement at the tunnel origin so bypassing the login page doesn't expose the gallery; also restrict direct origin reachability.

For unattended uploads, create a dedicated Access service token and a Service Auth policy on the application. Configure the publisher with that token's client ID and secret. The headers alone don't authorize a request until the Service Auth policy exists. Human viewers use the Allow policy. Cloudflare Access and the upload bearer token serve different purposes: configure both for protected automated uploads. Don't commit credentials or copy private evidence into the published root.

## Multipart uploads and remote MCP publisher

Upload only reviewed files with a multipart `metadata` JSON field and repeated `files` fields. Relative filenames can preserve subdirectories for HTML assets. Requests are limited to 100 MiB and 200 files; unsafe paths and duplicate names are rejected. A completed upload returns HTTP 201 with a relative gallery `url`. Missing or incorrect bearer authorization returns 401; invalid metadata or paths return 400.

```sh
curl --fail-with-body https://gallery.example.com/api/evidence \
  -H "Authorization: Bearer $UPLOAD_TOKEN" \
  -H "CF-Access-Client-Id: $CF_ACCESS_CLIENT_ID" \
  -H "CF-Access-Client-Secret: $CF_ACCESS_CLIENT_SECRET" \
  -F 'metadata={"slug":"homepage-review","title":"Homepage review","repository":"https://github.com/example/project","labels":["reviewed"],"categories":["UI"]}' \
  -F 'files=@index.html;filename=index.html' \
  -F 'files=@screenshot.png;filename=assets/screenshot.png'
```

For an MCP client, copy `mcp.example.json` into your client configuration and set its script path to this checkout's absolute `mcp.mjs` path. Set `EVIDENCE_ROOT` to a local, publish-only staging directory. For the remote publisher, also set `EVIDENCE_SERVER_URL=https://gallery.example.com` and `UPLOAD_TOKEN`; set `CF_ACCESS_CLIENT_ID` and `CF_ACCESS_CLIENT_SECRET` when the Access application requires a service token. Configure these variables in your client or secret store, not in a tracked file. Review and stage the folder before calling `register_evidence`. Once remote publishing is installed and configured, it uploads staged files and returns a remote gallery URL. The server, Tunnel, Access policies, and tokens must be configured by the operator; this repository doesn't provision them.

Without a remote server URL, the local stdio tool registers an existing folder beneath `EVIDENCE_ROOT` and returns a relative `/evidence/<slug>/` URL. It doesn't accept a private source path or copy a private tree. Example tool arguments:

```json
{"slug":"my-project-review","repository":"https://github.com/example/my-project.git","title":"Homepage review","labels":["desktop","reviewed"],"categories":["UI"]}
```

Registration requires a GitHub repository URL, either `https://github.com/owner/repository` or `git@github.com:owner/repository.git`. The main page groups entries by canonical `owner/repository`, not folder name, and sorts newest entries first. Entries without a valid repository are omitted from the index. Evidence pages show images, videos, and PDFs with previous and next navigation; `?page=3` links to the third item. Served HTML files include a fixed gallery-home link; `?raw=1` returns original bytes for integrity verification. An `index.html` opens as a file, while its directory URL shows the listing. Keep local configuration and secrets ignored, and publish no private evidence.

## Key-equipped computers

## Private and public entries

Uploads and MCP registration accept `visibility`, either `private` (the default) or `public`. Existing metadata without this field remains private. The gallery index and `/evidence/` routes must stay behind the operator's authentication proxy. Explicitly public entries are available through `/public/<slug>/` and their nested asset paths; private entries return 404 through that route. Public links do not expose a gallery index. Configure an authentication exception for `/public/*` only, and route that exception to the same server so it checks visibility on every request. Never bypass authentication for the whole hostname. Disable intermediary caching for this route so revocation takes effect.

An authenticated publisher can change an existing entry using `PATCH /api/evidence/<slug>/visibility` with the upload bearer token and JSON `{"visibility":"public"}` or `{"visibility":"private"}`. Changing back to private disables both the public page and all public asset paths. Private viewing still requires the configured viewer authentication; the upload token is not a browser login.

Store client configuration in `~/.omo/evidence-client.json` (mode `600` on Unix), or set `EVIDENCE_CLIENT_CONFIG` to a private file. Use your own values; never commit the resulting file:

```json
{"EVIDENCE_SERVER_URL":"https://gallery.example.com","EVIDENCE_ROOT":"/path/to/reviewed-staging","UPLOAD_TOKEN":"<upload-secret>","CF_ACCESS_CLIENT_ID":"<service-client-id>","CF_ACCESS_CLIENT_SECRET":"<service-client-secret>"}
```

The MCP reads this file automatically; explicit environment variables take precedence. It verifies each published file against the source SHA-256 before returning an absolute remote URL. Remove disposable staging files only after successful verification; retain product source assets and test fixtures.

Run `bun browse.mjs` on a key-equipped computer to browse without a manual Access login. Open the printed loopback URL on that computer. This read-only gateway adds service authentication to upstream requests and binds only `127.0.0.1`; it does not accept uploads, forward browser cookies, or expose the key to page scripts. A key file alone does not authenticate an ordinary browser request to the public hostname. Keep the gateway private and revoke its service token when retiring a computer. The public hostname still requires Access authentication for clients without a valid credential.

For a persistent client, put `mcp.mjs`, `repository.mjs`, `browse.mjs`, and `install-client.mjs` together in a private installation directory, create the credential file, then run `bun install-client.mjs`. It preserves other MCP servers while registering `evidence-viewer` and installs a user-level gateway service: LaunchAgent on macOS, systemd user service on Linux, or a logon task on Windows. Verify `http://127.0.0.1:17677/health` returns `{"status":"ok"}` without an interactive login. Services run as the installing user; start the user's desktop/session service manager before installing. Grant service tokens per computer where independent revocation is needed, and rotate expired or exposed credentials.
