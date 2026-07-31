using System.Text.Json;
using Newtonsoft.Json.Linq;
using NuGet.Protocol;
using NuGet.Versioning;

if (args.Length != 1 || !Uri.TryCreate(args[0], UriKind.Absolute, out var sourceUri) || sourceUri.Scheme != Uri.UriSchemeHttps)
{
    Console.Error.WriteLine("usage: ServiceIndexOracle HTTPS_SOURCE");
    return 2;
}

using var client = new HttpClient();
var document = await client.GetStringAsync(sourceUri);
var index = new ServiceIndexResourceV3(JObject.Parse(document), DateTime.UtcNow);

var clientVersion = new NuGetVersion(7, 0, 0);
var result = new
{
    registration = index.GetServiceEntryUris(clientVersion, ServiceTypes.RegistrationsBaseUrl).Select(uri => uri.AbsoluteUri),
    packageContent = index.GetServiceEntryUris(clientVersion, ServiceTypes.PackageBaseAddress).Select(uri => uri.AbsoluteUri),
    search = index.GetServiceEntryUris(clientVersion, ServiceTypes.SearchQueryService).Select(uri => uri.AbsoluteUri),
    vulnerability = index.GetServiceEntryUris(clientVersion, "VulnerabilityInfo/6.7.0").Select(uri => uri.AbsoluteUri),
    packagePublish = index.GetServiceEntryUris(clientVersion, ServiceTypes.PackagePublish).Select(uri => uri.AbsoluteUri),
};

Console.WriteLine(JsonSerializer.Serialize(result));
return 0;
