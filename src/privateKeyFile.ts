/** Largest file accepted as a private key. Real keys are a few KB (a 16384-bit
 *  RSA key is ~13 KB); the cap keeps a mis-picked file out of memory and out of
 *  the credential store. */
export const MAX_PRIVATE_KEY_FILE_BYTES = 64 * 1024;

export type PrivateKeyFileProblem = "tooLarge" | "publicKey" | "notAKey";

const PEM_PRIVATE_KEY = /-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----/;
const PEM_PUBLIC_KEY = /-----BEGIN [A-Z0-9 ]*PUBLIC KEY-----/;
/** `ssh-ed25519 AAAA…`, `ecdsa-sha2-nistp256 …`, `sk-ssh-ed25519@openssh.com …` */
const OPENSSH_PUBLIC_KEY = /^(ssh-|ecdsa-sha2-|sk-)\S+\s+AAAA/;

/** Why `content` cannot be a private key the backend would decode, or `null`.
 *
 *  Mirrors what `russh::keys::decode_secret_key` accepts — PEM-armoured
 *  OpenSSH / PKCS#1 / PKCS#8 / SEC1 keys and PuTTY `.ppk` files — so a wrong
 *  pick is caught here instead of as a parse error at connect time. Whether the
 *  key is *valid* (or its passphrase right) is still the backend's call. */
export function privateKeyProblem(content: string): PrivateKeyFileProblem | null {
  const text = content.trim();
  if (text.startsWith("PuTTY-User-Key-File-") || PEM_PRIVATE_KEY.test(text)) {
    return null;
  }
  if (OPENSSH_PUBLIC_KEY.test(text) || PEM_PUBLIC_KEY.test(text)) {
    return "publicKey";
  }
  return "notAKey";
}
