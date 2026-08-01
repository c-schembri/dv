# Package signature validation fixture

This fixture restores one repository-signed package from a local source with
`signatureValidationMode=require`. It isolates archive hashing, CMS/RSA
verification, timestamp validation, platform certificate-chain validation,
and the NuGet trusted-signers allow list from network latency.

`MessagePack.Annotations` 2.5.192 is MIT licensed and repository-signed by
NuGet.org. The package is retained unchanged in `source` as the shared oracle
input for both Microsoft restore and `dv restore`.
