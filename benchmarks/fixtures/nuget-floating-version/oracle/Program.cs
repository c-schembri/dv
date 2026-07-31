using NuGet.Versioning;

foreach (var text in args)
{
    if (!VersionRange.TryParse(text, out var range))
    {
        Console.WriteLine($"{text}|invalid");
        continue;
    }

    Console.WriteLine(
        $"{text}|{range.MinVersion?.ToNormalizedString()}|{range.MaxVersion?.ToNormalizedString()}|{range.Float?.FloatBehavior}|{range.Float?.OriginalReleasePrefix}");
}
