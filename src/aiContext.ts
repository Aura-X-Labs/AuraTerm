/**
 * Context extraction for the AI assistant: pure helpers that turn a shell
 * command block (command + captured output + exit code) into the user-visible
 * prompt sent to the model. Kept free of xterm/Tauri imports so the trimming
 * and prompt-building logic is unit-testable.
 */

import type { Locale } from "./i18n";

export interface CommandContext {
  command: string;
  /** Plain-text output captured between the command's start/end markers. */
  output: string;
  exitCode?: number;
  /** Shell path or session kind (ssh/telnet/serial) for environment hints. */
  shell?: string;
  os?: string;
}

/**
 * Free-form terminal content not tied to a shell-integration command block —
 * e.g. the lines currently visible in the viewport. Used for "summarize what
 * I'm looking at" style actions that must work without OSC 133 markers.
 */
export interface OutputContext {
  output: string;
  /** Shell path or session kind (ssh/telnet/serial) for environment hints. */
  shell?: string;
  os?: string;
}

/**
 * Soft cap for the output portion of a prompt. Measured in UTF-16 code units,
 * which tracks bytes closely enough for a guardrail against runaway prompts.
 */
export const MAX_OUTPUT_CHARS = 8 * 1024;

const TRUNCATION_HEAD_RATIO = 0.4;
const TRUNCATION_TAIL_RATIO = 0.5;

/**
 * Trim oversized output keeping both head and tail: the echoed command line
 * and early errors live at the top, the final error/stack usually at the
 * bottom. Middle lines are replaced with an explicit truncation marker so the
 * model knows content is missing.
 */
export function trimOutput(output: string, maxChars: number = MAX_OUTPUT_CHARS): string {
  if (output.length <= maxChars) return output;

  const headBudget = Math.floor(maxChars * TRUNCATION_HEAD_RATIO);
  const tailBudget = Math.floor(maxChars * TRUNCATION_TAIL_RATIO);
  const lines = output.split("\n");

  const head: string[] = [];
  let used = 0;
  let headEnd = 0;
  while (headEnd < lines.length) {
    const cost = lines[headEnd].length + 1;
    if (used + cost > headBudget) break;
    head.push(lines[headEnd]);
    used += cost;
    headEnd++;
  }

  const tail: string[] = [];
  used = 0;
  let tailStart = lines.length;
  while (tailStart - 1 > headEnd) {
    const cost = lines[tailStart - 1].length + 1;
    if (used + cost > tailBudget) break;
    tail.unshift(lines[tailStart - 1]);
    used += cost;
    tailStart--;
  }

  const omitted = tailStart - headEnd;
  if (head.length === 0 && tail.length === 0) {
    // Degenerate case: a single line larger than both budgets. Fall back to a
    // character-based head+tail slice.
    return `${output.slice(0, headBudget)}\n… [output truncated] …\n${output.slice(-tailBudget)}`;
  }
  if (omitted <= 0) return output;

  return [...head, `… [${omitted} lines truncated] …`, ...tail].join("\n");
}

function replyLanguageInstruction(locale: Locale): string {
  return locale === "zh-CN" ? "请用简体中文回答。" : "Respond in English.";
}

function envLine(ctx: { shell?: string; os?: string }): string | null {
  const env: string[] = [];
  if (ctx.os) env.push(`OS: ${ctx.os}`);
  if (ctx.shell) env.push(`Shell/session: ${ctx.shell}`);
  return env.length ? env.join("; ") : null;
}

/**
 * Build the user message for "explain this command block". The context data is
 * fenced so the model cannot confuse terminal content with instructions.
 */
export function buildExplainPrompt(ctx: CommandContext, locale: Locale): string {
  const failed = ctx.exitCode !== undefined && ctx.exitCode !== 0;
  const parts: string[] = [];

  parts.push(
    failed
      ? "The following terminal command failed. Explain what went wrong and how to fix it."
      : "Explain the following terminal command and its output.",
  );

  const env = envLine(ctx);
  if (env) parts.push(env);

  parts.push(`Command:\n\`\`\`\n${ctx.command}\n\`\`\``);
  if (ctx.exitCode !== undefined) parts.push(`Exit code: ${ctx.exitCode}`);

  const output = trimOutput(ctx.output).trim();
  parts.push(output ? `Output:\n\`\`\`\n${output}\n\`\`\`` : "Output: (empty)");

  parts.push(replyLanguageInstruction(locale));
  return parts.join("\n\n");
}

/**
 * Build the user message for "suggest a better way to write this command".
 * Reuses the same command-block context as buildExplainPrompt; the output is
 * included so the model can judge whether the command achieved its intent.
 */
export function buildOptimizePrompt(ctx: CommandContext, locale: Locale): string {
  const parts: string[] = [];

  parts.push(
    "Suggest a better way to write the following terminal command: more idiomatic, safer, or more efficient. Show the improved command in a code block and briefly explain what you changed and why. If the command is already fine, say so.",
  );

  const env = envLine(ctx);
  if (env) parts.push(env);

  parts.push(`Command:\n\`\`\`\n${ctx.command}\n\`\`\``);
  if (ctx.exitCode !== undefined) parts.push(`Exit code: ${ctx.exitCode}`);

  const output = trimOutput(ctx.output).trim();
  parts.push(output ? `Output:\n\`\`\`\n${output}\n\`\`\`` : "Output: (empty)");

  parts.push(replyLanguageInstruction(locale));
  return parts.join("\n\n");
}

/**
 * Build the user message for "summarize this terminal output". Works on raw
 * viewport content, so it needs no shell-integration command block.
 */
export function buildSummarizePrompt(ctx: OutputContext, locale: Locale): string {
  const parts: string[] = [];

  parts.push(
    "Summarize the following terminal output: the key results, any errors or warnings, and anything that needs attention. Be concise.",
  );

  const env = envLine(ctx);
  if (env) parts.push(env);

  const output = trimOutput(ctx.output).trim();
  parts.push(output ? `Output:\n\`\`\`\n${output}\n\`\`\`` : "Output: (empty)");

  parts.push(replyLanguageInstruction(locale));
  return parts.join("\n\n");
}
