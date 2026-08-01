# Argument forwarding fixture

The .NET 10 oracle prints its managed application argument vector as JSON.
Benchmark setup builds it outside the timed interval; timed Microsoft samples
use `dotnet run --no-build --no-restore` so the forwarded tail is observable.
