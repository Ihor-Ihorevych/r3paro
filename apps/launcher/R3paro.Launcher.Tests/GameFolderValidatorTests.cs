using R3paro.Launcher.Services;

namespace R3paro.Launcher.Tests;

public sealed class GameFolderValidatorTests : IDisposable
{
    private readonly string _root = Path.Combine(Path.GetTempPath(), "r3paro-val-" + Guid.NewGuid().ToString("N"));
    private readonly GameFolderValidator _v = new();

    public GameFolderValidatorTests() => Directory.CreateDirectory(_root);

    public void Dispose() => Directory.Delete(_root, recursive: true);

    private void Layout(string system = "System", string core = "Core.u", string maps = "Maps")
    {
        Directory.CreateDirectory(Path.Combine(_root, system));
        File.WriteAllText(Path.Combine(_root, system, core), "");
        Directory.CreateDirectory(Path.Combine(_root, maps));
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("   ")]
    public void Null_or_empty(string? folder)
    {
        var r = _v.Validate(folder);
        Assert.False(r.IsValid);
        Assert.Equal("Choose the game folder.", r.Message);
    }

    [Fact]
    public void Missing_folder()
    {
        var r = _v.Validate(Path.Combine(_root, "nope"));
        Assert.False(r.IsValid);
        Assert.Equal("Folder does not exist.", r.Message);
    }

    [Fact]
    public void No_System()
    {
        Directory.CreateDirectory(Path.Combine(_root, "Maps"));
        var r = _v.Validate(_root);
        Assert.False(r.IsValid);
        Assert.Equal("No System folder: this is not the HP3 game folder.", r.Message);
    }

    [Fact]
    public void No_CoreU()
    {
        Directory.CreateDirectory(Path.Combine(_root, "System"));
        Directory.CreateDirectory(Path.Combine(_root, "Maps"));
        var r = _v.Validate(_root);
        Assert.False(r.IsValid);
        Assert.Equal($"{Path.Combine("System", "Core.u")} is missing.", r.Message);
    }

    [Fact]
    public void No_Maps()
    {
        Directory.CreateDirectory(Path.Combine(_root, "System"));
        File.WriteAllText(Path.Combine(_root, "System", "Core.u"), "");
        var r = _v.Validate(_root);
        Assert.False(r.IsValid);
        Assert.Equal("No Maps folder: this is not the HP3 game folder.", r.Message);
    }

    [Fact]
    public void Lowercase_layout_valid()
    {
        Layout("system", "core.u", "maps");
        var r = _v.Validate(_root);
        Assert.True(r.IsValid, r.Message);
    }

    [Fact]
    public void Valid_layout()
    {
        Layout();
        var r = _v.Validate(_root);
        Assert.True(r.IsValid);
        Assert.Equal("Ready.", r.Message);
        Assert.True(Directory.Exists(Path.Combine(_root, "Save")));
    }

    [Fact]
    public void Existing_lowercase_save_is_reused()
    {
        Layout("system", "core.u", "maps");
        Directory.CreateDirectory(Path.Combine(_root, "save"));
        Assert.True(_v.Validate(_root).IsValid);
        Assert.Single(Directory.GetDirectories(_root), d => string.Equals(Path.GetFileName(d), "save", StringComparison.OrdinalIgnoreCase));
    }
}
