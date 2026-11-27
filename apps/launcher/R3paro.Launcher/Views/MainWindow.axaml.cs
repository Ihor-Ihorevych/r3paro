using Avalonia.Controls;
using R3paro.Launcher.ViewModels;

namespace R3paro.Launcher.Views;

public partial class MainWindow : Window
{
    public MainWindow() => InitializeComponent();

    protected override void OnClosing(WindowClosingEventArgs e)
    {
        // The game's stderr is piped to this process; if it exits, hp3's next log line fails and the game dies.
        // So while a game runs, closing only hides the window, and the launcher quits once the game exits.
        if (DataContext is MainViewModel { IsGameRunning: true, RunningGame: { } game })
        {
            e.Cancel = true;
            Hide();
            _ = CloseWhenDone(game);
        }
        base.OnClosing(e);
    }

    private async Task CloseWhenDone(Task game)
    {
        try { await game; } catch (Exception) { }
        await Task.Yield(); // let IsGameRunning drop before closing again
        Close();
    }
}
