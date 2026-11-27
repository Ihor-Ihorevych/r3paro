using R3paro.Launcher.Services;
using R3paro.Launcher.ViewModels;

namespace R3paro.Launcher.Tests;

internal sealed class FakeValidator : IGameFolderValidator
{
    public HashSet<string> Valid { get; } = [];
    public ValidationResult Validate(string? folder) =>
        folder is not null && Valid.Contains(folder) ? new(true, "Ready.") : new(false, "Folder does not exist.");
}

internal sealed class FakePicker(string? result) : IFolderPicker
{
    public Task<string?> PickFolderAsync() => Task.FromResult(result);
}

internal sealed class FakeLauncher : IGameLauncher
{
    public Func<Task<LaunchResult>> Next { get; set; } = () => Task.FromResult(new LaunchResult(true, null));
    public List<string> Calls { get; } = [];
    public Task<LaunchResult> LaunchAsync(string gameFolder, CancellationToken ct = default)
    {
        Calls.Add(gameFolder);
        return Next();
    }
}

public sealed class MainViewModelTests
{
    private readonly FakeValidator _validator = new();
    private readonly FakeLauncher _launcher = new();
    private FakeSettings _settings = new(new LauncherSettings());

    private MainViewModel Vm(string? picked = null) => new(_validator, _settings, _launcher, new FakePicker(picked));

    [Fact]
    public void Startup_validates_saved_folder()
    {
        _settings = new FakeSettings(new LauncherSettings("X"));
        var vm = Vm();
        Assert.Equal("X", vm.GameFolder);
        Assert.Equal("Folder does not exist.", vm.Status);
        Assert.False(vm.IsValid);
        Assert.False(vm.PlayCommand.CanExecute(null));
    }

    [Fact]
    public void Startup_with_valid_saved_folder_enables_play()
    {
        _settings = new FakeSettings(new LauncherSettings("G"));
        _validator.Valid.Add("G");
        var vm = Vm();
        Assert.True(vm.IsValid);
        Assert.True(vm.PlayCommand.CanExecute(null));
    }

    [Fact]
    public async Task Browse_sets_folder_validates_and_saves_when_valid()
    {
        _validator.Valid.Add("G");
        var vm = Vm("G");
        await vm.BrowseCommand.ExecuteAsync(null);
        Assert.Equal("G", vm.GameFolder);
        Assert.Equal("Ready.", vm.Status);
        Assert.True(vm.PlayCommand.CanExecute(null));
        Assert.Equal("G", _settings.Value.GameFolder);
    }

    [Fact]
    public async Task Browse_cancel_keeps_folder()
    {
        _settings = new FakeSettings(new LauncherSettings("X"));
        var vm = Vm(picked: null);
        await vm.BrowseCommand.ExecuteAsync(null);
        Assert.Equal("X", vm.GameFolder);
        Assert.Equal(0, _settings.Saves);
    }

    [Fact]
    public async Task Invalid_folder_not_saved()
    {
        var vm = Vm("bad");
        await vm.BrowseCommand.ExecuteAsync(null);
        Assert.Equal("bad", vm.GameFolder);
        Assert.False(vm.IsValid);
        Assert.Equal(0, _settings.Saves);
    }

    [Fact]
    public async Task Saving_keeps_engine_path()
    {
        _settings = new FakeSettings(new LauncherSettings(null, "/opt/hp3"));
        _validator.Valid.Add("G");
        await Vm("G").BrowseCommand.ExecuteAsync(null);
        Assert.Equal(new LauncherSettings("G", "/opt/hp3"), _settings.Value);
    }

    [Fact]
    public async Task Play_failure_shows_error()
    {
        _settings = new FakeSettings(new LauncherSettings("G"));
        _validator.Valid.Add("G");
        _launcher.Next = () => Task.FromResult(new LaunchResult(false, "Engine not found: p"));
        var vm = Vm();
        await vm.PlayCommand.ExecuteAsync(null);
        Assert.Equal("Engine not found: p", vm.Error);
        Assert.False(vm.IsLaunching);
        Assert.Equal(["G"], _launcher.Calls);
    }

    [Fact]
    public async Task Play_disabled_while_launching()
    {
        _settings = new FakeSettings(new LauncherSettings("G"));
        _validator.Valid.Add("G");
        var pending = new TaskCompletionSource<LaunchResult>();
        _launcher.Next = () => pending.Task;
        var vm = Vm();

        var first = vm.PlayCommand.ExecuteAsync(null);
        Assert.True(vm.IsLaunching);
        Assert.False(vm.PlayCommand.CanExecute(null));
        vm.PlayCommand.Execute(null);
        Assert.Single(_launcher.Calls);

        pending.SetResult(new LaunchResult(true, null));
        await first;
        Assert.False(vm.IsLaunching);
        Assert.True(vm.PlayCommand.CanExecute(null));
    }

    [Fact]
    public async Task Play_stays_disabled_while_game_runs()
    {
        _settings = new FakeSettings(new LauncherSettings("G"));
        _validator.Valid.Add("G");
        var game = new TaskCompletionSource();
        _launcher.Next = () => Task.FromResult(new LaunchResult(true, null, game.Task));
        var vm = Vm();

        await vm.PlayCommand.ExecuteAsync(null);
        Assert.True(vm.IsGameRunning);
        Assert.False(vm.PlayCommand.CanExecute(null));
        await vm.PlayCommand.ExecuteAsync(null);
        Assert.Single(_launcher.Calls);

        game.SetResult();
        await vm.RunningGame!;
        await Task.Yield();
        Assert.False(vm.IsGameRunning);
        Assert.True(vm.PlayCommand.CanExecute(null));
    }

    [Fact]
    public async Task Save_failure_shows_error_and_keeps_folder()
    {
        _validator.Valid.Add("G");
        _settings = new FakeSettings(new LauncherSettings()) { SaveError = new UnauthorizedAccessException("denied") };
        var vm = Vm("G");
        await vm.BrowseCommand.ExecuteAsync(null);
        Assert.Equal("G", vm.GameFolder);
        Assert.True(vm.IsValid);
        Assert.Equal("Could not save settings: denied", vm.Error);
    }

    [Fact]
    public async Task Unexpected_launch_exception_shows_error()
    {
        _settings = new FakeSettings(new LauncherSettings("G"));
        _validator.Valid.Add("G");
        _launcher.Next = () => throw new InvalidOperationException("weird");
        var vm = Vm();
        await vm.PlayCommand.ExecuteAsync(null);
        Assert.Equal("Could not start the game: weird", vm.Error);
        Assert.False(vm.IsLaunching);
        Assert.True(vm.PlayCommand.CanExecute(null));
    }

    [Fact]
    public async Task Play_success_clears_error()
    {
        _settings = new FakeSettings(new LauncherSettings("G"));
        _validator.Valid.Add("G");
        var vm = Vm();
        _launcher.Next = () => Task.FromResult(new LaunchResult(false, "boom"));
        await vm.PlayCommand.ExecuteAsync(null);
        _launcher.Next = () => Task.FromResult(new LaunchResult(true, null));
        await vm.PlayCommand.ExecuteAsync(null);
        Assert.Null(vm.Error);
    }
}
