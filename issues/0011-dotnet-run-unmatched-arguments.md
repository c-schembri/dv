# dotnet run unmatched arguments

The .NET 10 `dotnet run` parser can treat an otherwise unknown dash-prefixed
token as an application argument even when it appears before an explicit `--`.
For example, the selected `10.0.100` SDK executed the benchmark fixture for
`dotnet run --project SmallConsole.csproj --definitely-unknown` and returned
success instead of an option error.

`DROP-022` deliberately rejects such tokens while native run execution remains
unimplemented. This is explicit partial compatibility, not a silent no-op.
Before implementing the run workflow:

- capture unmatched-token behavior across every supported SDK band;
- distinguish driver options from implicit application arguments without
  guessing from spelling alone;
- retain exact OS tokens and ordering in the typed child argument batch;
- verify the explicit `--` boundary, empty tokens, non-Unicode tokens, and
  option-looking application values;
- add like-for-like execution and forwarding benchmarks.

The issue closes only when the run parser reproduces the selected SDK's
unmatched-token contract or rejects the relevant SDK/profile as unsupported.
