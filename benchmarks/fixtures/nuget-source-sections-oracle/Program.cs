using System.Text.Json;
using NuGet.Configuration;

if (args.Length != 1 || !Directory.Exists(args[0]))
{
    Console.Error.WriteLine("usage: SourceSectionsOracle WORKING_DIRECTORY");
    return 2;
}

var settings = Settings.LoadDefaultSettings(args[0]);
var provider = new PackageSourceProvider(settings);
var mapping = PackageSourceMapping.GetPackageSourceMapping(settings);
var result = new
{
    packageSources = provider.LoadPackageSources().Select(source => new
    {
        name = source.Name,
        url = source.Source,
        enabled = source.IsEnabled,
        protocol = source.ProtocolVersion,
    }),
    auditSources = provider.LoadAuditSources().Select(source => new
    {
        name = source.Name,
        url = source.Source,
        protocol = source.ProtocolVersion,
    }),
    mappings = new
    {
        newtonsoft = mapping.GetConfiguredPackageSources("Newtonsoft.Json"),
        decoy = mapping.GetConfiguredPackageSources("Decoy.Widget"),
        legacy = mapping.GetConfiguredPackageSources("Legacy.Widget"),
        cleared = mapping.GetConfiguredPackageSources("Company.Core"),
    },
};

Console.WriteLine(JsonSerializer.Serialize(result));
return 0;
