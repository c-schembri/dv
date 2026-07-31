using System.Text.Json;
using NuGet.Common;
using NuGet.Configuration;

if (args.Length != 1 || !Directory.Exists(args[0]))
{
    Console.Error.WriteLine("usage: StoragePolicyOracle WORKING_DIRECTORY");
    return 2;
}

var settings = Settings.LoadDefaultSettings(args[0]);
var result = new
{
    globalPackages = SettingsUtility.GetGlobalPackagesFolder(settings),
    fallbackPackages = SettingsUtility.GetFallbackPackageFolders(settings),
    httpCache = SettingsUtility.GetHttpCacheFolder(),
    scratch = NuGetEnvironment.GetFolderPath(NuGetFolderPath.Temp),
    signatureValidation = SettingsUtility.GetSignatureValidationMode(settings).ToString().ToLowerInvariant(),
    proxy = SettingsUtility.GetConfigValue(settings, ConfigurationConstants.HostKey),
    noProxy = SettingsUtility.GetConfigValue(settings, ConfigurationConstants.NoProxy),
};

Console.WriteLine(JsonSerializer.Serialize(result));
return 0;
