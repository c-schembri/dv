extern alias JsonAlias;

using JsonConvert = JsonAlias::Newtonsoft.Json.JsonConvert;

Console.WriteLine(JsonConvert.SerializeObject(new { Tool = "dv" }));
