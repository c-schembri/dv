using System.Reflection;
using System.Text.Json;
using NuGet.Common;
using NuGet.Configuration;
using NuGet.Protocol;

if (args.Length != 1 || !Directory.Exists(args[0]))
{
    Console.Error.WriteLine("usage: HttpPolicyOracle WORKING_DIRECTORY");
    return 2;
}

var settings = Settings.LoadDefaultSettings(args[0]);
var proxy = new ProxyCache(settings, EnvironmentVariableWrapper.Instance).GetUserConfiguredProxy();
var retryHandler = new HttpRetryHandler();
var helper = typeof(HttpRetryHandler)
    .GetField("_enhancedHttpRetryHelper", BindingFlags.Instance | BindingFlags.NonPublic)!
    .GetValue(retryHandler)!;

object Read(string name) => helper.GetType()
    .GetProperty(name, BindingFlags.Instance | BindingFlags.NonPublic)!
    .GetValue(helper)!;

var result = new
{
    maxTries = (int)Read("RetryCountOrDefault"),
    retryDelayMs = (int)Read("DelayInMillisecondsOrDefault"),
    maxRetryAfterSeconds = (int)((TimeSpan)Read("MaxRetryAfterDelayOrDefault")).TotalSeconds,
    requestTimeoutSeconds = (int)HttpSourceRequest.DefaultRequestTimeout.TotalSeconds,
    downloadTimeoutSeconds = (int)HttpRetryHandlerRequest.DefaultDownloadTimeout.TotalSeconds,
    maxRequestsPerSource = SettingsUtility.GetMaxHttpRequest(settings),
    retryHttp429 = (bool)Read("Retry429OrDefault"),
    observeRetryAfter = (bool)Read("ObserveRetryAfterOrDefault"),
    proxyConfigured = proxy is not null,
    proxyAuthenticated = proxy?.Credentials is not null,
    noProxyConfigured = proxy?.BypassList.Count > 0,
};

Console.WriteLine(JsonSerializer.Serialize(result));
return 0;
