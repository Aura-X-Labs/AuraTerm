import { describe, expect, it } from "vitest";

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
