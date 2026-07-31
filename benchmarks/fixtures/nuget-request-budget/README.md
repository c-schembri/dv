# NuGet request-budget fixture

The benchmark harness seeds this six-package graph from NuGet.org, then serves
the resulting packages from two delayed local V3 feeds. `NuGet.Config` is
generated with exact source mappings and `maxHttpRequestsPerSource=2`; both
tools run with `NUGET_CONCURRENCY_LIMIT=4`.

The server rejects a sample if either source exceeds two active requests or
the combined feeds exceed four. The delay makes concurrent request scheduling
observable without measuring the public network.
