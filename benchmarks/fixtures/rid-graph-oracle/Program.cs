using NuGet.RuntimeModel;

if (args.Length != 1 || args[0].Length == 0)
{
    Console.Error.WriteLine("usage: RidGraphOracle RID");
    return 2;
}

var path = Path.Combine(AppContext.BaseDirectory, "PortableRuntimeIdentifierGraph.json");
var graph = JsonRuntimeFormat.ReadRuntimeGraph(path);
foreach (var compatible in graph.ExpandRuntime(args[0]))
{
    Console.WriteLine(compatible);
}

return 0;
