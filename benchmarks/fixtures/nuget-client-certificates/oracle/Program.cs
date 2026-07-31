using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;
using System.Text.Json;
using NuGet.Configuration;

if (args.Length != 2)
{
    Console.Error.WriteLine("usage: ClientCertificateOracle setup|query|cleanup ROOT");
    return 2;
}

var operation = args[0];
var root = Path.GetFullPath(args[1]);
var certRoot = Path.Combine(root, "certs");
var metadataPath = Path.Combine(certRoot, "metadata.json");

switch (operation)
{
    case "setup":
        Directory.CreateDirectory(certRoot);
        using (var clientKey = RSA.Create(2048))
        {
            var request = new CertificateRequest("CN=dv benchmark client", clientKey, HashAlgorithmName.SHA256, RSASignaturePadding.Pkcs1);
            request.CertificateExtensions.Add(new X509BasicConstraintsExtension(false, false, 0, true));
            request.CertificateExtensions.Add(new X509KeyUsageExtension(X509KeyUsageFlags.DigitalSignature, true));
            request.CertificateExtensions.Add(new X509EnhancedKeyUsageExtension([new Oid("1.3.6.1.5.5.7.3.2")], true));
            using var certificate = request.CreateSelfSigned(DateTimeOffset.UtcNow.AddMinutes(-5), DateTimeOffset.UtcNow.AddDays(1));
            var pfxPath = Path.Combine(certRoot, "client.pfx");
            File.WriteAllBytes(pfxPath, certificate.Export(X509ContentType.Pfx, "fixture-client-password"));
            File.WriteAllText(metadataPath, JsonSerializer.Serialize(new { client = certificate.Thumbprint }));
            using var persisted = X509CertificateLoader.LoadPkcs12FromFile(
                pfxPath,
                "fixture-client-password",
                X509KeyStorageFlags.UserKeySet | X509KeyStorageFlags.PersistKeySet | X509KeyStorageFlags.Exportable);
            AddToStore(persisted);
        }
        return 0;

    case "cleanup":
        if (File.Exists(metadataPath))
        {
            using var metadata = JsonDocument.Parse(File.ReadAllText(metadataPath));
            RemoveFromStore(metadata.RootElement.GetProperty("client").GetString()!);
        }
        return 0;

    case "query":
        var settings = Settings.LoadSpecificSettings(root, "NuGet.Config");
        var results = new PackageSourceProvider(settings).LoadPackageSources().Select(source =>
        {
            var certificates = source.ClientCertificates?.ToArray() ?? [];
            return new
            {
                name = source.Name,
                location = source.Source,
                protocol = source.ProtocolVersion == 3 ? "v3" : "v2",
                authentication = certificates.Length == 0 ? "none" : "client_certificate",
                certificateCount = certificates.Length,
            };
        });
        Console.WriteLine(JsonSerializer.Serialize(results));
        return 0;

    default:
        Console.Error.WriteLine($"unknown operation {operation}");
        return 2;
}

static void AddToStore(X509Certificate2 certificate)
{
    using var store = new X509Store(StoreName.My, StoreLocation.CurrentUser);
    store.Open(OpenFlags.ReadWrite);
    store.Add(certificate);
}

static void RemoveFromStore(string thumbprint)
{
    using var store = new X509Store(StoreName.My, StoreLocation.CurrentUser);
    store.Open(OpenFlags.ReadWrite);
    foreach (var certificate in store.Certificates.Find(X509FindType.FindByThumbprint, thumbprint, false))
    {
        store.Remove(certificate);
    }
}
