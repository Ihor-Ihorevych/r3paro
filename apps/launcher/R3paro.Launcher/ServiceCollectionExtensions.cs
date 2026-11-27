using Microsoft.Extensions.DependencyInjection;
using R3paro.Launcher.Services;
using R3paro.Launcher.ViewModels;

namespace R3paro.Launcher;

public static class ServiceCollectionExtensions
{
    public static IServiceCollection AddLauncher(this IServiceCollection s) => s
        .AddSingleton<IGameFolderValidator, GameFolderValidator>()
        .AddSingleton<ISettingsStore>(_ => new JsonSettingsStore(JsonSettingsStore.DefaultPath))
        .AddSingleton<IGameLauncher>(sp =>
            new GameLauncher(sp.GetRequiredService<ISettingsStore>(), AppContext.BaseDirectory, TimeSpan.FromSeconds(5)))
        .AddSingleton<IFolderPicker, AvaloniaFolderPicker>()
        .AddSingleton<MainViewModel>();
}
