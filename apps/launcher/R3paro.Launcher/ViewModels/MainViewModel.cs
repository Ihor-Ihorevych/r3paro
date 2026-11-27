using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using R3paro.Launcher.Services;

namespace R3paro.Launcher.ViewModels;

public sealed partial class MainViewModel : ObservableObject
{
    private readonly IGameFolderValidator _validator;
    private readonly ISettingsStore _settings;
    private readonly IGameLauncher _launcher;
    private readonly IFolderPicker _picker;

    public MainViewModel(IGameFolderValidator validator, ISettingsStore settings, IGameLauncher launcher, IFolderPicker picker)
    {
        _validator = validator;
        _settings = settings;
        _launcher = launcher;
        _picker = picker;
        _loading = true;
        GameFolder = settings.Load().GameFolder;
        Revalidate();
        _loading = false;
    }

    private readonly bool _loading;

    [ObservableProperty]
    public partial string? GameFolder { get; set; }

    [ObservableProperty]
    public partial string Status { get; private set; } = "";

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(PlayCommand))]
    public partial bool IsValid { get; private set; }

    [ObservableProperty]
    public partial string? Error { get; private set; }

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(PlayCommand))]
    public partial bool IsLaunching { get; private set; }

    partial void OnGameFolderChanged(string? value)
    {
        if (_loading) return;
        Revalidate();
        if (!IsValid) return;
        try
        {
            _settings.Save(_settings.Load() with { GameFolder = value });
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            Error = $"Could not save settings: {e.Message}";
        }
    }

    private void Revalidate()
    {
        var result = _validator.Validate(GameFolder);
        IsValid = result.IsValid;
        Status = result.Message;
    }

    [RelayCommand]
    private async Task BrowseAsync()
    {
        var folder = await _picker.PickFolderAsync();
        if (folder is not null)
            GameFolder = folder;
    }

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(PlayCommand))]
    public partial bool IsGameRunning { get; private set; }

    /// <summary>Completes when the game started by Play exits; null before the first launch.</summary>
    public Task? RunningGame { get; private set; }

    private bool CanPlay => IsValid && !IsLaunching && !IsGameRunning;

    [RelayCommand(CanExecute = nameof(CanPlay))]
    private async Task PlayAsync()
    {
        if (!CanPlay) return; // Execute() does not consult CanExecute; never start the game twice
        IsLaunching = true;
        Error = null;
        try
        {
            var result = await _launcher.LaunchAsync(GameFolder!);
            Error = result.Started ? null : result.Error;
            if (result is { Started: true, Running: { IsCompleted: false } running })
                RunningGame = TrackGameAsync(running);
        }
        catch (Exception e)
        {
            // A command exception would be rethrown on the UI thread and close the launcher.
            Error = $"Could not start the game: {e.Message}";
        }
        finally
        {
            IsLaunching = false;
        }
    }

    private async Task TrackGameAsync(Task running)
    {
        IsGameRunning = true;
        try
        {
            await running;
        }
        catch (Exception)
        {
            // The game's exit is all that matters here.
        }
        finally
        {
            IsGameRunning = false;
        }
    }
}
