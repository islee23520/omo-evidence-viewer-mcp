import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { mkdir, readFile, writeFile } from "node:fs/promises";

const home = homedir();
const client = resolve(import.meta.dir);
const bun = process.execPath;
const configPath = join(home, ".omo", "evidence-client.json");
const config = JSON.parse(await readFile(configPath, "utf8"));
if (!config.EVIDENCE_SERVER_URL || !config.CF_ACCESS_CLIENT_ID || !config.CF_ACCESS_CLIENT_SECRET) throw new Error("Install private client credentials first");
await mkdir(config.EVIDENCE_ROOT, { recursive: true });
const mcpPath = join(home, ".omo", "mcp.json");
let mcp = {};
try { mcp = JSON.parse(await readFile(mcpPath, "utf8")); } catch (error) { if (error.code !== "ENOENT") throw error; }
mcp.mcpServers ||= {};
mcp.mcpServers["evidence-viewer"] = { command: bun, args: [join(client, "mcp.mjs")] };
await writeFile(mcpPath, JSON.stringify(mcp, null, 2) + "\n");

async function command(args) {
  const proc = Bun.spawn(args, { stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, code] = await Promise.all([new Response(proc.stdout).text(), new Response(proc.stderr).text(), proc.exited]);
  if (code) throw new Error(`${args[0]} failed: ${stderr || stdout}`);
}
if (process.platform === "darwin") {
  const label = "com.example.evidence-browser";
  const path = join(home, "Library", "LaunchAgents", `${label}.plist`);
  const escape = value => value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll('"', "&quot;");
  await mkdir(join(home, "Library", "LaunchAgents"), { recursive: true });
  await writeFile(path, `<?xml version="1.0"?><plist version="1.0"><dict><key>Label</key><string>${label}</string><key>ProgramArguments</key><array><string>${escape(bun)}</string><string>${escape(join(client, "browse.mjs"))}</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/></dict></plist>`);
  const domain = `gui/${process.getuid()}`;
  const unload = Bun.spawn(["launchctl", "bootout", `${domain}/${label}`], { stdout: "ignore", stderr: "ignore" });
  await unload.exited;
  await command(["launchctl", "bootstrap", domain, path]);
} else if (process.platform === "linux") {
  const dir = join(home, ".config", "systemd", "user");
  await mkdir(dir, { recursive: true });
  await writeFile(join(dir, "evidence-browser.service"), `[Unit]\nDescription=Private evidence browser gateway\n[Service]\nExecStart="${bun}" "${join(client, "browse.mjs")}"\nRestart=on-failure\n[Install]\nWantedBy=default.target\n`);
  await command(["systemctl", "--user", "daemon-reload"]);
  await command(["systemctl", "--user", "enable", "--now", "evidence-browser.service"]);
} else if (process.platform === "win32") {
  const script = join(client, "start-browser.ps1");
  const quote = value => value.replaceAll("'", "''");
  await writeFile(script, `Start-Process -WindowStyle Hidden -FilePath '${quote(bun)}' -ArgumentList '${quote(join(client, "browse.mjs"))}'\n`);
  await command(["schtasks.exe", "/Create", "/F", "/SC", "ONLOGON", "/TN", "EvidenceBrowser", "/TR", `powershell.exe -NoProfile -ExecutionPolicy Bypass -File "${script}"`]);
  await command(["schtasks.exe", "/Run", "/TN", "EvidenceBrowser"]);
} else throw new Error("Unsupported client platform");
console.log("Installed remote MCP publisher and loopback browser gateway at http://127.0.0.1:17677/");
