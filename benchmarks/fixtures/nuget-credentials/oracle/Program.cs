using System.Text.Json;
using NuGet.Configuration;

if (args.Length != 1)
{
    Console.Error.WriteLine("usage: CredentialOracle ROOT");
    return 2;
}

var root = Path.GetFullPath(args[0]);
var settings = Settings.LoadSpecificSettings(root, "NuGet.Config");
var sources = new PackageSourceProvider(settings).LoadPackageSources().Select(source =>
{
    var credential = source.Credentials;
    var types = credential?.ValidAuthenticationTypes.ToArray() ?? [];
    var basic = credential is not null
        && credential.IsValid()
        && (types.Length == 0 || types.Any(type => type.Equals("basic", StringComparison.OrdinalIgnoreCase)));
    return new
    {
        name = source.Name,
        location = source.Source,
        protocol = source.ProtocolVersion == 3 ? "v3" : "v2",
        authentication = basic ? "basic" : "none",
        credentialSelected = source.Name == "private"
            ? credential?.Username == "environment-user" && credential.Password == "environment-pat"
            : credential?.Username == "config-only-user" && credential.Password == "config-only-pat",
    };
});

Console.WriteLine(JsonSerializer.Serialize(sources));
return 0;
