using Avalonia;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Markup.Xaml;
using Microsoft.Extensions.DependencyInjection;
using R3paro.Launcher.ViewModels;
using R3paro.Launcher.Views;

namespace R3paro.Launcher;

public partial class App : Application
{
    private ServiceProvider? _services;

    public override void Initialize() => AvaloniaXamlLoader.Load(this);

    public override void OnFrameworkInitializationCompleted()
    {
        _services = new ServiceCollection().AddLauncher().BuildServiceProvider();
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            desktop.MainWindow = new MainWindow { DataContext = _services.GetRequiredService<MainViewModel>() };
            desktop.Exit += (_, _) => _services.Dispose();
        }
        base.OnFrameworkInitializationCompleted();
    }
}
