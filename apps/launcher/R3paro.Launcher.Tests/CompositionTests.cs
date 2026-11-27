using Microsoft.Extensions.DependencyInjection;
using R3paro.Launcher.Services;
using R3paro.Launcher.ViewModels;

namespace R3paro.Launcher.Tests;

public sealed class CompositionTests
{
    [Fact]
    public void AddLauncher_resolves_main_view_model()
    {
        var services = new ServiceCollection()
            .AddLauncher()
            .AddSingleton<IFolderPicker>(new FakePicker(null))
            .AddSingleton<ISettingsStore>(new FakeSettings(new LauncherSettings()));
        using var provider = services.BuildServiceProvider(new ServiceProviderOptions { ValidateOnBuild = true, ValidateScopes = true });
        Assert.NotNull(provider.GetRequiredService<MainViewModel>());
        Assert.IsType<GameFolderValidator>(provider.GetRequiredService<IGameFolderValidator>());
        Assert.IsType<GameLauncher>(provider.GetRequiredService<IGameLauncher>());
    }
}
