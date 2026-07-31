# Massive package graph fixture

This `net10.0` restore-only workload combines 51 direct package references
used by Microsoft's eShop reference application at commit
`9b4f9434f46fdc5c1a6e9e936af2868340cdbc48`: Aspire hosting and components,
ASP.NET Core, EF Core and PostgreSQL, service discovery and resilience,
OpenTelemetry, Duende IdentityServer, gRPC, validation, mediation, and test
infrastructure.

The union is intentionally represented as one project because `dv` does not
yet resolve a complete solution/project-reference closure as one package
batch. It measures a real-solution-sized NuGet graph without claiming
project-graph restore parity.

On 2026-07-31, .NET SDK `10.0.100` selected 203 packages and populated 272
package archives totaling 197,860,237 bytes in a fresh isolated package
directory. The benchmark passes `-p:NuGetAudit=false` to keep vulnerability
service access outside the timed package-graph transform. This fixture is
performance evidence, not a dependency-security recommendation.
