import { reactive } from "vue";

/**
 * Import review plumbing.
 *
 * An import used to be one call that parsed, deduplicated and wrote in a single
 * breath — the user clicked a file and it was already in the library. Once a
 * bookmark file can arrive from someone else, it is *external input*: what it
 * contains, what it collides with, and what it would run on connect all have to
 * be visible before anything touches disk.
 *
 * Mirrors the types in `connections.rs`. The parsed payload stays in Rust and is
 * referenced by `planId`; nothing here carries credentials.
 */

export type ImportAction = "add" | "update" | "skip";

export interface ImportPlanEntry {
  index: number;
  name: string;
  group?: string | null;
  protocol: string;
  target: string;
  disposition: ImportAction;
  matchedName?: string | null;
  /** `origin` = the same entry of the same share; `endpoint` = the same machine. */
  matchedBy?: "origin" | "endpoint" | null;
}

/** How many entries carry something that runs or authenticates on connect. */
export interface ImportRisks {
  postConnectCommands: number;
  autoLoginResponses: number;
  jumpHostCredentials: number;
  passwords: number;
  privateKeys: number;
}

export interface ShareMeta {
  kind: string;
  rootName: string;
  label?: string | null;
  note?: string | null;
  bundleId: string;
  emptyGroups: string[];
  sourceLabel?: string | null;
}

export interface ImportPlan {
  planId: string;
  format: string;
  /** The landing group in effect; empty means "keep what the file says". */
  group: string;
  share?: ShareMeta | null;
  entries: ImportPlanEntry[];
  emptyGroups: string[];
  risks: ImportRisks;
  warnings: string[];
}

export interface ImportDecision {
  index: number;
  action: ImportAction;
}

export interface ImportTrust {
  allowCommands: boolean;
  allowCredentials: boolean;
}

/** The dialog's verdict; null when the user backed out. */
export interface ImportReview {
  group: string;
  decisions: ImportDecision[];
  trust: ImportTrust;
}

export interface ImportReviewRequest {
  plan: ImportPlan;
  resolve: (review: ImportReview | null) => void;
}

/** FIFO of open reviews; ImportPreviewHost renders the head. */
export const importReviewQueue = reactive<ImportReviewRequest[]>([]);

/** Show the plan and wait for the user's decisions. */
export function reviewImport(plan: ImportPlan): Promise<ImportReview | null> {
  return new Promise((resolve) => {
    importReviewQueue.push({ plan, resolve });
  });
}

/** Entries that would auto-run or auto-authenticate — the trust gate's subject. */
export function commandRisks(risks: ImportRisks): number {
  return risks.postConnectCommands + risks.autoLoginResponses;
}

export function credentialRisks(risks: ImportRisks): number {
  return risks.jumpHostCredentials + risks.passwords + risks.privateKeys;
}

export function hasRisks(risks: ImportRisks): boolean {
  return commandRisks(risks) + credentialRisks(risks) > 0;
}

/** Tally of what the plan will do, for the dialog's summary line. */
export function countActions(
  entries: readonly ImportPlanEntry[],
  actions: ReadonlyMap<number, ImportAction>,
): Record<ImportAction, number> {
  const totals: Record<ImportAction, number> = { add: 0, update: 0, skip: 0 };
  for (const entry of entries) {
    totals[actions.get(entry.index) ?? entry.disposition] += 1;
  }
  return totals;
}
