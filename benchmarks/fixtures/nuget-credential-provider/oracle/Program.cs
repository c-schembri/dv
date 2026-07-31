using System.Text.Json;
using NuGet.Protocol.Plugins;

if (args.Length != 1 || !Uri.TryCreate(args[0], UriKind.Absolute, out var source))
{
    Console.Error.WriteLine("usage: CredentialProviderOracle SOURCE");
    return 2;
}

var providers = await PluginManager.Instance.FindAvailablePluginsAsync(CancellationToken.None);
var discovered = 0;
foreach (var provider in providers)
{
    discovered++;
    var created = await PluginManager.Instance.TryGetSourceAgnosticPluginAsync(
        provider,
        OperationClaim.Authentication,
        CancellationToken.None);
    if (!created.Item1 || created.Item2?.Plugin is null)
    {
        continue;
    }

    var response = await created.Item2.Plugin.Connection.SendRequestAndReceiveResponseAsync<
        GetAuthenticationCredentialsRequest,
        GetAuthenticationCredentialsResponse>(
        MessageMethod.GetAuthenticationCredentials,
        new GetAuthenticationCredentialsRequest(source, isRetry: false, isNonInteractive: true, canShowDialog: false),
        CancellationToken.None);
    var basic = response.ResponseCode == MessageResponseCode.Success
        && response.IsValid()
        && response.AuthenticationTypes?.Any(type => type.Equals("Basic", StringComparison.OrdinalIgnoreCase)) == true;
    Console.WriteLine(JsonSerializer.Serialize(new { authentication = basic ? "basic" : "none", selected = basic, providerCount = discovered }));
    return basic ? 0 : 1;
}

Console.WriteLine(JsonSerializer.Serialize(new { authentication = "none", selected = false, providerCount = discovered }));
return 1;
