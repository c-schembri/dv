# Online package-signature revocation

## Status

Open compatibility follow-up after `RES-015`.

## Observed contract

NuGet defaults package signing certificate-chain checks to online revocation,
while allowing unavailable or unknown revocation data. A definitively revoked
signing or timestamp certificate remains an error.

## Current boundary

`dv` verifies CMS signatures, package integrity, timestamps, certificate
validity and EKUs, platform-correct trust roots, and `trustedSigners` policy.
The Rust certificate path does not fetch or cache CRLs or OCSP responses, so it
cannot distinguish a known-revoked certificate from one whose revocation state
is unavailable.

## Required evidence

Implement bounded, cached CRL/OCSP retrieval without putting network I/O on the
blocking verifier path. Verify revoked, good, unavailable, offline, and stale
responses against the corresponding NuGet restore outcomes on Windows, Linux,
and macOS before closing this issue.
