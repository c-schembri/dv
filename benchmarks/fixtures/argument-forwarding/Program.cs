using System.Text.Json;

if (args is ["exit", var requestedExit])
{
    return int.Parse(requestedExit);
}

if (args is ["environment"])
{
    Console.WriteLine(JsonSerializer.Serialize(new
    {
        selected = Environment.GetEnvironmentVariable("DV_CLI013_ORACLE"),
        secretPresent = !string.IsNullOrEmpty(Environment.GetEnvironmentVariable("DV_CLI013_TOKEN")),
    }));
}
else
{
    Console.WriteLine(JsonSerializer.Serialize(args));
}

return 0;
