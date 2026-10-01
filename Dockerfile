FROM oven/bun:1.4 AS tested
WORKDIR /app
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile
COPY server.mjs storage.mjs repository.mjs mcp.mjs ggui.mjs ggui-client.jsx document-data.mjs ./
COPY gallery.test.mjs reviews.test.mjs ggui.test.mjs document-data.test.mjs ./
RUN bun test

FROM oven/bun:1.4
WORKDIR /app
COPY --from=tested /app/node_modules ./node_modules
COPY --from=tested /app/package.json ./
COPY --from=tested /app/server.mjs /app/storage.mjs /app/repository.mjs /app/mcp.mjs /app/ggui.mjs /app/ggui-client.jsx /app/document-data.mjs ./
ENV HOST=0.0.0.0 PORT=17678 EVIDENCE_ROOT=/data RETENTION_DAYS=30 CLEANUP_INTERVAL_HOURS=24
HEALTHCHECK --interval=5s --timeout=3s --start-period=10s --retries=6 CMD bun -e 'const r = await fetch("http://127.0.0.1:17678/health"); process.exit(r.ok && (await r.json()).status === "ok" ? 0 : 1)'
CMD ["bun", "server.mjs"]
