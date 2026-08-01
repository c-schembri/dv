using Humanizer;
using Newtonsoft.Json;

Console.WriteLine(JsonConvert.SerializeObject(new { Message = "central package management".Humanize() }));
