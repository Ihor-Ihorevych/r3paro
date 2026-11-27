using R3paro.Launcher.Services;

namespace R3paro.Launcher.Tests;

public sealed class JsonSettingsStoreTests : IDisposable
{
    private readonly string _dir = Path.Combine(Path.GetTempPath(), "r3paro-set-" + Guid.NewGuid().ToString("N"));
    private string FilePath => Path.Combine(_dir, "sub", "launcher.json");

    public void Dispose()
    {
        if (Directory.Exists(_dir)) Directory.Delete(_dir, recursive: true);
    }

    [Fact]
    public void Missing_file_returns_defaults() =>
        Assert.Equal(new LauncherSettings(), new JsonSettingsStore(FilePath).Load());

    [Fact]
    public void Roundtrip()
    {
        var s = new LauncherSettings(GameFolder: "C:/Games/HP3 Ü", EnginePath: "/opt/hp3");
        new JsonSettingsStore(FilePath).Save(s);
        Assert.Equal(s, new JsonSettingsStore(FilePath).Load());
    }

    [Theory]
    [InlineData("{not json")]
    [InlineData("")]
    [InlineData("null")]
    public void Corrupt_json_returns_defaults(string content)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(FilePath)!);
        File.WriteAllText(FilePath, content);
        Assert.Equal(new LauncherSettings(), new JsonSettingsStore(FilePath).Load());
    }

    [Fact]
    public void Save_creates_directory()
    {
        new JsonSettingsStore(FilePath).Save(new LauncherSettings("x"));
        Assert.True(File.Exists(FilePath));
    }
}
