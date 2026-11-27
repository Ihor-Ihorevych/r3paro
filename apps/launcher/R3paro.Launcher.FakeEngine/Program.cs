// Fake hp3 for launcher tests. Usage: fake-hp3 <game folder>; <game folder>/fake.txt holds the mode.
var folder = args[0];
File.WriteAllText(Path.Combine(folder, "arg.txt"), folder);
File.WriteAllText(Path.Combine(folder, "argc.txt"), args.Length.ToString());
var mode = File.ReadAllText(Path.Combine(folder, "fake.txt")).Trim();
switch (mode)
{
    case "fail":
        for (var i = 1; i <= 25; i++) Console.Error.WriteLine($"line {i}");
        Console.Error.WriteLine("boom");
        return 3;
    case "sleep":
        Thread.Sleep(3000);
        return 0;
    default:
        return 0;
}
