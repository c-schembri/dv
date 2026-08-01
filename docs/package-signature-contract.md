# Package Signature Verification Contract

This document defines the `RES-015` transform implemented by
`package_signature::verify_package`. Restore schedules this private transform
once for each completed package download or required-policy cache hit. A call
is deliberately a singleton: it owns one archive file handle, while the
existing package batch scheduler provides bounded package-level concurrency.

## Input And Output

Input is an existing `.nupkg` path plus an immutable `SignaturePolicy` shared
for the restore lifetime. The policy contains `accept` or `require`, a compact
trusted-signer batch, and the selected SDK root on non-Windows systems. The
archive is external variable-sized data. Signed input must be a single-disk
ZIP32 archive with exactly one uncompressed `.signature.p7s` entry no larger
than 16 MiB.

Output is `Ok(true)` for a verified signed package, `Ok(false)` only for an
unsigned package under `accept`, or a typed integrity, configuration, or I/O
error. The caller retains the path and policy. File handles and parse storage
live only for the call; lazily loaded trust anchors live with the policy and
are reused across the restore.

Malformed ZIP bounds, duplicate signature entries, unsupported CMS forms,
invalid hashes or signatures, mismatched identities, invalid timestamps,
untrusted chains, and unmatched required signers are rejected before package
extraction or atomic publication. `require` rejects unsigned input. No invalid
or out-of-range value is clamped or dropped.

## Transform

1. Read at most 65,557 footer bytes, find the ZIP end record, and validate
   single-disk ZIP32 bounds.
2. Scan the central directory linearly into contiguous 56-byte records. Each
   record has 11 fixed fields, 8-byte alignment, and a 56-byte retained working
   set. On the assumed 64-byte benchmark cache line, one complete record fits
   per line; array traversal is linear even where successive records straddle
   lines. Total record storage is `56 * entry_count` bytes.
3. Read only a 14-byte candidate name into a stack buffer; skip all other ZIP
   names without allocating. Locate and validate the signature local record.
4. Allocate the externally sized CMS payload once, bounded at 16 MiB. Parse its
   certificate and signer batches, then verify signed attributes and RSA
   SHA-256/384/512 signatures.
5. Reconstruct the unsigned ZIP byte stream into a SHA-256/384/512 hasher with
   one reused 64 KiB stack buffer. Archive content is never copied into a
   second full-package buffer.
6. Verify author or repository commitment, repository countersignatures,
   signing-certificate attributes, and RFC 3161 timestamp imprints.
7. Under `require`, scan the normally small trusted-signer and certificate
   arrays linearly, then validate code-signing and timestamp chains. Windows
   uses native roots. Linux uses its object-signing bundle when present and
   otherwise the selected SDK bundle; other non-Windows systems use the SDK
   bundles. Root parsing happens once per policy.

The central-directory and hashing passes are linear with predictable loop
bounds. CMS field presence and malformed-input checks branch per structure but
sit outside the byte hashing loop. Trusted-signer matching is a linear scan;
adding a hash table would add startup allocation and random access for the
observed small policy batches.

## Cost And Simplification

Verification launches no process and performs no network request. A signed
archive is read for ZIP metadata, its bounded CMS entry, and one streaming
unsigned-content hash. Dynamic storage is limited to variable-sized external
ZIP records, CMS DER/certificates, trust roots, and digest output. Fixed ZIP
headers, candidate names, and the hash buffer use stack storage.

The simplification pass removed a per-entry filename allocation, a full
archive buffer, a separate signature subprocess, and repeated root loading.
The verifier does not match repository `serviceIndex` during allow-list
selection because NuGet preserves that configuration value but matches trust
by fingerprint, placement, and owner.

Online CRL and OCSP retrieval is intentionally outside this blocking transform
and remains tracked in [signature-revocation.md](../issues/signature-revocation.md).
Its absence is compatibility work, not a claim that revocation succeeds.

## Evidence

Correctness fixtures cover official author and repository signatures,
timestamps, unsigned packages, byte tampering, owner case sensitivity,
fingerprint algorithms, restrictive `allowUntrustedRoot` merge behavior, and
trusted-signer hierarchy merge. Cold and warm paired restore benchmarks use the
same signed archive, fingerprint policy, and zero timed HTTP requests for both
tools. A design failure is any accepted tamper or policy mismatch, a published
package before verification, nondeterministic result ordering, or a regression
outside the retained benchmark distribution.
