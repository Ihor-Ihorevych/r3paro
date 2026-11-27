using Avalonia;
using Avalonia.Controls;
using Avalonia.Headless;
using Avalonia.Headless.XUnit;
using Avalonia.LogicalTree;
using R3paro.Launcher.Services;
using R3paro.Launcher.ViewModels;
using R3paro.Launcher.Views;

[assembly: AvaloniaTestApplication(typeof(R3paro.Launcher.Tests.HeadlessApp))]

namespace R3paro.Launcher.Tests;

public static class HeadlessApp
{
    public static AppBuilder BuildAvaloniaApp() =>
        AppBuilder.Configure<App>().UseHeadless(new AvaloniaHeadlessPlatformOptions());
}

public sealed class MainWindowTests
{
    private static (MainWindow window, Button play, FakeLauncher launcher) Open(TaskCompletionSource? game = null)
    {
        var validator = new FakeValidator();
        validator.Valid.Add("G");
        var launcher = new FakeLauncher();
        if (game is not null)
            launcher.Next = () => Task.FromResult(new LaunchResult(true, null, game.Task));
        var vm = new MainViewModel(validator, new FakeSettings(new LauncherSettings("G")), launcher, new FakePicker(null));
        var window = new MainWindow { DataContext = vm };
        window.Show();
        var play = window.GetLogicalDescendants().OfType<Button>().Single(b => Equals(b.Content, "Play"));
        return (window, play, launcher);
    }

    [AvaloniaFact]
    public void Play_enabled_at_startup_with_valid_saved_folder()
    {
        var (_, play, _) = Open();
        Assert.True(play.IsEffectivelyEnabled);
    }

    [AvaloniaFact]
    public async Task Play_disabled_while_game_runs_and_reenabled_after()
    {
        var game = new TaskCompletionSource();
        var (window, play, _) = Open(game);
        await ((MainViewModel)window.DataContext!).PlayCommand.ExecuteAsync(null);
        Assert.False(play.IsEffectivelyEnabled);
        game.SetResult();
        await ((MainViewModel)window.DataContext!).RunningGame!;
        Assert.True(play.IsEffectivelyEnabled);
    }

    [AvaloniaFact]
    public async Task Closing_while_game_runs_hides_until_game_exits()
    {
        var game = new TaskCompletionSource();
        var (window, _, _) = Open(game);
        var closed = false;
        window.Closed += (_, _) => closed = true;
        await ((MainViewModel)window.DataContext!).PlayCommand.ExecuteAsync(null);

        window.Close();
        Assert.False(closed);
        Assert.False(window.IsVisible);

        game.SetResult();
        for (var i = 0; i < 20 && !closed; i++)
        {
            Avalonia.Threading.Dispatcher.UIThread.RunJobs();
            await Task.Delay(10);
        }
        Assert.True(closed);
    }

    [AvaloniaFact]
    public void Closing_without_game_closes()
    {
        var (window, _, _) = Open();
        var closed = false;
        window.Closed += (_, _) => closed = true;
        window.Close();
        Assert.True(closed);
    }
}
