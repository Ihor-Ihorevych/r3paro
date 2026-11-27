using System.Diagnostics;
using R3paro.Launcher.Services;

namespace R3paro.Launcher.Tests;

internal sealed class FakeSettings(LauncherSettings value) : ISettingsStore
{
    public LauncherSettings Value { get; private set; } = value;
    public int Saves { get; private set; }
    public LauncherSettings Load() => Value;
    public void Save(LauncherSettings settings) { Value = settings; Saves++; }
}

public sealed class GameLauncherTests : IDisposable
{
    private readonly string _root = Path.Combine(Path.GetTempPath(), "r3paro-run-" + Guid.NewGuid().ToString("N"));

    private static readonly string FakeEngine =
        Path.Combine(AppContext.BaseDirectory, OperatingSystem.IsWindows() ? "fake-hp3.exe" : "fake-hp3");

    public GameLauncherTests() => Directory.CreateDirectory(_root);

    public void Dispose()
    {
        try { Directory.Delete(_root, recursive: true); } catch (IOException) { }
    }

    private string Game(string mode, string name = "HP3")
    {
        var dir = Path.Combine(_root, name);
        Directory.CreateDirectory(dir);
        File.WriteAllText(Path.Combine(dir, "fake.txt"), mode);
        return dir;
    }

    private static GameLauncher Launcher(string? engine, TimeSpan? window = null) =>
        new(new FakeSettings(new LauncherSettings(EnginePath: engine)), "/base", window ?? TimeSpan.FromSeconds(5));

    [Fact]
    public void Resolve_uses_configured_when_set() =>
        Assert.Equal("/x/my-hp3", GameLauncher.ResolveEnginePath("/base", "/x/my-hp3", isWindows: true));

    [Fact]
    public void Resolve_windows() =>
        Assert.Equal(Path.Combine("/base", "hp3.exe"), GameLauncher.ResolveEnginePath("/base", null, isWindows: true));

    [Fact]
    public void Resolve_unix() =>
        Assert.Equal(Path.Combine("/base", "hp3"), GameLauncher.ResolveEnginePath("/base", "  ", isWindows: false));

    [Fact]
    public void Tail_returns_last_lines()
    {
        var text = string.Join("\n", Enumerable.Range(1, 25).Select(i => $"l{i}")) + "\n";
        Assert.Equal(string.Join("\n", Enumerable.Range(6, 20).Select(i => $"l{i}")), GameLauncher.Tail(text, 20));
    }

    [Fact]
    public async Task Missing_engine()
    {
        var path = Path.Combine(_root, "no-hp3.exe");
        var r = await Launcher(path).LaunchAsync(Game("ok"), TestContext.Current.CancellationToken);
        Assert.False(r.Started);
        Assert.Equal($"Engine not found: {path}", r.Error);
    }

    [Fact]
    public async Task Early_failure_reports_stderr()
    {
        var r = await Launcher(FakeEngine).LaunchAsync(Game("fail"), TestContext.Current.CancellationToken);
        Assert.False(r.Started);
        Assert.Contains("boom", r.Error);
        Assert.DoesNotContain("line 6\n", r.Error + "\n");
        Assert.StartsWith("line 7", r.Error);
    }

    [Fact]
    public async Task Argument_passed_intact()
    {
        var game = Game("ok", "HP3 Ü dir");
        var r = await Launcher(FakeEngine).LaunchAsync(game, TestContext.Current.CancellationToken);
        Assert.True(r.Started, r.Error);
        Assert.Equal(game, File.ReadAllText(Path.Combine(game, "arg.txt")));
        Assert.Equal("1", File.ReadAllText(Path.Combine(game, "argc.txt")));
    }

    [Fact]
    public async Task Long_running_counts_as_started()
    {
        var sw = Stopwatch.StartNew();
        var r = await Launcher(FakeEngine, TimeSpan.FromMilliseconds(500)).LaunchAsync(Game("sleep"), TestContext.Current.CancellationToken);
        Assert.True(r.Started, r.Error);
        Assert.True(sw.Elapsed < TimeSpan.FromSeconds(2.5), $"took {sw.Elapsed}");
    }
}
