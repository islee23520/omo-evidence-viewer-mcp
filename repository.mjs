export function githubRepository(value) {
  if (typeof value !== "string") throw new Error("GitHub repository URL is required");
  const match = /^(?:https:\/\/github\.com\/|git@github\.com:)([a-z0-9][a-z0-9-]*)\/([a-z0-9_.-]+?)(?:\.git)?\/?$/i.exec(value.trim());
  if (!match || [".", ".."].includes(match[2])) throw new Error("Use a GitHub repository URL: https://github.com/owner/repository");
  return `https://github.com/${match[1].toLowerCase()}/${match[2].toLowerCase()}`;
}
