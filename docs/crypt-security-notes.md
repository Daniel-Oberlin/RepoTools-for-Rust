# Crypt repository — security notes (for a future phase)

The encrypted ("crypt") repository is **not yet ported** to Rust. This document
captures a security review of the .NET `CryptRepositoryProxy` so that, when the
feature is implemented here, it is a **redesign** rather than a faithful port —
several of the .NET design choices are weak and should not be carried over.

## How the .NET scheme works

References: `RepoTools-for-.NET/RepositorySync/CryptRepositoryProxy.cs`,
`RepoTools-for-.NET/Utilities/CryptUtilities.cs`.

- **Outer repo** = the real plaintext files. The **outer manifest** (real paths,
  sizes, dates, per-file plaintext content hash) is encrypted and stored as a
  blob named `.repositoryManifest.outer` inside the inner repo.
- **Inner repo** = encrypted blobs. Each blob's filename is
  `hex(MD5(MD5(plaintext)))` — a "hash of the hash" (`CryptRepositoryProxy.cs:148-151`).
- **Per-file key** = `PBKDF2-HMAC-SHA1(password, salt = plaintext content hash,
  10000 iters, 32 bytes)` (`CryptUtilities.cs:27-37`, used at `:159-161`).
- **Cipher** = `RijndaelManaged` 256-bit, default **CBC + PKCS7**, with a single
  **hardcoded IV** = `MD5("DoNotChangeThisStringUnderAnyCircumstances")` reused
  for every file and the manifest (`CryptUtilities.cs:13-25, 39-61`).
- **Integrity** = MD5 of the *ciphertext* in the inner manifest; `validateCrypt`
  decrypts and checks the *plaintext* hash against the outer manifest (`:807-854`).

This is essentially **convergent encryption** (key tied to content) layered over
a password-based KDF.

## Findings

### Critical / High

1. **Content-fingerprint leak → confirmation-of-file without the password.**
   Inner blob filenames (`MD5(MD5(plaintext))`) depend only on content. Anyone
   with read access to the inner repo can compute `MD5(MD5(candidate))` for a
   known file and confirm its presence/identity — no key needed. The
   "hash-the-hash" comment (`:144-147`) is obscurity, not protection. Ciphertext
   sizes (≈ plaintext ± one block) and explicit dedup leak further.

2. **Non-random, content-derived salts → cheap offline password cracking.**
   PBKDF2 salt is the content hash (data files) or the inner GUID (manifest) —
   public/recoverable, not random. The inner GUID is stored in plaintext and the
   encrypted outer manifest is JSON beginning with a known crib (`{"Guid":"…`),
   giving a perfect known-plaintext oracle to verify password guesses offline.
   Non-unique salts also allow precomputation shared across repos.

3. **Weak KDF.** PBKDF2-**HMAC-SHA1**, **10,000 iterations** — GPU-friendly and
   far below modern guidance (OWASP ≥600k for PBKDF2-SHA256; prefer Argon2id).

4. **No authenticated encryption.** CBC with no key-bound MAC. The stored MD5 is
   over ciphertext and is itself unauthenticated, so an attacker who can write to
   the inner store can swap a blob and recompute its MD5. CBC is malleable. The
   only real check is `validateCrypt` (on-demand, MD5-based, trusts an outer
   manifest that lives in the same untrusted store).

5. **MD5 everywhere** (content hash, salt, filename, integrity). Chosen-prefix
   MD5 collisions are practical: integrity can be bypassed, and two colliding
   files map to the **same blob name** (dedup at `:154`) → silent shadowing /
   wrong-file retrieval / data loss.

### Medium

6. **No anti-rollback / replay protection.** A party controlling the inner store
   can serve a stale outer manifest or drop blobs undetected. Per-save
   `ChangeGUID()` (`:421`) rotates a salt, not a signed version.

7. **Plaintext in predictable temp files.** Decryption writes plaintext to the
   system temp dir under fixed names (`temp-decrypted-data`, `temp-repository`;
   `:294, 311-314, 960-961`), removed with an ordinary delete (recoverable),
   left behind on crash, and exposed to TOCTOU/symlink races on shared systems.

8. **Key on the command line.** `-sourceKey`/`-destKey` accept the passphrase as
   a CLI arg (visible in `ps`/history); the key lives in a never-zeroed .NET
   `String`.

9. **Fixed IV → fully deterministic ciphertext** for a given (password, content),
   enabling cross-time / cross-repository correlation (detecting reverts, linking
   the same file across repos under one password). Content-derived keys avoid the
   classic same-key/same-IV CBC break *between distinct files*, but not this.

### Low

- `throw e;` (`:782`) drops the stack trace; `RijndaelManaged` is deprecated;
  validation failures are printed but may not propagate a failing exit code.

## What the .NET design got right (keep these)

- Per-file keys bound to **password + content** (one file's key doesn't decrypt
  others).
- The **outer manifest is encrypted**, hiding real names/paths/structure (subject
  to finding 1).
- `validateCrypt` verifies decrypted plaintext against an expected hash.
- Streaming/chunked encryption; space-efficient dedup.

## Recommendations for the Rust implementation

1. **AEAD**: AES-256-GCM or XChaCha20-Poly1305 with a **random per-file nonce**
   stored with the ciphertext — fixes findings 4 and 9 and binds integrity to the
   key.
2. **Random per-file salts** stored with the blob; drop content-derived salts. If
   dedup/convergence is required, document that it inherently leaks equality and
   confirmation-of-file, and derive blob names from a **password-keyed HMAC** (not
   a bare hash) so names stop leaking content fingerprints.
3. **Modern KDF**: Argon2id (or scrypt); derive a master key once, then HKDF for
   per-file subkeys.
4. **SHA-256+** everywhere MD5 is used today.
5. **Authenticate whole-repo state**: a signed/MACed, monotonically-versioned
   manifest to stop rollback/replay/omission.
6. **Secret hygiene**: read the passphrase from prompt/stdin/file/env, never a CLI
   arg; zeroize key material (`zeroize` crate); never write plaintext to a shared
   temp dir — use per-run `tempfile` with mode `0600`, randomized names, best-effort
   secure cleanup.

**Bottom line:** the .NET crypt feature is **obfuscation-grade, not strong
confidentiality** — chiefly due to the content-fingerprint filename leak,
non-secret salts with a weak SHA-1/10k KDF, MD5 throughout, and the absence of
authenticated encryption. The Rust port should adopt the structure (encrypted
manifest, on-decrypt verification, streaming) but replace the cryptography.
