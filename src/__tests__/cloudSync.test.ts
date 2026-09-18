import { describe, expect, it, vi } from "vitest";
import { classifySyncError, syncNowConfirmingDeletes, type SyncResult } from "../cloudSync";

describe("classifySyncError (mirrors cloud_sync.rs ERR_* texts)", () => {
  it("recognises the states the UI acts on", () => {
    expect(classifySyncError("Sign in to your AuraXLab account again — the saved credential is no longer valid.")).toBe("signIn");
    expect(classifySyncError("Sign in to your AuraXLab account first.")).toBe("notSignedIn");
    expect(classifySyncError(new Error("The cloud copy still uses the old sync passphrase format; migrate it once from Sync settings.")))
      .toBe("legacyVault");
  });
  it("leaves everything else alone", () => {
    expect(classifySyncError("Network error: timeout")).toBe("other");
    expect(classifySyncError("The server has newer data than this device. Pull first, then push again.")).toBe("other");
    expect(classifySyncError(undefined)).toBe("other");
  });
});

import {
  SYNC_EMAIL_RE,
  SYNC_USERNAME_RE,
  SYNC_MIN_PASSWORD_LENGTH,
  validateRegistration,
} from "../cloudSync";

// These rules must stay identical to the AuraXLab server's register endpoint
// (app/api/sync.py: auraterm_sync_register). This suite locks the client side.
describe("validateRegistration (mirrors AuraXLab server rules)", () => {
  it("accepts a valid registration", () => {
    expect(validateRegistration("alice@example.com", "alice", "password123")).toBeNull();
  });

  it("rejects a password shorter than 8 characters", () => {
    expect(validateRegistration("alice@example.com", "alice", "short")).toBe(
      "Password must be at least 8 characters",
    );
    // exactly 8 is allowed
    expect(validateRegistration("alice@example.com", "alice", "12345678")).toBeNull();
  });

  it("does not trim the password (spaces count toward length)", () => {
    expect(validateRegistration("alice@example.com", "alice", "  6chars")).toBeNull();
    expect(validateRegistration("alice@example.com", "alice", "  3   ")).toBe(
      "Password must be at least 8 characters",
    );
  });

  it("rejects a malformed email", () => {
    for (const bad of ["not-an-email", "a@b", "a b@c.com", "@c.com", "a@.com"]) {
      expect(validateRegistration(bad, "alice", "password123")).toBe(
        "A valid email address is required",
      );
    }
    expect(validateRegistration("  alice@example.com  ", "alice", "password123")).toBeNull();
  });

  it("rejects a username that does not start with a letter or has bad chars", () => {
    for (const bad of ["1bob", "_bob", ".bob", "bo b", "b-o-b", ""]) {
      expect(validateRegistration("alice@example.com", bad, "password123")).toBe(
        "Username must start with a letter and contain only letters, numbers, dots or underscores",
      );
    }
    for (const ok of ["alice", "Bob_99", "a.b.c", "X"]) {
      expect(validateRegistration("alice@example.com", ok, "password123")).toBeNull();
    }
  });

  it("exposes the same constants the server uses", () => {
    expect(SYNC_MIN_PASSWORD_LENGTH).toBe(8);
    expect(SYNC_EMAIL_RE.source).toBe("^[^@\\s]+@[^@\\s]+\\.[^@\\s]+$");
    expect(SYNC_USERNAME_RE.source).toBe("^[A-Za-z][A-Za-z0-9_.]*$");
  });
});

describe("syncNowConfirmingDeletes (manual run vs. the mass-delete guard)", () => {
  const result = (deletesHeld: SyncResult["deletesHeld"]): SyncResult => ({
    pushed: true,
    pulled: true,
    bookmarksTotal: 12,
    bookmarksAdded: 0,
    bookmarksUpdated: 0,
    bookmarksRemoved: 0,
    conflicts: [],
    deletesHeld,
    knownHostsAdded: 0,
    credentialsSynced: 0,
    credentialsSkipped: null,
    settingsApplied: false,
    remoteVersion: "3",
    message: "Two-way sync complete.",
  });

  it("does not ask when nothing was held back", async () => {
    const run = vi.fn().mockResolvedValue(result(null));
    const confirm = vi.fn();
    await syncNowConfirmingDeletes(confirm, run);
    expect(run).toHaveBeenCalledTimes(1);
    expect(confirm).not.toHaveBeenCalled();
  });

  it("runs once more with the confirmation after a yes", async () => {
    const held = { local: 0, remote: 9 };
    const run = vi.fn().mockResolvedValueOnce(result(held)).mockResolvedValueOnce(result(null));
    const confirm = vi.fn().mockResolvedValue(true);
    const final = await syncNowConfirmingDeletes(confirm, run);
    expect(confirm).toHaveBeenCalledWith(held);
    expect(run).toHaveBeenNthCalledWith(2, true);
    expect(final.deletesHeld).toBeNull();
  });

  it("keeps the held result after a no", async () => {
    const held = { local: 10, remote: 0 };
    const run = vi.fn().mockResolvedValue(result(held));
    const final = await syncNowConfirmingDeletes(async () => false, run);
    expect(run).toHaveBeenCalledTimes(1);
    expect(final.deletesHeld).toEqual(held);
  });
});
