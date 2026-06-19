import type { QuickButton } from "./settings";

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function matchesAny(value: string | undefined, patterns: string[] | undefined): boolean {
  if (!patterns?.length) return true;
  if (!value) return false;
  return patterns.some((pattern) => {
    const source = escapeRegExp(pattern.trim()).replace(/\\\*/g, ".*");
    return new RegExp(`^${source}$`, "i").test(value);
  });
}

export function snippetApplies(button: QuickButton, host?: string, sessionGroup?: string): boolean {
  return matchesAny(host, button.hosts) && matchesAny(sessionGroup, button.sessionGroups);
}

export function snippetVariables(command: string): string[] {
  return [...command.matchAll(/{{\s*([\w.-]+)\s*}}/g)]
    .map((match) => match[1])
    .filter((name, index, values) => values.indexOf(name) === index);
}

export function resolveSnippetVariables(command: string, values: Record<string, string>): string {
  return command.replace(/{{\s*([\w.-]+)\s*}}/g, (_token, name: string) => values[name] ?? "");
}

/** Decode the common control notation accepted by SecureCRT-style button bars. */
export function decodeControlCharacters(value: string): string {
  return value
    .replace(/\\x([0-9a-f]{2})/gi, (_token, hex: string) => String.fromCharCode(Number.parseInt(hex, 16)))
    .replace(/\\u([0-9a-f]{4})/gi, (_token, hex: string) => String.fromCharCode(Number.parseInt(hex, 16)))
    .replace(/\\e/gi, "\x1b")
    .replace(/\\n/g, "\n")
    .replace(/\\r/g, "\r")
    .replace(/\\t/g, "\t")
    .replace(/\\\\/g, "\\")
    .replace(/\^([@A-Z[\\\]^_])/g, (_token, character: string) => (
      String.fromCharCode(character === "?" ? 127 : character.charCodeAt(0) & 31)
    ));
}

export function buildSnippetPayload(button: QuickButton, values: Record<string, string> = {}): string {
  const decoded = decodeControlCharacters(resolveSnippetVariables(button.command, values));
  if (button.sendMode === "raw") return decoded;
  return decoded.endsWith("\n") ? decoded : `${decoded}\n`;
}
