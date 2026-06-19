import type { OutputRule } from "./settings";

export interface OutputRuleMatch {
  rule: OutputRule;
  text: string;
  response?: string;
}

export interface OutputRuleResult {
  rendered: string;
  matches: OutputRuleMatch[];
}

const ANSI_PATTERN = /\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-?]*[ -\/]*[@-~]|\x1b./g;
const MAX_TRIGGER_TAIL = 2048;

export function stripTerminalSequences(value: string): string {
  return value.replace(ANSI_PATTERN, "").replace(/\r/g, "");
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function compileRule(rule: OutputRule, global: boolean): RegExp | null {
  if (!rule.pattern) return null;
  try {
    return new RegExp(rule.isRegex ? rule.pattern : escapeRegExp(rule.pattern), `${global ? "g" : ""}${rule.caseSensitive ? "" : "i"}`);
  } catch {
    return null;
  }
}

function wildcardMatches(value: string, pattern: string): boolean {
  try {
    return new RegExp(`^${escapeRegExp(pattern.trim()).replace(/\\\*/g, ".*")}$`, "i").test(value);
  } catch {
    return false;
  }
}

export function ruleAppliesToHost(rule: OutputRule, host?: string): boolean {
  if (rule.scope !== "hosts") return true;
  if (!host) return false;
  return rule.hosts.some((pattern) => wildcardMatches(host, pattern));
}

function rgbSequence(color: string | undefined, background: boolean): string {
  if (!color) return "";
  const match = color.trim().match(/^#([0-9a-f]{6})$/i);
  if (!match) return "";
  const value = Number.parseInt(match[1], 16);
  const r = (value >> 16) & 255;
  const g = (value >> 8) & 255;
  const b = value & 255;
  return `\x1b[${background ? 48 : 38};2;${r};${g};${b}m`;
}

function highlightPlainText(value: string, rules: OutputRule[]): string {
  const ranges: Array<{ start: number; end: number; rule: OutputRule }> = [];
  rules.forEach((rule) => {
    if (!rule.foreground && !rule.background) return;
    const regex = compileRule(rule, true);
    if (!regex) return;
    for (const match of value.matchAll(regex)) {
      if (!match[0] || match.index === undefined) continue;
      ranges.push({ start: match.index, end: match.index + match[0].length, rule });
    }
  });
  ranges.sort((left, right) => left.start - right.start || right.end - left.end);

  let cursor = 0;
  let rendered = "";
  for (const range of ranges) {
    if (range.start < cursor) continue;
    rendered += value.slice(cursor, range.start);
    rendered += rgbSequence(range.rule.foreground, false);
    rendered += rgbSequence(range.rule.background, true);
    rendered += value.slice(range.start, range.end);
    rendered += "\x1b[0m";
    cursor = range.end;
  }
  return rendered + value.slice(cursor);
}

function highlightOutput(value: string, rules: OutputRule[]): string {
  let cursor = 0;
  let rendered = "";
  for (const match of value.matchAll(new RegExp(ANSI_PATTERN.source, "g"))) {
    const index = match.index ?? 0;
    rendered += highlightPlainText(value.slice(cursor, index), rules);
    rendered += match[0];
    cursor = index + match[0].length;
  }
  return rendered + highlightPlainText(value.slice(cursor), rules);
}

function resolveResponse(template: string | undefined, match: RegExpExecArray): string | undefined {
  if (!template) return undefined;
  return template.replace(/\$(\d+)/g, (_token, index: string) => match[Number(index)] ?? "");
}

export class OutputRuleEngine {
  private tail = "";
  private lastTriggered = new Map<string, number>();

  reset() {
    this.tail = "";
    this.lastTriggered.clear();
  }

  process(data: string, rules: OutputRule[], host?: string, now = Date.now()): OutputRuleResult {
    const activeRules = rules.filter((rule) => rule.enabled && ruleAppliesToHost(rule, host));
    const plain = stripTerminalSequences(data);
    const combined = this.tail + plain;
    const boundary = this.tail.length;
    const matches: OutputRuleMatch[] = [];

    for (const rule of activeRules) {
      if (!rule.bell && !rule.notify && !rule.autoResponse) continue;
      const regex = compileRule(rule, true);
      if (!regex) continue;
      for (const match of combined.matchAll(regex)) {
        const end = (match.index ?? 0) + match[0].length;
        if (!match[0] || end <= boundary) continue;
        const previous = this.lastTriggered.get(rule.id) ?? Number.NEGATIVE_INFINITY;
        if (now - previous < rule.cooldownMs) break;
        this.lastTriggered.set(rule.id, now);
        matches.push({ rule, text: match[0], response: resolveResponse(rule.autoResponse, match as RegExpExecArray) });
        break;
      }
    }

    this.tail = combined.slice(-MAX_TRIGGER_TAIL);
    return { rendered: highlightOutput(data, activeRules), matches };
  }
}
