using System.Text.Json;
using NuGet.Configuration;

if (args.Length != 1 || !Directory.Exists(args[0]))
{
    Console.Error.WriteLine("usage: SecurityOracle WORKING_DIRECTORY");
    return 2;
}

var settings = Settings.LoadDefaultSettings(args[0]);
var sources = new PackageSourceProvider(settings)
    .LoadPackageSources()
    .Where(source => source.IsEnabled)
    .Select(source => new
    {
        name = source.Name,
        location = source.Source,
        protocol = $"v{source.ProtocolVersion}",
        allowInsecureConnections = source.AllowInsecureConnections,
        disableTlsCertificateValidation = source.DisableTLSCertificateValidation,
    });

Console.WriteLine(JsonSerializer.Serialize(sources));
return 0;
