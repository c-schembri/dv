# Framework Reference Fixture

This .NET 10 console project references `Microsoft.AspNetCore.App` so framework
planning must resolve both the implicit `Microsoft.NETCore.App` framework and
the explicit ASP.NET Core framework. `LatestPatch` exercises installed shared
framework selection without changing the SDK-selected minimum versions.
